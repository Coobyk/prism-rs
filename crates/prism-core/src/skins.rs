//! Local skin library and authenticated Minecraft skin/cape management.

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};

use crate::{
    auth::{AccountList, AccountType},
    Paths,
};

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read};

    #[test]
    fn skins_client_uses_saved_proxy() {
        if std::env::var_os("PRISM_SKINS_PROXY_CHILD").is_some() {
            tokio::runtime::Runtime::new().unwrap().block_on(async {
                SkinClient::new()
                    .http
                    .get("http://skin-proxy.invalid/check")
                    .timeout(std::time::Duration::from_secs(5))
                    .send()
                    .await
                    .unwrap()
                    .error_for_status()
                    .unwrap();
            });
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let (base, handle) = server("200 OK", PROFILE);
        let paths = Paths {
            root: root.path().into(),
        };
        let mut settings = crate::settings::Settings::default();
        settings.set("ProxyType", "HTTP");
        settings.set("ProxyAddr", "127.0.0.1");
        settings.set("ProxyPort", base.rsplit(':').next().unwrap());
        settings.save(&paths.global_config()).unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "skins::tests::skins_client_uses_saved_proxy",
                "--nocapture",
            ])
            .env("PRISM_SKINS_PROXY_CHILD", "1")
            .env("PRISM_RS_DATA", root.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(handle
            .join()
            .unwrap()
            .0
            .starts_with("GET http://skin-proxy.invalid/check HTTP/1.1"));
    }

    #[tokio::test]
    #[ignore = "requires live Minecraft services"]
    async fn skins_live_username_import() {
        let root = tempfile::tempdir().unwrap();
        let mut library = SkinLibrary::load(&Paths {
            root: root.path().into(),
        })
        .unwrap();
        let skin = library.import_username("jeb_").await.unwrap();
        let bytes = fs::read(library.path(&skin.name).unwrap()).unwrap();
        validate_png(&bytes).unwrap();
        println!(
            "Imported {}: {} bytes, {:?}, {}",
            skin.name,
            bytes.len(),
            skin.model,
            skin.url.unwrap()
        );
        assert_eq!(library.list().len(), 1);
    }

    #[tokio::test]
    async fn skins_download_retries_only_bounded_rate_limits() {
        for (statuses, retry_after, expected) in [
            (vec![200], Some("0"), 200),
            (vec![429, 200], Some("0"), 200),
            (vec![429, 429, 429, 429], Some("0"), 429),
            (vec![404], Some("0"), 404),
            (vec![429], Some("31"), 429),
            (vec![429], Some("invalid"), 429),
            (vec![429, 200], None, 200),
        ] {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let handle = std::thread::spawn(move || {
                for status in statuses {
                    let mut stream = listener.accept().unwrap().0;
                    stream
                        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                        .unwrap();
                    let mut reader = BufReader::new(&mut stream);
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    assert!(line.starts_with("GET / HTTP/1.1"));
                    loop {
                        line.clear();
                        assert_ne!(reader.read_line(&mut line).unwrap(), 0);
                        if line == "\r\n" {
                            break;
                        }
                    }
                    let header = retry_after
                        .map(|v| format!("Retry-After: {v}\r\n"))
                        .unwrap_or_default();
                    write!(stream, "HTTP/1.1 {status} Test\r\n{header}Content-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
                }
            });
            let result = download_response(&reqwest::Client::new(), &url).await;
            let status = match result {
                Ok(response) => response.status().as_u16(),
                Err(SkinError::Http(error)) => error.status().unwrap().as_u16(),
                Err(error) => panic!("{error}"),
            };
            assert_eq!(status, expected);
            handle.join().unwrap();
        }
    }

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgba8(width, height)
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        bytes.into_inner()
    }

    #[test]
    fn skins_library_roundtrip_rename_delete_validation() {
        let root = tempfile::tempdir().unwrap();
        let paths = Paths {
            root: root.path().into(),
        };
        let mut library = SkinLibrary::load(&paths).unwrap();
        assert!(library.list().is_empty());
        assert!(library
            .import_bytes("bad", b"not png", SkinModel::Classic, None)
            .is_err());
        assert!(library
            .import_bytes("bad", &png(32, 32), SkinModel::Classic, None)
            .is_err());
        assert!(library
            .import_bytes("bad", &png(64, 16), SkinModel::Classic, None)
            .is_err());
        let source = root.path().join("source.png");
        fs::write(&source, png(64, 32)).unwrap();
        let first = library.import_file(&source).unwrap();
        let second = library
            .import_bytes(
                "source",
                &png(64, 64),
                SkinModel::Slim,
                Some("https://example.com/texture".into()),
            )
            .unwrap();
        assert_eq!(second.name, "source1");
        library.set_cape(&first.name, "cape").unwrap();
        library.set_model(&first.name, SkinModel::Slim).unwrap();
        library
            .set_texture(&first.name, "https://example.com/uploaded", "uploaded")
            .unwrap();
        let renamed = library.rename(&first.name, "source1").unwrap();
        assert_eq!(renamed, "source11");
        assert!(!paths.skins_dir().join("source.png").exists());
        assert!(library.path(&renamed).unwrap().exists());
        let sanitized = library.rename(&renamed, "../unsafe").unwrap();
        assert!(!sanitized.contains('/'));
        let loaded = SkinLibrary::load(&paths).unwrap();
        assert_eq!(loaded.list().len(), 2);
        let saved = loaded.list().iter().find(|s| s.name == sanitized).unwrap();
        assert_eq!(saved.cape_id, "cape");
        assert_eq!(saved.model, SkinModel::Slim);
        assert_eq!(saved.url.as_deref(), Some("https://example.com/uploaded"));
        assert_eq!(saved.texture_id.as_deref(), Some("uploaded"));
        assert_eq!(
            loaded
                .list()
                .iter()
                .find(|s| s.name == second.name)
                .unwrap(),
            &second
        );
        library.delete(&sanitized).unwrap();
        assert!(!paths.skins_dir().join(format!("{sanitized}.png")).exists());
        assert!(library.delete("missing").is_err());
        assert!(library.rename("missing", "new").is_err());
        assert!(library.set_model("missing", SkinModel::Classic).is_err());
        assert!(library.set_cape("missing", "").is_err());
        assert!(library.set_texture("missing", "url", "id").is_err());
        assert_eq!(SkinLibrary::load(&paths).unwrap().list(), &[second]);
        fs::write(paths.skins_dir().join("index.json"), "invalid").unwrap();
        assert!(SkinLibrary::load(&paths).is_err());
    }

    fn server(
        status: &str,
        response: &[u8],
    ) -> (String, std::thread::JoinHandle<(String, Vec<u8>)>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let status = status.to_owned();
        let response = response.to_vec();
        let handle = std::thread::spawn(move || {
            let mut stream = listener.accept().unwrap().0;
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut reader = BufReader::new(&mut stream);
            let mut headers = String::new();
            loop {
                let start = headers.len();
                assert_ne!(reader.read_line(&mut headers).unwrap(), 0);
                if &headers[start..] == "\r\n" {
                    break;
                }
            }
            let length = headers
                .to_ascii_lowercase()
                .lines()
                .find_map(|l| l.strip_prefix("content-length: "))
                .unwrap_or("0")
                .parse()
                .unwrap();
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            write!(stream, "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", response.len()).unwrap();
            stream.write_all(&response).unwrap();
            (headers, body)
        });
        (base, handle)
    }

    fn accounts(path: &Path, account_type: AccountType) {
        AccountList::edit(path, |list| {
            list.accounts.push(crate::auth::MinecraftAccount {
                id: "account".into(),
                account_type,
                player_name: "Player".into(),
                profile_id: "uuid".into(),
                access_token: "test-token".into(),
                refresh_token: None,
                expires_at: u64::MAX,
                skin_url: None,
            });
        })
        .unwrap();
    }

    const PROFILE: &[u8] = br#"{"id":"uuid","name":"Player","skins":[{"id":"texture","state":"ACTIVE","url":"https://example.com/texture","variant":"SLIM"}],"capes":[{"id":"cape","alias":"Cape","url":"https://example.com/cape","state":"ACTIVE"}]}"#;

    #[tokio::test]
    async fn skins_untrusted_sources_upload_saved_png() {
        let root = tempfile::tempdir().unwrap();
        let paths = Paths {
            root: root.path().into(),
        };
        let mut library = SkinLibrary::load(&paths).unwrap();
        let bytes = png(64, 64);
        let (source, download) = server("200 OK", &bytes);
        let mut skin = library.import_url("download", &source).await.unwrap();
        download.join().unwrap();
        assert_eq!(skin.url, None);
        assert_eq!(skin.texture_id, None);
        accounts(&paths.accounts_file(), AccountType::Msa);
        let file = library.path(&skin.name).unwrap();
        for url in [
            None,
            Some("https://expired.example/skin.png?signature=old"),
            Some("https://textures.minecraft.net.evil/texture/id"),
            Some("ftp://textures.minecraft.net/texture/id"),
        ] {
            skin.url = url.map(str::to_owned);
            let (base, upload) = server("200 OK", PROFILE);
            let mut client = SkinClient::new();
            client.base_url = base;
            client
                .upload_skin(&paths.accounts_file(), "account", &skin, &file)
                .await
                .unwrap();
            let (headers, body) = upload.join().unwrap();
            assert!(headers.to_ascii_lowercase().contains("multipart/form-data"));
            assert!(body.windows(bytes.len()).any(|part| part == bytes));
        }
    }

    #[test]
    fn skins_long_duplicate_names_survive_reload() {
        for name in ["a".repeat(100), "é".repeat(100)] {
            let root = tempfile::tempdir().unwrap();
            let paths = Paths {
                root: root.path().into(),
            };
            let mut library = SkinLibrary::load(&paths).unwrap();
            let bytes = png(64, 64);
            for _ in 0..12 {
                let skin = library
                    .import_bytes(&name, &bytes, SkinModel::Slim, None)
                    .unwrap();
                assert!(skin.name.chars().count() <= 100);
                assert_eq!(sanitized(&skin.name), skin.name);
            }
            let renamed = library.rename(&name, &name).unwrap();
            assert!(renamed.chars().count() <= 100);
            let loaded = SkinLibrary::load(&paths).unwrap();
            assert_eq!(loaded.list().len(), 12);
            assert!(loaded
                .list()
                .iter()
                .all(|skin| skin.model == SkinModel::Slim));
            assert_eq!(
                loaded
                    .list()
                    .iter()
                    .map(|s| s.name.clone())
                    .collect::<std::collections::HashSet<_>>()
                    .len(),
                12
            );
        }
    }

    #[tokio::test]
    async fn skins_api_upload_capes_profile_and_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("accounts.json");
        accounts(&path, AccountType::Msa);
        let file = dir.path().join("skin.png");
        fs::write(&file, png(64, 64)).unwrap();
        let mut skin = Skin {
            name: "Skin".into(),
            model: SkinModel::Slim,
            cape_id: String::new(),
            url: None,
            texture_id: None,
        };
        let mut client = SkinClient::new();
        let (base, handle) = server("200 OK", PROFILE);
        client.base_url = base;
        client
            .upload_skin(&path, "account", &skin, &file)
            .await
            .unwrap();
        let (headers, body) = handle.join().unwrap();
        assert!(headers.starts_with("POST /minecraft/profile/skins HTTP/1.1"));
        assert!(headers
            .to_ascii_lowercase()
            .contains("authorization: bearer test-token"));
        let body = String::from_utf8_lossy(&body);
        assert!(body.contains("name=\"variant\"\r\n\r\nslim"));
        assert!(body.contains("name=\"file\"; filename=\"Skin.png\""));
        assert!(body.contains("Content-Type: image/png"));
        assert_eq!(
            AccountList::load(&path).accounts[0].skin_url.as_deref(),
            Some("https://example.com/texture")
        );
        skin.url = Some("https://textures.minecraft.net/texture/known".into());
        let (base, handle) = server("200 OK", PROFILE);
        client.base_url = base;
        client
            .upload_skin(&path, "account", &skin, &file)
            .await
            .unwrap();
        let (headers, body) = handle.join().unwrap();
        assert!(headers.starts_with("POST /minecraft/profile/skins "));
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap(),
            serde_json::json!({"variant":"slim","url":"https://textures.minecraft.net/texture/known"})
        );
        let (base, handle) = server("204 No Content", b"");
        client.base_url = base;
        client
            .set_active_cape(&path, "account", "cape")
            .await
            .unwrap();
        let (headers, body) = handle.join().unwrap();
        assert!(headers.starts_with("PUT /minecraft/profile/capes/active "));
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap(),
            serde_json::json!({"capeId":"cape"})
        );
        let (base, handle) = server("204 No Content", b"");
        client.base_url = base;
        client.hide_cape(&path, "account").await.unwrap();
        let (headers, body) = handle.join().unwrap();
        assert!(headers.starts_with("DELETE /minecraft/profile/capes/active "));
        assert!(body.is_empty());
        let (base, handle) = server("200 OK", PROFILE);
        client.base_url = base;
        let profile = client.fetch_profile(&path, "account").await.unwrap();
        assert_eq!(profile.capes[0].id, "cape");
        assert!(handle
            .join()
            .unwrap()
            .0
            .starts_with("GET /minecraft/profile "));
        let (base, handle) = server("200 OK", br#"{"id":"uuid","name":"Player"}"#);
        client.base_url = base;
        assert!(client
            .fetch_profile(&path, "account")
            .await
            .unwrap()
            .skins
            .is_empty());
        handle.join().unwrap();
        assert!(AccountList::load(&path).accounts[0].skin_url.is_none());
        let (base, handle) = server("403 Forbidden", b"{}");
        client.base_url = base;
        assert!(client.hide_cape(&path, "account").await.is_err());
        handle.join().unwrap();
        let (base, handle) = server("403 Forbidden", b"{}");
        client.base_url = base;
        assert!(client.reset_skin(&path, "account").await.is_err());
        assert!(handle
            .join()
            .unwrap()
            .0
            .starts_with("DELETE /minecraft/profile/skins/active "));
        skin.url = None;
        fs::write(&file, b"invalid").unwrap();
        assert!(matches!(
            client.upload_skin(&path, "account", &skin, &file).await,
            Err(SkinError::InvalidImage)
        ));
        assert!(client.fetch_profile(&path, "missing").await.is_err());
        let offline = dir.path().join("offline.json");
        accounts(&offline, AccountType::Offline);
        assert!(matches!(
            client.fetch_profile(&offline, "account").await,
            Err(SkinError::Offline)
        ));
    }

    #[tokio::test]
    async fn skins_url_import_validates_response() {
        let dir = tempfile::tempdir().unwrap();
        let mut library = SkinLibrary::load(&Paths {
            root: dir.path().into(),
        })
        .unwrap();
        let (base, handle) = server("200 OK", &png(64, 64));
        let skin = library.import_url("download", &base).await.unwrap();
        assert_eq!(skin.url, None);
        handle.join().unwrap();
        let (base, handle) = server("200 OK", b"bad image");
        assert!(matches!(
            library.import_url("bad", &base).await,
            Err(SkinError::InvalidImage)
        ));
        handle.join().unwrap();
        assert_eq!(library.list().len(), 1);
        assert!(library.import_username("../invalid").await.is_err());
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SkinError {
    #[error("Skin management requires a Microsoft account")]
    Offline,
    #[error("Skin not found: {0}")]
    NotFound(String),
    #[error("Skin images must be 64x64 or 64x32 pixel PNG files")]
    InvalidImage,
    #[error("Invalid skin data: {0}")]
    InvalidData(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    #[error(transparent)]
    Auth(#[from] crate::auth::AuthError),
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProfileSkin {
    pub id: String,
    pub state: String,
    pub url: String,
    pub variant: SkinModel,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Cape {
    pub id: String,
    pub alias: String,
    pub url: String,
    pub state: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MinecraftProfile {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub skins: Vec<ProfileSkin>,
    #[serde(default)]
    pub capes: Vec<Cape>,
}

pub struct SkinClient {
    http: reqwest::Client,
    pub base_url: String,
}

impl Default for SkinClient {
    fn default() -> Self {
        Self::new()
    }
}

impl SkinClient {
    pub fn new() -> Self {
        Self {
            http: crate::net::configured_http_builder()
                .build()
                .expect("failed to build HTTP client"),
            base_url: "https://api.minecraftservices.com".into(),
        }
    }

    async fn token(&self, accounts_path: &Path, account_id: &str) -> Result<String, SkinError> {
        let accounts = AccountList::load(accounts_path);
        let account = accounts
            .accounts
            .iter()
            .find(|a| a.id == account_id)
            .ok_or(crate::auth::AuthError::AccountRemoved)?;
        if account.account_type != AccountType::Msa {
            return Err(SkinError::Offline);
        }
        Ok(AccountList::session_for_account(accounts_path, account_id)
            .await?
            .ok_or(crate::auth::AuthError::AccountRemoved)?
            .access_token)
    }

    fn request(&self, method: reqwest::Method, path: &str, token: &str) -> reqwest::RequestBuilder {
        self.http
            .request(
                method,
                format!("{}{path}", self.base_url.trim_end_matches('/')),
            )
            .bearer_auth(token)
    }

    async fn profile_with_token(&self, token: &str) -> Result<MinecraftProfile, SkinError> {
        Ok(self
            .request(reqwest::Method::GET, "/minecraft/profile", token)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }

    fn store_profile(
        &self,
        path: &Path,
        id: &str,
        profile: &MinecraftProfile,
    ) -> Result<(), SkinError> {
        let url = profile
            .skins
            .iter()
            .find(|s| s.state == "ACTIVE")
            .map(|s| s.url.clone());
        AccountList::edit(path, |accounts| {
            if let Some(account) = accounts.accounts.iter_mut().find(|a| a.id == id) {
                account.skin_url = url;
            }
        })?;
        Ok(())
    }

    pub async fn fetch_profile(
        &self,
        accounts_path: &Path,
        account_id: &str,
    ) -> Result<MinecraftProfile, SkinError> {
        let token = self.token(accounts_path, account_id).await?;
        let profile = self.profile_with_token(&token).await?;
        self.store_profile(accounts_path, account_id, &profile)?;
        Ok(profile)
    }

    pub async fn upload_skin(
        &self,
        accounts_path: &Path,
        account_id: &str,
        skin: &Skin,
        file: &Path,
    ) -> Result<MinecraftProfile, SkinError> {
        let token = self.token(accounts_path, account_id).await?;
        let request = self.request(reqwest::Method::POST, "/minecraft/profile/skins", &token);
        let request = if let Some(url) = skin.url.as_ref().filter(|u| is_mojang_texture_url(u)) {
            request.json(&serde_json::json!({"variant": skin.model.variant(), "url": url}))
        } else {
            let bytes = fs::read(file)?;
            validate_png(&bytes)?;
            let part = reqwest::multipart::Part::bytes(bytes)
                .file_name(format!("{}.png", skin.name))
                .mime_str("image/png")?;
            request.multipart(
                reqwest::multipart::Form::new()
                    .text("variant", skin.model.variant())
                    .part("file", part),
            )
        };
        let profile = request.send().await?.error_for_status()?.json().await?;
        self.store_profile(accounts_path, account_id, &profile)?;
        Ok(profile)
    }

    pub async fn set_active_cape(
        &self,
        accounts_path: &Path,
        account_id: &str,
        cape_id: &str,
    ) -> Result<(), SkinError> {
        let token = self.token(accounts_path, account_id).await?;
        self.request(
            reqwest::Method::PUT,
            "/minecraft/profile/capes/active",
            &token,
        )
        .json(&serde_json::json!({"capeId": cape_id}))
        .send()
        .await?
        .error_for_status()?;
        Ok(())
    }

    pub async fn hide_cape(&self, accounts_path: &Path, account_id: &str) -> Result<(), SkinError> {
        let token = self.token(accounts_path, account_id).await?;
        self.request(
            reqwest::Method::DELETE,
            "/minecraft/profile/capes/active",
            &token,
        )
        .send()
        .await?
        .error_for_status()?;
        Ok(())
    }

    pub async fn reset_skin(
        &self,
        accounts_path: &Path,
        account_id: &str,
    ) -> Result<MinecraftProfile, SkinError> {
        let token = self.token(accounts_path, account_id).await?;
        self.request(
            reqwest::Method::DELETE,
            "/minecraft/profile/skins/active",
            &token,
        )
        .send()
        .await?
        .error_for_status()?;
        let profile = self.profile_with_token(&token).await?;
        self.store_profile(accounts_path, account_id, &profile)?;
        Ok(profile)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum SkinModel {
    #[default]
    #[serde(alias = "classic")]
    Classic,
    #[serde(alias = "slim")]
    Slim,
}

impl SkinModel {
    pub fn variant(self) -> &'static str {
        match self {
            Self::Classic => "classic",
            Self::Slim => "slim",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Skin {
    pub name: String,
    #[serde(default)]
    pub model: SkinModel,
    #[serde(default, rename = "capeId")]
    pub cape_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub texture_id: Option<String>,
}

#[derive(Default, Serialize, Deserialize)]
struct Index {
    skins: Vec<Skin>,
}

pub struct SkinLibrary {
    dir: PathBuf,
    index: Index,
}

fn validate_png(bytes: &[u8]) -> Result<(), SkinError> {
    let (width, height) =
        image::ImageReader::with_format(std::io::Cursor::new(bytes), image::ImageFormat::Png)
            .into_dimensions()
            .map_err(|_| SkinError::InvalidImage)?;
    if width != 64 || ![32, 64].contains(&height) {
        return Err(SkinError::InvalidImage);
    }
    image::load_from_memory_with_format(bytes, image::ImageFormat::Png)
        .map_err(|_| SkinError::InvalidImage)?;
    Ok(())
}

async fn download_response(
    http: &reqwest::Client,
    url: &str,
) -> Result<reqwest::Response, SkinError> {
    let mut retries = 0;
    loop {
        let response = http.get(url).send().await?;
        if response.status() != reqwest::StatusCode::TOO_MANY_REQUESTS || retries == 3 {
            return Ok(response.error_for_status()?);
        }
        let delay = match response.headers().get(reqwest::header::RETRY_AFTER) {
            Some(value) => match value.to_str().ok().and_then(|s| s.parse::<u64>().ok()) {
                Some(seconds) if seconds <= 30 => seconds,
                _ => return Ok(response.error_for_status()?),
            },
            None => 1 << retries,
        };
        drop(response);
        tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
        retries += 1;
    }
}

fn sanitized(name: &str) -> String {
    let name: String = name
        .trim()
        .trim_end_matches(".png")
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '-' | '_' | ' ') {
                c
            } else {
                '_'
            }
        })
        .take(100)
        .collect();
    if name.trim().is_empty() {
        "Skin".into()
    } else {
        name
    }
}

fn is_mojang_texture_url(url: &str) -> bool {
    reqwest::Url::parse(url).is_ok_and(|url| {
        matches!(url.scheme(), "http" | "https")
            && url.host_str() == Some("textures.minecraft.net")
            && url.username().is_empty()
            && url.password().is_none()
            && url
                .path()
                .strip_prefix("/texture/")
                .is_some_and(|id| !id.is_empty() && !id.contains('/'))
    })
}

impl SkinLibrary {
    pub fn load(paths: &Paths) -> Result<Self, SkinError> {
        let dir = paths.skins_dir();
        fs::create_dir_all(&dir)?;
        let mut index: Index = match fs::read(dir.join("index.json")) {
            Ok(bytes) => serde_json::from_slice(&bytes)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Index::default(),
            Err(e) => return Err(e.into()),
        };
        index.skins.retain(|s| {
            sanitized(&s.name) == s.name
                && fs::read(dir.join(format!("{}.png", s.name)))
                    .is_ok_and(|b| validate_png(&b).is_ok())
        });
        for entry in fs::read_dir(&dir)? {
            let path = entry?.path();
            if path.extension().is_some_and(|e| e == "png") {
                let Some(name) = path.file_stem().and_then(|s| s.to_str()) else {
                    continue;
                };
                if sanitized(name) == name
                    && !index.skins.iter().any(|s| s.name == name)
                    && validate_png(&fs::read(&path)?).is_ok()
                {
                    index.skins.push(Skin {
                        name: name.into(),
                        model: SkinModel::Classic,
                        cape_id: String::new(),
                        url: None,
                        texture_id: None,
                    });
                }
            }
        }
        index.skins.sort_by(|a, b| a.name.cmp(&b.name));
        let library = Self { dir, index };
        library.save()?;
        Ok(library)
    }

    pub fn list(&self) -> &[Skin] {
        &self.index.skins
    }

    pub fn path(&self, name: &str) -> Result<PathBuf, SkinError> {
        self.position(name)?;
        Ok(self.dir.join(format!("{name}.png")))
    }

    fn position(&self, name: &str) -> Result<usize, SkinError> {
        self.index
            .skins
            .iter()
            .position(|s| s.name == name)
            .ok_or_else(|| SkinError::NotFound(name.into()))
    }

    fn unique_name(&self, name: &str) -> String {
        let base = sanitized(name);
        let mut candidate = base.clone();
        let mut n = 0;
        while self.dir.join(format!("{candidate}.png")).exists()
            || self.index.skins.iter().any(|s| s.name == candidate)
        {
            n += 1;
            let suffix = n.to_string();
            let stem: String = base.chars().take(100 - suffix.len()).collect();
            candidate = format!("{stem}{suffix}");
        }
        candidate
    }

    fn save(&self) -> Result<(), SkinError> {
        let mut file = tempfile::NamedTempFile::new_in(&self.dir)?;
        serde_json::to_writer_pretty(&mut file, &self.index)?;
        file.flush()?;
        file.as_file().sync_all()?;
        file.persist(self.dir.join("index.json"))
            .map_err(|e| e.error)?;
        Ok(())
    }

    pub fn import_bytes(
        &mut self,
        name: &str,
        bytes: &[u8],
        model: SkinModel,
        url: Option<String>,
    ) -> Result<Skin, SkinError> {
        validate_png(bytes)?;
        let url = url.filter(|url| is_mojang_texture_url(url));
        let skin = Skin {
            name: self.unique_name(name),
            model,
            cape_id: String::new(),
            texture_id: url
                .as_ref()
                .and_then(|u| u.rsplit('/').next())
                .map(str::to_owned),
            url,
        };
        let path = self.dir.join(format!("{}.png", skin.name));
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        file.write_all(bytes)?;
        self.index.skins.push(skin.clone());
        if let Err(e) = self.save() {
            self.index.skins.pop();
            let _ = fs::remove_file(path);
            return Err(e);
        }
        Ok(skin)
    }

    pub fn import_file(&mut self, path: &Path) -> Result<Skin, SkinError> {
        let name = path.file_stem().and_then(|n| n.to_str()).unwrap_or("Skin");
        self.import_bytes(name, &fs::read(path)?, SkinModel::Classic, None)
    }

    pub async fn import_url(&mut self, name: &str, url: &str) -> Result<Skin, SkinError> {
        let bytes = Self::download_url(url).await?;
        self.import_bytes(name, &bytes, SkinModel::Classic, Some(url.into()))
    }

    pub async fn download_url(url: &str) -> Result<Vec<u8>, SkinError> {
        let http = crate::net::Downloader::new().client();
        let bytes = download_response(&http, url).await?.bytes().await?;
        validate_png(&bytes)?;
        Ok(bytes.to_vec())
    }

    pub async fn import_username(&mut self, username: &str) -> Result<Skin, SkinError> {
        let (bytes, model, url) = Self::download_username(username).await?;
        self.import_bytes(username, &bytes, model, Some(url))
    }

    pub async fn download_username(
        username: &str,
    ) -> Result<(Vec<u8>, SkinModel, String), SkinError> {
        if username.is_empty()
            || username.len() > 16
            || !username
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_')
        {
            return Err(SkinError::InvalidData("Invalid Minecraft username".into()));
        }
        let http = crate::net::Downloader::new().client();
        let profile: serde_json::Value = download_response(
            &http,
            &format!("https://api.minecraftservices.com/minecraft/profile/lookup/name/{username}"),
        )
        .await?
        .json()
        .await?;
        let id = profile["id"]
            .as_str()
            .ok_or_else(|| SkinError::InvalidData("Missing UUID".into()))?;
        let profile: serde_json::Value = download_response(
            &http,
            &format!("https://sessionserver.mojang.com/session/minecraft/profile/{id}"),
        )
        .await?
        .json()
        .await?;
        let encoded = profile["properties"]
            .as_array()
            .and_then(|p| p.iter().find(|p| p["name"] == "textures"))
            .and_then(|p| p["value"].as_str())
            .ok_or_else(|| SkinError::InvalidData("Missing textures".into()))?;
        let bytes = STANDARD
            .decode(encoded)
            .map_err(|e| SkinError::InvalidData(e.to_string()))?;
        let textures: serde_json::Value = serde_json::from_slice(&bytes)?;
        let skin = &textures["textures"]["SKIN"];
        let url = skin["url"]
            .as_str()
            .ok_or_else(|| SkinError::InvalidData("Missing skin URL".into()))?;
        let bytes = download_response(&http, url).await?.bytes().await?;
        let model = if skin["metadata"]["model"] == "slim" {
            SkinModel::Slim
        } else {
            SkinModel::Classic
        };
        validate_png(&bytes)?;
        Ok((bytes.to_vec(), model, url.into()))
    }

    pub fn rename(&mut self, name: &str, new_name: &str) -> Result<String, SkinError> {
        let pos = self.position(name)?;
        if sanitized(new_name) == name {
            return Ok(name.into());
        }
        let new_name = self.unique_name(new_name);
        let old = self.path(name)?;
        let new = self.dir.join(format!("{new_name}.png"));
        fs::rename(&old, &new)?;
        self.index.skins[pos].name = new_name.clone();
        if let Err(e) = self.save() {
            self.index.skins[pos].name = name.into();
            fs::rename(new, old)?;
            return Err(e);
        }
        Ok(new_name)
    }

    pub fn delete(&mut self, name: &str) -> Result<(), SkinError> {
        let pos = self.position(name)?;
        let path = self.path(name)?;
        let skin = self.index.skins.remove(pos);
        if let Err(e) = self.save() {
            self.index.skins.insert(pos, skin);
            return Err(e);
        }
        if let Err(e) = fs::remove_file(path) {
            self.index.skins.insert(pos, skin);
            self.save()?;
            return Err(e.into());
        }
        Ok(())
    }

    pub fn set_model(&mut self, name: &str, model: SkinModel) -> Result<(), SkinError> {
        let pos = self.position(name)?;
        let old = self.index.skins[pos].model;
        self.index.skins[pos].model = model;
        if let Err(e) = self.save() {
            self.index.skins[pos].model = old;
            return Err(e);
        }
        Ok(())
    }

    pub fn set_cape(&mut self, name: &str, cape_id: &str) -> Result<(), SkinError> {
        let pos = self.position(name)?;
        let old = std::mem::replace(&mut self.index.skins[pos].cape_id, cape_id.into());
        if let Err(e) = self.save() {
            self.index.skins[pos].cape_id = old;
            return Err(e);
        }
        Ok(())
    }

    pub fn set_texture(
        &mut self,
        name: &str,
        url: &str,
        texture_id: &str,
    ) -> Result<(), SkinError> {
        let pos = self.position(name)?;
        let old = self.index.skins[pos].clone();
        self.index.skins[pos].url = Some(url.into());
        self.index.skins[pos].texture_id = Some(texture_id.into());
        if let Err(e) = self.save() {
            self.index.skins[pos] = old;
            return Err(e);
        }
        Ok(())
    }
}
