//! CurseForge (Flame) API client: search, project files, download.
//! Mirrors `launcher/modplatform/flame/`.

use super::{install_path, PlatformError, ResourceType};
use crate::loaders::Loader;
use crate::net::{DownloadRequest, Downloader};
use reqwest::header::HeaderValue;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const API_BASE: &str = "https://api.curseforge.com/v1";
pub const API_KEY_ENV: &str = "PRISM_RS_CURSEFORGE_API_KEY";

fn configured_api_key() -> Option<HeaderValue> {
    let key = crate::settings::ApiKey::CurseForge.load()?;
    let mut header = HeaderValue::from_str(&key).ok()?;
    header.set_sensitive(true);
    Some(header)
}

pub(super) fn curseforge_loader(loader: &Loader) -> Option<u8> {
    match loader {
        Loader::Forge { .. } => Some(1),
        Loader::Fabric { .. } => Some(4),
        Loader::Quilt { .. } => Some(5),
        Loader::NeoForge { .. } => Some(6),
        Loader::None => None,
    }
}

fn require_loader(loader: &Loader) -> Result<u8, PlatformError> {
    curseforge_loader(loader).ok_or_else(|| {
        PlatformError::Request("Install a mod loader before browsing CurseForge mods.".into())
    })
}

#[derive(Deserialize)]
struct FlameProject {
    #[serde(rename = "classId")]
    class_id: i64,
}

impl FlameProject {
    fn resource_type(&self) -> ResourceType {
        match self.class_id {
            12 => ResourceType::ResourcePack,
            6552 => ResourceType::ShaderPack,
            _ => ResourceType::Mod,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlameSearchHit {
    pub id: i64,
    pub name: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub slug: String,
    #[serde(default, rename = "downloadCount")]
    pub downloads: u64,
    #[serde(default)]
    pub links: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlameFile {
    pub id: i64,
    #[serde(default)]
    pub displayName: String,
    #[serde(default)]
    pub fileName: String,
    #[serde(default)]
    pub downloadUrl: Option<String>,
    #[serde(default)]
    pub fileLength: i64,
    #[serde(default)]
    pub gameVersions: Vec<String>,
    #[serde(default, rename = "fileDate")]
    pub file_date: String,
    #[serde(default, rename = "releaseType")]
    pub release_type: u8,
    #[serde(default)]
    pub hashes: Vec<FlameHash>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlameHash {
    pub algo: u8,
    pub value: String,
}

impl FlameFile {
    pub(super) fn sha1(&self) -> Option<&str> {
        self.hashes.iter().find(|hash| hash.algo == 1).map(|hash| hash.value.as_str())
    }
}

pub struct FlameApi {
    http: reqwest::Client,
    api_key: Option<HeaderValue>,
    base_url: String,
}

impl FlameApi {
    #[cfg(test)]
    pub(crate) fn clear_test_key(&mut self) {
        self.api_key = None;
    }
    pub fn new() -> Self {
        FlameApi {
            base_url: API_BASE.to_string(),
            http: crate::net::configured_http_builder()
                .user_agent(format!("prism-rs/{}", env!("CARGO_PKG_VERSION")))
                .timeout(std::time::Duration::from_secs(30))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
            api_key: configured_api_key(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_test_endpoint(base_url: String) -> Self {
        Self {
            base_url,
            api_key: Some(HeaderValue::from_static("mock-key")),
            ..Self::new()
        }
    }

    pub(crate) fn has_api_key(&self) -> bool {
        self.api_key.is_some()
    }

    pub fn is_configured() -> bool {
        configured_api_key().is_some()
    }

    fn get(&self, path: &str) -> Result<reqwest::RequestBuilder, PlatformError> {
        self.request(reqwest::Method::GET, path)
    }

    fn request(&self, method: reqwest::Method, path: &str) -> Result<reqwest::RequestBuilder, PlatformError> {
        let key = self.api_key.as_ref().ok_or_else(|| {
            PlatformError::Request(format!("set a CurseForge API key in Settings → API Keys or set {API_KEY_ENV}"))
        })?;
        Ok(self.http
            .request(method, format!("{}{path}", self.base_url))
            .header("x-api-key", key)
            .header("Accept", "application/json"))
    }

    pub(crate) async fn match_fingerprints(
        &self,
        fingerprints: &[u32],
    ) -> Result<BTreeMap<u32, FlameManifestFile>, PlatformError> {
        #[derive(Deserialize)]
        struct File {
            #[serde(rename = "fileFingerprint")]
            fingerprint: u32,
            #[serde(rename = "modId")]
            project: i64,
            id: i64,
            #[serde(rename = "isAvailable")]
            available: bool,
        }
        #[derive(Deserialize)]
        struct Match {
            file: File,
        }
        #[derive(Deserialize)]
        struct Data {
            #[serde(rename = "exactMatches")]
            exact_matches: Vec<Match>,
        }
        #[derive(Deserialize)]
        struct Response {
            data: Data,
        }
        let response = self
            .request(reqwest::Method::POST, "/fingerprints")?
            .json(&serde_json::json!({ "fingerprints": fingerprints }))
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(Self::lookup_error(response.status(), "CurseForge fingerprint lookup".into()));
        }
        let mut files = BTreeMap::new();
        for Match { file } in response.json::<Response>().await?.data.exact_matches {
            if file.available && file.project > 0 && file.id > 0 && fingerprints.contains(&file.fingerprint) {
                files.entry(file.fingerprint).or_insert(FlameManifestFile {
                    projectID: file.project,
                    fileID: file.id,
                    required: true,
                });
            }
        }
        Ok(files)
    }

    pub async fn search_mods(
        &self,
        query: &str,
        game_version: &str,
        loader: &Loader,
    ) -> Result<Vec<FlameSearchHit>, PlatformError> {
        #[derive(Deserialize)]
        struct Response {
            data: Vec<FlameSearchHit>,
        }

        let loader = require_loader(loader)?;
        let mut request = self.get("/mods/search")?
            .query(&[("gameId", 432), ("classId", 6), ("sortField", 2), ("pageSize", 25)])
            .query(&[("sortOrder", "desc"), ("gameVersion", game_version)])
            .query(&[("modLoaderType", loader)]);
        if !query.trim().is_empty() {
            request = request.query(&[("searchFilter", query.trim())]);
        }
        let resp = request.send().await?;
        if !resp.status().is_success() {
            return Err(Self::lookup_error(resp.status(), "CurseForge search".into()));
        }
        Ok(resp.json::<Response>().await?.data)
    }

    /// List files of a project.
    pub async fn files(&self, project_id: i64) -> Result<Vec<FlameFile>, PlatformError> {
        let resp = self.get(&format!("/mods/{project_id}/files"))?.send().await?;
        if !resp.status().is_success() {
            return Err(PlatformError::Request(format!("flame files: {}", resp.status())));
        }
        let json: serde_json::Value = resp.json().await?;
        let files: Vec<FlameFile> = serde_json::from_value(json.get("data").cloned().unwrap_or_default())?;
        Ok(files)
    }

    pub async fn file(&self, project_id: i64, file_id: i64) -> Result<FlameFile, PlatformError> {
        #[derive(Deserialize)]
        struct Response {
            data: FlameFile,
        }

        let resp = self
            .get(&format!("/mods/{project_id}/files/{file_id}"))?
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(Self::lookup_error(
                resp.status(),
                format!("CurseForge project {project_id}, file {file_id}"),
            ));
        }
        Ok(resp.json::<Response>().await?.data)
    }

    pub(super) async fn compatible_files(
        &self,
        project_id: i64,
        game_version: &str,
        loader: u8,
    ) -> Result<Vec<FlameFile>, PlatformError> {
        #[derive(Deserialize)]
        struct Response {
            data: Vec<FlameFile>,
            pagination: Pagination,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Pagination {
            index: usize,
            total_count: usize,
        }

        let mut files = Vec::new();
        let path = format!("/mods/{project_id}/files");
        loop {
            let index = files.len();
            let resp = self
                .get(&path)?
                .query(&[("gameVersion", game_version)])
                .query(&[("modLoaderType", loader)])
                .query(&[("pageSize", 50), ("index", index)])
                .send()
                .await?;
            if !resp.status().is_success() {
                return Err(Self::lookup_error(resp.status(), format!("CurseForge project {project_id}")));
            }
            let page: Response = resp.json().await?;
            if page.pagination.index != index || (page.data.is_empty() && index < page.pagination.total_count) {
                return Err(PlatformError::Request(format!(
                    "incomplete CurseForge file listing for project {project_id} at index {index}"
                )));
            }
            files.extend(page.data);
            if files.len() >= page.pagination.total_count {
                return Ok(files);
            }
        }
    }

    pub async fn project_resource_type(&self, project_id: i64) -> Result<ResourceType, PlatformError> {
        #[derive(Deserialize)]
        struct Response {
            data: FlameProject,
        }

        let resp = self.get(&format!("/mods/{project_id}"))?.send().await?;
        if !resp.status().is_success() {
            return Err(Self::lookup_error(resp.status(), format!("CurseForge project {project_id}")));
        }
        Ok(resp.json::<Response>().await?.data.resource_type())
    }

    fn lookup_error(status: reqwest::StatusCode, context: String) -> PlatformError {
        match status {
            reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN => {
                PlatformError::CurseForgeApiKeyRejected(status)
            }
            _ => PlatformError::Request(format!("{context}: {status}")),
        }
    }

    pub async fn install_mod(
        &self,
        downloader: &Downloader,
        game_root: &Path,
        project_id: i64,
        game_version: &str,
        loader: &Loader,
    ) -> Result<PathBuf, PlatformError> {
        let loader = require_loader(loader)?;
        let files = self.compatible_files(project_id, game_version, loader).await?;
        let (file, url, sha1) = files.iter().filter_map(|file| {
            let url = file.downloadUrl.as_deref().filter(|url| !url.is_empty())?;
            let sha1 = file.sha1().filter(|hash|
                hash.len() == 40 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
            )?;
            if !(1..=3).contains(&file.release_type) || !super::updates::is_jar(&file.fileName)
                || !file.gameVersions.iter().any(|version| version == game_version)
            {
                return None;
            }
            let date = chrono::DateTime::parse_from_rfc3339(&file.file_date).ok()?;
            Some(((std::cmp::Reverse(file.release_type), date, file.id), (file, url, sha1)))
        })
        .max_by_key(|(rank, _)| *rank)
        .map(|(_, candidate)| candidate)
        .ok_or_else(|| PlatformError::Request(
            format!("No compatible, downloadable JAR for Minecraft {game_version} with a checksum. Check the project on CurseForge.")
        ))?;

        let dest = install_path(game_root, ResourceType::Mod, &file.fileName);
        let disabled = dest.with_file_name(format!("{}.disabled", file.fileName));
        let ensure_unoccupied = || -> Result<(), PlatformError> {
            for path in [&dest, &disabled] {
                match path.symlink_metadata() {
                    Ok(_) => return Err(PlatformError::Request(format!(
                        "{} is already in the mods folder", path.file_name().unwrap().to_string_lossy()
                    ))),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.into()),
                }
            }
            Ok(())
        };
        ensure_unoccupied()?;
        let mods = game_root.join(ResourceType::Mod.folder());
        std::fs::create_dir_all(&mods)?;
        let stage = tempfile::tempdir_in(&mods)?;
        let req = DownloadRequest::new(url, stage.path().join("mod.jar")).with_sha1(sha1);
        downloader.download(&req).await?;
        ensure_unoccupied()?;
        tempfile::TempPath::try_from_path(req.dest)?.persist_noclobber(&dest).map_err(|e| e.error)?;
        if let Err(error) = super::updates::write_curseforge_index(game_root, project_id, file) {
            if let Err(rollback) = std::fs::remove_file(&dest) {
                return Err(PlatformError::Request(format!(
                    "Could not save mod metadata: {error}; could not remove {}: {rollback}", file.fileName
                )));
            }
            return Err(error);
        }
        Ok(dest)
    }

    /// Download a file into the instance's resource folder.
    pub async fn install_file(
        &self,
        downloader: &Arc<Downloader>,
        game_root: &Path,
        file: &FlameFile,
        resource_type: ResourceType,
    ) -> Result<std::path::PathBuf, PlatformError> {
        let url = file.downloadUrl.clone().ok_or_else(|| {
            PlatformError::Request(format!(
                "file {} has no download URL (may need manual download)",
                file.id
            ))
        })?;
        if file.fileName.is_empty()
            || file.fileName == "."
            || file.fileName == ".."
            || file.fileName.contains(['/', '\\', ':'])
        {
            return Err(PlatformError::Request(format!(
                "CurseForge file {} has an invalid filename",
                file.id
            )));
        }
        let dest = install_path(game_root, resource_type, &file.fileName);
        let mut req = DownloadRequest::new(url, &dest);
        if let Some(hash) = file.sha1() {
            req = req.with_sha1(hash);
        }
        downloader.download(&req).await?;
        Ok(dest)
    }
}

impl Default for FlameApi {
    fn default() -> Self {
        Self::new()
    }
}

/// CurseForge modpack `manifest.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlameManifest {
    #[serde(default)]
    pub minecraft: serde_json::Value,
    #[serde(default)]
    pub manifestType: String,
    #[serde(default)]
    pub files: Vec<FlameManifestFile>,
    #[serde(default = "default_overrides")]
    pub overrides: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub version: String,
}

fn default_overrides() -> String {
    "overrides".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlameManifestFile {
    pub projectID: i64,
    pub fileID: i64,
    #[serde(default)]
    pub required: bool,
}

impl FlameManifest {
    pub fn from_dir(dir: &Path) -> Result<FlameManifest, PlatformError> {
        let text = std::fs::read_to_string(dir.join("manifest.json"))?;
        Ok(serde_json::from_str(&text)?)
    }

    pub fn minecraft_version(&self) -> Option<String> {
        self.minecraft["version"].as_str().map(String::from)
    }

    pub fn loader_version(&self) -> Option<(String, String)> {
        let loaders = self.minecraft["modLoaders"].as_array()?;
        let loader = loaders
            .iter()
            .find(|loader| loader["primary"].as_bool() == Some(true))
            .or_else(|| loaders.first())?;
        let id = loader["id"].as_str()?;
        let (kind, version) = id.split_once('-')?;
        let version = if kind == "neoforge" {
            version.strip_prefix("1.20.1-").unwrap_or(version)
        } else {
            version
        };
        if version.is_empty() {
            return None;
        }
        Some((kind.to_string(), version.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, Write};
    use std::net::TcpListener;

    fn reply(listener: &TcpListener, expected: &[&str], status: &str, body: &[u8]) {
        let (mut socket, _) = listener.accept().unwrap();
        let mut reader = std::io::BufReader::new(&mut socket);
        let mut request = String::new();
        loop {
            let size = reader.read_line(&mut request).unwrap();
            if size <= 2 {
                break;
            }
        }
        let line = request.lines().next().unwrap();
        for part in expected {
            assert!(line.contains(part), "missing {part} in {line}");
        }
        assert_eq!(request.contains("x-api-key: mock-key"), !line.starts_with("GET /download "));
        write!(socket, "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).unwrap();
        socket.write_all(body).unwrap();
    }

    #[tokio::test]
    async fn searches_compatible_curseforge_mods_and_installs_the_latest_stable_jar() {
        use sha1::{Digest, Sha1};

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let api = FlameApi::with_test_endpoint(base.clone());
        let bytes = b"verified mod jar";
        let hash = hex::encode(Sha1::digest(bytes));
        let file = |id, name, release, game| serde_json::json!({
            "id": id,
            "fileName": name,
            "displayName": "Example Mod",
            "fileDate": format!("2026-01-0{id}T00:00:00Z"),
            "releaseType": release,
            "gameVersions": [game],
            "downloadUrl": format!("{base}/download"),
            "hashes": [{"algo": 1, "value": hash}],
        });
        let files = serde_json::json!({
            "data": [
                file(5, "wrong-game.jar", 1, "1.21"),
                file(4, "../unsafe.jar", 1, "1.20.1"),
                file(3, "beta.jar", 2, "1.20.1"),
                file(2, "stable.jar", 1, "1.20.1"),
                file(1, "older.jar", 1, "1.20.1")
            ],
            "pagination": {"index": 0, "totalCount": 5}
        }).to_string();
        let worker = std::thread::spawn(move || {
            reply(&listener, &["GET /mods/search?", "classId=6", "gameId=432", "searchFilter=Example+Mod", "gameVersion=1.20.1", "modLoaderType=4"],
                "200 OK", br#"{"data":[{"id":42,"name":"Example Mod","downloadCount":3}]}"#);
            let expected_files = ["GET /mods/42/files?", "gameVersion=1.20.1", "modLoaderType=4"];
            reply(&listener, &expected_files, "200 OK", files.as_bytes());
            reply(&listener, &["GET /download "], "200 OK", bytes);
            reply(&listener, &expected_files, "200 OK", files.as_bytes());
            reply(&listener, &expected_files, "200 OK", files.as_bytes());
            reply(&listener, &["GET /download "], "200 OK", bytes);
        });

        let loader = Loader::Fabric { version: "0.16.14".into() };
        let hits = api.search_mods("Example Mod", "1.20.1", &loader).await.unwrap();
        assert_eq!((hits[0].id, hits[0].downloads), (42, 3));
        let temp = tempfile::tempdir().unwrap();
        let path = api.install_mod(&Downloader::new(), temp.path(), 42, "1.20.1", &loader).await.unwrap();
        assert_eq!(path, temp.path().join("mods/stable.jar"));
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        let index = std::fs::read_to_string(
            std::fs::read_dir(temp.path().join("mods/.index")).unwrap().next().unwrap().unwrap().path()
        ).unwrap();
        let index: toml::Value = toml::from_str(&index).unwrap();
        assert_eq!(index["update"]["curseforge"]["project-id"].as_integer(), Some(42));
        assert_eq!(index["update"]["curseforge"]["file-id"].as_integer(), Some(2));

        let disabled = path.with_file_name("stable.jar.disabled");
        std::fs::rename(&path, &disabled).unwrap();
        let error = api.install_mod(&Downloader::new(), temp.path(), 42, "1.20.1", &loader).await.unwrap_err();
        assert!(error.to_string().contains("stable.jar.disabled is already in the mods folder"));
        assert!(!path.exists());
        assert_eq!(std::fs::read(disabled).unwrap(), bytes);

        let blocked = tempfile::tempdir().unwrap();
        std::fs::create_dir(blocked.path().join("mods")).unwrap();
        std::fs::write(blocked.path().join("mods/.index"), b"not a directory").unwrap();
        assert!(api.install_mod(&Downloader::new(), blocked.path(), 42, "1.20.1", &loader).await.is_err());
        assert!(!blocked.path().join("mods/stable.jar").exists());
        worker.join().unwrap();
    }

    #[tokio::test]
    async fn reports_a_rejected_search_key() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let api = FlameApi::with_test_endpoint(format!("http://{}", listener.local_addr().unwrap()));
        let worker = std::thread::spawn(move || {
            reply(&listener, &["GET /mods/search?"], "403 Forbidden", b"");
        });
        let loader = Loader::Forge { version: "47.4.0".into() };
        assert!(matches!(api.search_mods("", "1.20.1", &loader).await,
            Err(PlatformError::CurseForgeApiKeyRejected(_))));
        worker.join().unwrap();
    }

    #[test]
    fn project_classes_route_files_to_the_right_game_folders() {
        for (class_id, folder) in [(6, "mods"), (12, "resourcepacks"), (6552, "shaderpacks")] {
            let project: FlameProject =
                serde_json::from_value(serde_json::json!({ "classId": class_id })).unwrap();
            assert_eq!(project.resource_type().folder(), folder);
        }
    }

    #[test]
    fn parses_manifest() {
        let json = serde_json::json!({
            "minecraft": {
                "version": "1.20.1",
                "modLoaders": [
                    {"id": "forge-47.2.0", "primary": false},
                    {"id": "fabric-0.15.0", "primary": true}
                ]
            },
            "manifestType": "minecraftModpack",
            "name": "Test",
            "files": [{"projectID": 1, "fileID": 2}]
        });
        let m: FlameManifest = serde_json::from_value(json).unwrap();
        assert_eq!(m.minecraft_version().as_deref(), Some("1.20.1"));
        assert_eq!(m.loader_version().map(|(k, _)| k), Some("fabric".to_string()));
        assert_eq!(m.loader_version(), Some(("fabric".into(), "0.15.0".into())));
        assert_eq!(m.overrides, "overrides");
        assert_eq!(m.files.len(), 1);
        assert_eq!((m.files[0].projectID, m.files[0].fileID), (1, 2));
    }

    #[test]
    fn parses_legacy_neoforge_loader() {
        let manifest: FlameManifest = serde_json::from_value(serde_json::json!({
            "minecraft": {"modLoaders": [{"id": "neoforge-1.20.1-47.1.106"}]}
        })).unwrap();
        assert_eq!(manifest.loader_version(), Some(("neoforge".into(), "47.1.106".into())));
    }
}
