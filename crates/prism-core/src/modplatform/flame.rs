//! CurseForge (Flame) API client: search, project files, download.
//! Mirrors `launcher/modplatform/flame/`.

use super::{install_path, PlatformError, ResourceType};
use crate::net::{DownloadRequest, Downloader};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;

/// Prism's public CurseForge API key (embedded in the original launcher).
pub const FLAME_API_KEY: &str =
    "$2a$10$wuAJuNZuted3NORVmpgUC.m8sI.pv1tOPKZyBgLFGjxFp/br0lZCC";

pub const API_BASE: &str = "https://api.curseforge.com/v1";

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
}

pub struct FlameApi {
    http: reqwest::Client,
}

impl FlameApi {
    pub fn new() -> Self {
        FlameApi {
            http: reqwest::Client::builder()
                .user_agent(format!("prism-rs/{}", env!("CARGO_PKG_VERSION")))
                .build()
                .unwrap(),
        }
    }

    fn get(&self, path: &str) -> reqwest::RequestBuilder {
        self.http
            .get(format!("{API_BASE}{path}"))
            .header("x-api-key", FLAME_API_KEY)
            .header("Accept", "application/json")
    }

    /// Search mods in the classId=6 (mods) section.
    pub async fn search(
        &self,
        query: &str,
        game_version: &str,
        class_id: i64,
        limit: u32,
    ) -> Result<Vec<FlameSearchHit>, PlatformError> {
        let url = format!(
            "{API_BASE}/mods/search?gameId=432&searchFilter={query}&classId={class_id}&gameVersion={game_version}&sortField=2&sortOrder=desc&pageSize={limit}"
        );
        let resp = self.http.get(&url).header("x-api-key", FLAME_API_KEY).send().await?;
        if !resp.status().is_success() {
            return Err(PlatformError::Request(format!("flame search: {}", resp.status())));
        }
        let json: serde_json::Value = resp.json().await?;
        let hits: Vec<FlameSearchHit> = serde_json::from_value(json.get("data").cloned().unwrap_or_default())?;
        Ok(hits)
    }

    /// List files of a project.
    pub async fn files(&self, project_id: i64) -> Result<Vec<FlameFile>, PlatformError> {
        let resp = self.get(&format!("/mods/{project_id}/files")).send().await?;
        if !resp.status().is_success() {
            return Err(PlatformError::Request(format!("flame files: {}", resp.status())));
        }
        let json: serde_json::Value = resp.json().await?;
        let files: Vec<FlameFile> = serde_json::from_value(json.get("data").cloned().unwrap_or_default())?;
        Ok(files)
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
        let dest = install_path(game_root, resource_type, &file.fileName);
        let req = DownloadRequest::new(url, &dest);
        downloader.download(&req).await?;
        Ok(dest)
    }
}

impl Default for FlameApi {
    fn default() -> Self {
        Self::new()
    }
}

/// CurseForge `manifest.json` (pack export format).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlameManifest {
    #[serde(default)]
    pub minecraft: serde_json::Value,
    #[serde(default)]
    pub manifestType: String,
    #[serde(default)]
    pub files: Vec<FlameManifestFile>,
    #[serde(default)]
    pub overrides: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub version: String,
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
        let first = loaders.first()?;
        let id = first["id"].as_str()?;
        // e.g. "fabric-0.15.0" or "forge-47.2.0"
        let (kind, version) = id.split_once('-')?;
        Some((kind.to_string(), version.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_manifest() {
        let json = serde_json::json!({
            "minecraft": {
                "version": "1.20.1",
                "modLoaders": [{"id": "fabric-0.15.0", "primary": true}]
            },
            "manifestType": "minecraftModpack",
            "name": "Test",
            "files": [{"projectID": 1, "fileID": 2}]
        });
        let m: FlameManifest = serde_json::from_value(json).unwrap();
        assert_eq!(m.minecraft_version().as_deref(), Some("1.20.1"));
        assert_eq!(m.loader_version().map(|(k, _)| k), Some("fabric".to_string()));
        assert_eq!(m.files.len(), 1);
    }
}
