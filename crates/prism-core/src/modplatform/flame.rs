//! CurseForge (Flame) API client: search, project files, download.
//! Mirrors `launcher/modplatform/flame/`.

use super::{install_path, PlatformError, ResourceType};
use crate::net::{DownloadRequest, Downloader};
use reqwest::header::HeaderValue;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;

pub const API_BASE: &str = "https://api.curseforge.com/v1";
pub const API_KEY_ENV: &str = "PRISM_RS_CURSEFORGE_API_KEY";

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
    #[serde(default)]
    pub downloadCount: i64,
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
    pub fn new() -> Self {
        FlameApi {
            base_url: API_BASE.to_string(),
            http: reqwest::Client::builder()
                .user_agent(format!("prism-rs/{}", env!("CARGO_PKG_VERSION")))
                .timeout(std::time::Duration::from_secs(30))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
            api_key: std::env::var(API_KEY_ENV).ok().and_then(|key| {
                let key = key.trim();
                if key.is_empty() {
                    return None;
                }
                let mut header = HeaderValue::from_str(key).ok()?;
                header.set_sensitive(true);
                Some(header)
            }),
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

    fn get(&self, path: &str) -> Result<reqwest::RequestBuilder, PlatformError> {
        let key = self.api_key.as_ref().ok_or_else(|| {
            PlatformError::Request(format!("set {API_KEY_ENV} to a valid CurseForge API key"))
        })?;
        Ok(self.http
            .get(format!("{}{path}", self.base_url))
            .header("x-api-key", key)
            .header("Accept", "application/json"))
    }

    /// Search mods in the classId=6 (mods) section.
    pub async fn search(
        &self,
        query: &str,
        game_version: &str,
        class_id: i64,
        limit: u32,
    ) -> Result<Vec<FlameSearchHit>, PlatformError> {
        let path = format!(
            "/mods/search?gameId=432&classId={class_id}&sortField=2&sortOrder=desc&pageSize={limit}"
        );
        let resp = self.get(&path)?
            .query(&[("searchFilter", query), ("gameVersion", game_version)])
            .send().await?;
        if !resp.status().is_success() {
            return Err(PlatformError::Request(format!("flame search: {}", resp.status())));
        }
        let json: serde_json::Value = resp.json().await?;
        let hits: Vec<FlameSearchHit> = serde_json::from_value(json.get("data").cloned().unwrap_or_default())?;
        Ok(hits)
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
