//! Accounts: offline accounts and Microsoft (MSA) device-code login.
//! Mirrors `launcher/minecraft/auth/`. Storage: `accounts.json`.

use crate::launch::MSA_CLIENT_ID;
use serde::{Deserialize, Serialize};
use std::{io::{self, Write}, path::{Path, PathBuf}};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AccountType {
    Msa,
    Offline,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MinecraftAccount {
    pub id: String,
    pub account_type: AccountType,
    pub player_name: String,
    pub profile_id: String,
    /// Access token for minecraftservices (MSA) or "0" (offline).
    pub access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    /// Unix seconds when access_token expires (0 = unknown for MSA, never for offline).
    #[serde(default)]
    pub expires_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skin_url: Option<String>,
}

impl MinecraftAccount {
    pub fn needs_refresh(&self, now: u64) -> bool {
        self.account_type == AccountType::Msa && self.expires_at <= now.saturating_add(300)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AccountList {
    #[serde(default)]
    pub accounts: Vec<MinecraftAccount>,
    #[serde(default)]
    pub active: Option<String>,
}

impl AccountList {
    pub async fn session_for_account(
        path: &Path,
        account_id: &str,
    ) -> Result<Option<crate::launch::AuthSession>, AuthError> {
        let mut accounts = Self::load(path);
        if !accounts.accounts.iter().any(|account| account.id == account_id) {
            return Err(AuthError::AccountRemoved);
        }
        accounts.active = Some(account_id.to_owned());
        accounts.session_for_launch(path).await
    }

    pub async fn session_for_launch(
        &mut self,
        path: &Path,
    ) -> Result<Option<crate::launch::AuthSession>, AuthError> {
        self.session_for_launch_with(path, now_unix(), |account| async move {
            MsaClient::new().refresh(&account).await
        })
        .await
    }

    async fn session_for_launch_with<F, Fut>(
        &mut self,
        path: &Path,
        now: u64,
        refresh: F,
    ) -> Result<Option<crate::launch::AuthSession>, AuthError>
    where
        F: FnOnce(MinecraftAccount) -> Fut,
        Fut: std::future::Future<Output = Result<MinecraftAccount, AuthError>>,
    {
        let Some(account) = self.active_account().cloned() else {
            return Ok(None);
        };
        if !account.needs_refresh(now) {
            return Ok(Some(self.to_session(&account)));
        }
        let refreshed = refresh(account.clone())
            .await
            .map_err(|_| AuthError::SignInAgain)?;
        let (updated, _) = Self::edit_inner(path, false, |updated| {
            for stored in &mut updated.accounts {
                if stored.id == account.id && stored.refresh_token == account.refresh_token {
                    *stored = refreshed.clone();
                }
            }
        })?;
        *self = updated;
        Ok(Some(self.to_session(&refreshed)))
    }

    pub fn load(path: &Path) -> AccountList {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    /// Edit the latest account list, returning any backup of malformed data.
    pub fn edit(path: &Path, edit: impl FnOnce(&mut Self)) -> io::Result<(Self, Option<PathBuf>)> {
        Self::edit_inner(path, true, edit)
    }

    fn edit_inner(
        path: &Path,
        recover_invalid: bool,
        edit: impl FnOnce(&mut Self),
    ) -> io::Result<(Self, Option<PathBuf>)> {
        let parent = account_dir(path);
        std::fs::create_dir_all(parent)?;
        let lock = std::fs::File::options()
            .write(true)
            .create(true)
            .truncate(false)
            .open(path.with_extension("lock"))?;
        fs2::FileExt::lock_exclusive(&lock)?;
        let mut recovered_backup = None;
        let mut accounts = match std::fs::read(path) {
            Ok(json) => match serde_json::from_slice(&json) {
                Ok(accounts) => accounts,
                Err(_) if recover_invalid => {
                    let mut backup = tempfile::Builder::new()
                        .prefix("accounts.json.corrupt-")
                        .tempfile_in(parent)?;
                    backup.write_all(&json)?;
                    let (_, backup_path) = backup.keep().map_err(|error| error.error)?;
                    recovered_backup = Some(backup_path);
                    Self::default()
                }
                Err(error) => return Err(error.into()),
            },
            Err(error) if error.kind() == io::ErrorKind::NotFound => Self::default(),
            Err(error) => return Err(error),
        };
        edit(&mut accounts);
        accounts.save(path)?;
        Ok((accounts, recovered_backup))
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        let parent = account_dir(path);
        std::fs::create_dir_all(parent)?;
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        serde_json::to_writer_pretty(&mut file, self)?;
        file.flush()?;
        file.persist(path).map_err(|error| error.error)?;
        Ok(())
    }

    pub fn active_account(&self) -> Option<&MinecraftAccount> {
        self.active
            .as_deref()
            .and_then(|id| self.accounts.iter().find(|a| a.id == id))
            .or_else(|| self.accounts.first())
    }

    pub fn add_offline(&mut self, player_name: &str) -> &MinecraftAccount {
        let account = MinecraftAccount {
            id: crate::launch::offline_uuid(player_name),
            account_type: AccountType::Offline,
            player_name: player_name.to_string(),
            profile_id: crate::launch::offline_uuid(player_name),
            access_token: "0".to_string(),
            refresh_token: None,
            expires_at: 0,
            skin_url: None,
        };
        self.accounts.retain(|a| !(a.account_type == AccountType::Offline && a.player_name == player_name));
        self.accounts.push(account);
        let id = self.accounts.last().unwrap().id.clone();
        if self.active.is_none() {
            self.active = Some(id);
        }
        self.accounts.last().unwrap()
    }

    pub fn to_session(&self, account: &MinecraftAccount) -> crate::launch::AuthSession {
        crate::launch::AuthSession {
            player_name: account.player_name.clone(),
            uuid: account.profile_id.clone(),
            access_token: account.access_token.clone(),
            user_type: match account.account_type {
                AccountType::Msa => "msa".to_string(),
                AccountType::Offline => "legacy".to_string(),
            },
            session: format!("token:{}:{}", account.access_token, account.profile_id),
            xuid: "0".to_string(),
            client_id: MSA_CLIENT_ID.to_string(),
        }
    }
}

fn account_dir(path: &Path) -> &Path {
    path.parent().filter(|parent| !parent.as_os_str().is_empty()).unwrap_or(Path::new("."))
}

// ---------------------------------------------------------------------------
// MSA device-code flow
// ---------------------------------------------------------------------------

const MSA_DEVICECODE_URL: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/devicecode";
const MSA_TOKEN_URL: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/token";
const XBL_AUTH_URL: &str = "https://user.auth.xboxlive.com/user/authenticate";
const XSTS_AUTH_URL: &str = "https://xsts.auth.xboxlive.com/xsts/authorize";
const MC_LOGIN_URL: &str = "https://api.minecraftservices.com/launcher/login";
const MC_PROFILE_URL: &str = "https://api.minecraftservices.com/minecraft/profile";
const MC_ENTITLEMENTS_URL: &str = "https://api.minecraftservices.com/entitlements/mcstore";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceCodeStart {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    #[serde(default = "default_interval")]
    pub interval: u64,
    #[serde(default)]
    pub expires_in: u64,
    #[serde(default)]
    pub message: String,
}

fn default_interval() -> u64 {
    5
}

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("The selected account was removed before launch")]
    AccountRemoved,
    #[error("Microsoft account refresh failed. Please sign in again before launching.")]
    SignInAgain,
    #[error("network: {0}")]
    Net(#[from] crate::net::NetError),
    #[error("request failed: {0}")]
    Request(String),
    #[error("login declined or expired: {0}")]
    Declined(String),
    #[error("missing entitlement: this account does not own Minecraft")]
    NoEntitlement,
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

pub struct MsaClient {
    http: reqwest::Client,
}

impl MsaClient {
    pub fn new() -> Self {
        MsaClient {
            http: crate::net::configured_http_builder().build().expect("failed to build HTTP client"),
        }
    }

    /// Step 1: start the device-code flow. The user opens `verification_uri`
    /// and enters `user_code`.
    pub async fn start_device_flow(&self) -> Result<DeviceCodeStart, AuthError> {
        let resp = self
            .http
            .post(MSA_DEVICECODE_URL)
            .form(&[
                ("client_id", MSA_CLIENT_ID),
                ("scope", "XboxLive.signin offline_access"),
            ])
            .send()
            .await
            .map_err(|e| AuthError::Request(e.to_string()))?;
        let status = resp.status();
        let text = resp.text().await.map_err(|e| AuthError::Request(e.to_string()))?;
        if !status.is_success() {
            return Err(AuthError::Request(format!("devicecode endpoint: {status} {text}")));
        }
        Ok(serde_json::from_str(&text)?)
    }

    /// Step 2: poll the token endpoint until the user completes sign-in.
    /// Returns (access_token, refresh_token, expires_in).
    pub async fn poll_token(&self, start: &DeviceCodeStart) -> Result<(String, String, u64), AuthError> {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(start.interval.max(1))).await;
            let resp = self
                .http
                .post(MSA_TOKEN_URL)
                .form(&[
                    ("client_id", MSA_CLIENT_ID),
                    ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                    ("device_code", start.device_code.as_str()),
                ])
                .send()
                .await
                .map_err(|e| AuthError::Request(e.to_string()))?;
            let json: serde_json::Value = resp.json().await.map_err(|e| AuthError::Request(e.to_string()))?;
            match json["error"].as_str() {
                None => {
                    let access = json["access_token"].as_str().ok_or_else(|| AuthError::Request("no access_token".into()))?;
                    let refresh = json["refresh_token"].as_str().unwrap_or_default().to_string();
                    let expires = json["expires_in"].as_u64().unwrap_or(3600);
                    return Ok((access.to_string(), refresh, expires));
                }
                Some("authorization_pending") => continue,
                Some("slow_down") => continue,
                Some(other) => return Err(AuthError::Declined(other.to_string())),
            }
        }
    }

    /// Full chain: device flow → XBL → XSTS → minecraftservices → profile.
    pub async fn login_device_flow(&self) -> Result<MinecraftAccount, AuthError> {
        let start = self.start_device_flow().await?;
        let (msa_token, refresh_token, _) = self.poll_token(&start).await?;
        self.login_with_msa_token(&msa_token, refresh_token).await
    }

    /// Chain the rest of the login from an existing MSA access token (also
    /// used for refresh).
    pub async fn login_with_msa_token(
        &self,
        msa_token: &str,
        refresh_token: String,
    ) -> Result<MinecraftAccount, AuthError> {
        // XBL
        let xbl: serde_json::Value = self
            .http
            .post(XBL_AUTH_URL)
            .json(&serde_json::json!({
                "Properties": {
                    "AuthMethod": "RPS",
                    "SiteName": "user.auth.xboxlive.com",
                    "RpsTicket": format!("d={msa_token}")
                },
                "RelyingParty": "http://auth.xboxlive.com",
                "TokenType": "JWT"
            }))
            .send()
            .await
            .map_err(|e| AuthError::Request(e.to_string()))?
            .json()
            .await
            .map_err(|e| AuthError::Request(e.to_string()))?;
        let xbl_token = xbl["Token"]
            .as_str()
            .ok_or_else(|| AuthError::Request("missing XBL token".into()))?
            .to_string();

        // XSTS
        let xsts: serde_json::Value = self
            .http
            .post(XSTS_AUTH_URL)
            .json(&serde_json::json!({
                "Properties": {
                    "SandboxId": "RETAIL",
                    "UserTokens": [xbl_token]
                },
                "RelyingParty": "rp://api.minecraftservices.com/",
                "TokenType": "JWT"
            }))
            .send()
            .await
            .map_err(|e| AuthError::Request(e.to_string()))?
            .json()
            .await
            .map_err(|e| AuthError::Request(e.to_string()))?;
        let xsts_token = xsts["Token"]
            .as_str()
            .ok_or_else(|| AuthError::Request(format!("missing XSTS token: {xsts}")))?
            .to_string();
        let uhs = xsts["DisplayClaims"]["xui"][0]["uhs"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        // Minecraft services login
        let mc_login: serde_json::Value = self
            .http
            .post(MC_LOGIN_URL)
            .header("Content-Type", "application/json")
            .body(serde_json::json!({
                "xtoken": format!("XBL3.0 x={uhs};{xsts_token}"),
                "platform": "PC_LAUNCHER"
            })
            .to_string())
            .send()
            .await
            .map_err(|e| AuthError::Request(e.to_string()))?
            .json()
            .await
            .map_err(|e| AuthError::Request(e.to_string()))?;
        let mc_token = mc_login["access_token"]
            .as_str()
            .ok_or_else(|| AuthError::Request(format!("missing minecraft token: {mc_login}")))?
            .to_string();

        // Profile
        let profile: serde_json::Value = self
            .http
            .get(MC_PROFILE_URL)
            .bearer_auth(&mc_token)
            .send()
            .await
            .map_err(|e| AuthError::Request(e.to_string()))?
            .json()
            .await
            .map_err(|e| AuthError::Request(e.to_string()))?;
        let profile_id = profile["id"]
            .as_str()
            .ok_or_else(|| AuthError::Request(format!("no minecraft profile: {profile}")))?
            .to_string();
        let player_name = profile["name"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let skin_url = profile["skins"]
            .as_array()
            .and_then(|s| s.first())
            .and_then(|s| s["url"].as_str())
            .map(String::from);

        Ok(MinecraftAccount {
            id: profile_id.clone(),
            account_type: AccountType::Msa,
            player_name,
            profile_id,
            access_token: mc_token,
            refresh_token: Some(refresh_token),
            expires_at: now_unix().saturating_add(mc_login["expires_in"].as_u64().unwrap_or(0)),
            skin_url,
        })
    }

    /// Refresh an MSA account with its refresh token.
    pub async fn refresh(&self, account: &MinecraftAccount) -> Result<MinecraftAccount, AuthError> {
        let Some(refresh_token) = &account.refresh_token else {
            return Err(AuthError::Request("no refresh token stored".into()));
        };
        let resp = self
            .http
            .post(MSA_TOKEN_URL)
            .form(&[
                ("client_id", MSA_CLIENT_ID),
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token.as_str()),
            ])
            .send()
            .await
            .map_err(|e| AuthError::Request(e.to_string()))?;
        let json: serde_json::Value = resp.json().await.map_err(|e| AuthError::Request(e.to_string()))?;
        let access = json["access_token"]
            .as_str()
            .ok_or_else(|| AuthError::Request(format!("refresh failed: {json}")))?
            .to_string();
        let new_refresh = json["refresh_token"].as_str().unwrap_or(refresh_token).to_string();
        self.login_with_msa_token(&access, new_refresh).await
    }

    /// Verify ownership via the entitlements endpoint.
    pub async fn check_entitlements(&self, mc_token: &str) -> Result<bool, AuthError> {
        let json: serde_json::Value = self
            .http
            .get(MC_ENTITLEMENTS_URL)
            .bearer_auth(mc_token)
            .send()
            .await
            .map_err(|e| AuthError::Request(e.to_string()))?
            .json()
            .await
            .map_err(|e| AuthError::Request(e.to_string()))?;
        Ok(json["items"].as_array().map(|a| !a.is_empty()).unwrap_or(false))
    }
}

impl Default for MsaClient {
    fn default() -> Self {
        Self::new()
    }
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn pending_launch_keeps_the_selected_account_if_the_active_account_changes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("accounts.json");
        let mut accounts = AccountList::default();
        let player_id = accounts.add_offline("Player").id.clone();
        accounts.active = Some(accounts.add_offline("Other").id.clone());
        accounts.save(&path).unwrap();
        let session = AccountList::session_for_account(&path, &player_id).await.unwrap().unwrap();
        assert_eq!(session.player_name, "Player");
        assert_eq!(AccountList::load(&path).active_account().unwrap().player_name, "Other");
        accounts.accounts.remove(0);
        accounts.save(&path).unwrap();
        assert!(matches!(AccountList::session_for_account(&path, &player_id).await, Err(AuthError::AccountRemoved)));
    }

    #[test]
    fn account_edits_recover_from_invalid_json_and_keep_a_backup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("accounts.json");
        let malformed = b"{not valid JSON";
        std::fs::write(&path, malformed).unwrap();
        let (_, backup) = AccountList::edit(&path, |accounts| {
            accounts.add_offline("Player");
        }).unwrap();
        assert_eq!(std::fs::read(backup.unwrap()).unwrap(), malformed);
        assert_eq!(AccountList::load(&path).accounts[0].player_name, "Player");
    }

    #[test]
    fn changing_active_account_preserves_credentials_refreshed_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("accounts.json");
        let mut gui = AccountList::default();
        gui.add_offline("Online");
        gui.accounts[0].account_type = AccountType::Msa;
        gui.accounts[0].refresh_token = Some("old".into());
        let offline_id = gui.add_offline("Offline").id.clone();
        gui.save(&path).unwrap();
        AccountList::edit(&path, |accounts| {
            accounts.accounts[0].refresh_token = Some("refreshed".into());
        }).unwrap();
        assert_eq!(gui.accounts[0].refresh_token.as_deref(), Some("old"));
        AccountList::edit(&path, |accounts| accounts.active = Some(offline_id.clone())).unwrap();
        let saved = AccountList::load(&path);
        assert_eq!(saved.active, Some(offline_id));
        assert_eq!(saved.accounts[0].refresh_token.as_deref(), Some("refreshed"));
    }

    #[tokio::test]
    async fn expired_launch_refreshes_and_persists() {
        let path = std::env::temp_dir().join(format!("prism-auth-{}.json", uuid::Uuid::new_v4()));
        let mut list = AccountList::default();
        list.add_offline("Notch");
        list.accounts[0].account_type = AccountType::Msa;
        list.accounts[0].expires_at = 100;
        list.accounts[0].refresh_token = Some("old-refresh".into());
        list.save(&path).unwrap();
        let session = list
            .session_for_launch_with(&path, 101, |mut account| async move {
                account.access_token = "new-access".into();
                account.refresh_token = Some("new-refresh".into());
                account.expires_at = 5000;
                Ok(account)
            })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(session.access_token, "new-access");
        let saved = AccountList::load(&path);
        assert_eq!(saved.accounts[0].access_token, "new-access");
        assert_eq!(
            saved.accounts[0].refresh_token.as_deref(),
            Some("new-refresh")
        );
        assert_eq!(saved.accounts[0].expires_at, 5000);
        std::fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn refresh_preserves_accounts_edited_on_disk() {
        for remove_original in [false, true] {
            let path =
                std::env::temp_dir().join(format!("prism-auth-{}.json", uuid::Uuid::new_v4()));
            let mut captured = AccountList::default();
            captured.add_offline("Original");
            captured.accounts[0].account_type = AccountType::Msa;
            captured.save(&path).unwrap();
            let mut current = captured.clone();
            let added_id = current.add_offline("Added").id.clone();
            current.active = Some(added_id.clone());
            if remove_original {
                current.accounts.remove(0);
            }
            let session = captured
                .session_for_launch_with(&path, 1000, |mut account| async {
                    current.save(&path).unwrap();
                    account.access_token = "refreshed-access".into();
                    account.refresh_token = Some("refreshed-refresh".into());
                    account.expires_at = 5000;
                    Ok(account)
                })
                .await
                .unwrap()
                .unwrap();
            assert_eq!(session.access_token, "refreshed-access");
            let saved = AccountList::load(&path);
            assert_eq!(saved.active.as_deref(), Some(added_id.as_str()));
            assert_eq!(saved.accounts.last().unwrap().player_name, "Added");
            assert_eq!(saved.accounts.len(), if remove_original { 1 } else { 2 });
            if !remove_original {
                assert_eq!(saved.accounts[0].access_token, "refreshed-access");
                assert_eq!(
                    saved.accounts[0].refresh_token.as_deref(),
                    Some("refreshed-refresh")
                );
                assert_eq!(saved.accounts[0].expires_at, 5000);
            }
            assert_eq!(
                serde_json::to_value(&captured).unwrap(),
                serde_json::to_value(&saved).unwrap()
            );
            std::fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn refresh_margin_and_account_types() {
        let mut list = AccountList::default();
        list.add_offline("Player");
        let account = &mut list.accounts[0];
        assert!(!account.needs_refresh(1000));
        account.account_type = AccountType::Msa;
        for expiry in [0, 999, 1000, 1299, 1300] {
            account.expires_at = expiry;
            assert!(account.needs_refresh(1000));
        }
        account.expires_at = 1301;
        assert!(!account.needs_refresh(1000));
    }

    #[tokio::test]
    async fn empty_offline_and_valid_accounts_skip_refresh() {
        let path = std::env::temp_dir().join(format!("prism-auth-{}.json", uuid::Uuid::new_v4()));
        let mut list = AccountList::default();
        assert!(list
            .session_for_launch_with(&path, 1000, |_| async {
                panic!("empty list must not refresh")
            })
            .await
            .unwrap()
            .is_none());
        list.add_offline("Player");
        for msa in [false, true] {
            if msa {
                list.accounts[0].account_type = AccountType::Msa;
                list.accounts[0].expires_at = 2000;
            }
            let session = list
                .session_for_launch_with(&path, 1000, |_| async {
                    panic!("valid or offline account must not refresh")
                })
                .await
                .unwrap()
                .unwrap();
            assert_eq!(session.access_token, "0");
        }
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn refresh_and_persistence_failures_block_launch() {
        let path = std::env::temp_dir().join(format!("prism-auth-{}.json", uuid::Uuid::new_v4()));
        let mut list = AccountList::default();
        list.add_offline("Player");
        list.accounts[0].account_type = AccountType::Msa;
        list.save(&path).unwrap();
        let before = std::fs::read(&path).unwrap();
        let error = list
            .session_for_launch_with(&path, 1000, |_| async {
                Err(AuthError::Request("revoked".into()))
            })
            .await
            .unwrap_err();
        assert!(error.to_string().contains("sign in again"));
        assert_eq!(std::fs::read(&path).unwrap(), before);
        let error = list
            .session_for_launch_with(&path.join("impossible"), 1000, |mut account| async move {
                account.access_token = "refreshed".into();
                Ok(account)
            })
            .await
            .unwrap_err();
        assert!(matches!(error, AuthError::Io(_)));
        assert_eq!(list.accounts[0].access_token, "0");
        let error = list.session_for_launch(&path).await.unwrap_err();
        assert!(error.to_string().contains("sign in again"));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn account_list_offline() {
        let mut list = AccountList::default();
        list.add_offline("Notch");
        assert_eq!(list.accounts.len(), 1);
        assert_eq!(list.accounts[0].account_type, AccountType::Offline);
        assert_eq!(list.accounts[0].profile_id, crate::launch::offline_uuid("Notch"));
        let json = serde_json::to_string(&list).unwrap();
        let parsed: AccountList = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.accounts[0].player_name, "Notch");
    }
}
