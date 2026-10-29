//! Accounts: offline accounts and Microsoft (MSA) device-code login.
//! Mirrors `launcher/minecraft/auth/`. Storage: `accounts.json`.

use crate::launch::MSA_CLIENT_ID;
use serde::{Deserialize, Serialize};
use std::path::Path;

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
    /// Unix seconds when access_token expires (0 = never).
    #[serde(default)]
    pub expires_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skin_url: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AccountList {
    #[serde(default)]
    pub accounts: Vec<MinecraftAccount>,
    #[serde(default)]
    pub active: Option<String>,
}

impl AccountList {
    pub fn load(path: &Path) -> AccountList {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(self).unwrap_or_default())
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
            http: reqwest::Client::new(),
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
        let (msa_token, refresh_token, expires_in) = self.poll_token(&start).await?;
        self.login_with_msa_token(&msa_token, refresh_token, expires_in).await
    }

    /// Chain the rest of the login from an existing MSA access token (also
    /// used for refresh).
    pub async fn login_with_msa_token(
        &self,
        msa_token: &str,
        refresh_token: String,
        expires_in: u64,
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
            expires_at: now_unix() + expires_in,
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
        let expires_in = json["expires_in"].as_u64().unwrap_or(3600);
        self.login_with_msa_token(&access, new_refresh, expires_in).await
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
