//! Modrinth API v2 client: search, project versions, download/install, and
//! `.mrpack` modpack import. Mirrors `launcher/modplatform/modrinth/`.

use super::{install_path, PlatformError, ResourceType};
use crate::net::{DownloadRequest, Downloader};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;

pub const API_BASE: &str = "https://api.modrinth.com/v2";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub slug: String,
    pub title: String,
    pub description: String,
    pub project_id: String,
    pub project_type: String,
    #[serde(default)]
    pub icon_url: Option<String>,
    #[serde(default)]
    pub downloads: u64,
    #[serde(default)]
    pub versions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResponse {
    pub hits: Vec<SearchResult>,
    pub total_hits: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModVersion {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub version_number: String,
    #[serde(default)]
    pub version_type: String,
    #[serde(default)]
    pub loaders: Vec<String>,
    #[serde(default)]
    pub game_versions: Vec<String>,
    #[serde(default)]
    pub date_published: String,
    pub files: Vec<ModFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModFile {
    pub url: String,
    pub filename: String,
    pub primary: bool,
    pub size: u64,
    #[serde(default)]
    pub hashes: std::collections::BTreeMap<String, String>,
}

pub struct ModrinthApi {
    http: reqwest::Client,
}

impl ModrinthApi {
    pub fn new() -> Self {
        ModrinthApi {
            http: reqwest::Client::builder()
                .user_agent(format!("prism-rs/{}", env!("CARGO_PKG_VERSION")))
                .build()
                .unwrap(),
        }
    }

    pub fn with_client(http: reqwest::Client) -> Self {
        ModrinthApi { http }
    }

    /// Search projects of one resource type.
    pub async fn search(
        &self,
        query: &str,
        project_type: ResourceType,
        game_version: Option<&str>,
        limit: u32,
    ) -> Result<SearchResponse, PlatformError> {
        let mut facets = vec![format!("[\"project_type:{}\"]", project_type.modrinth_project_type())];
        if let Some(gv) = game_version {
            facets.push(format!("[\"versions:{gv}\"]"));
        }
        let url = format!(
            "{API_BASE}/search?query={query}&facets=[{}]&limit={limit}",
            urlencoding_encode(&facets.join(","))
        );
        let resp = self.http.get(&url).send().await.map_err(|e| PlatformError::Request(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(PlatformError::Request(format!("search: {}", resp.status())));
        }
        Ok(resp.json().await?)
    }

    /// List versions of a project, optionally filtered.
    pub async fn versions(
        &self,
        project_id_or_slug: &str,
        loaders: &[&str],
        game_versions: &[&str],
    ) -> Result<Vec<ModVersion>, PlatformError> {
        let mut url = format!("{API_BASE}/project/{project_id_or_slug}/version");
        let mut params = Vec::new();
        if !loaders.is_empty() {
            let inner: Vec<String> = loaders.iter().map(|l| format!("\"{l}\"")).collect();
            params.push(format!("loaders=[{}]", inner.join(",")));
        }
        if !game_versions.is_empty() {
            let inner: Vec<String> = game_versions.iter().map(|g| format!("\"{g}\"")).collect();
            params.push(format!("game_versions=[{}]", inner.join(",")));
        }
        if !params.is_empty() {
            url.push('?');
            url.push_str(&params.join("&"));
        }
        let resp = self.http.get(&url).send().await.map_err(|e| PlatformError::Request(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(PlatformError::Request(format!("versions: {}", resp.status())));
        }
        Ok(resp.json().await?)
    }

    /// Pick the primary file of a version.
    pub fn primary_file<'a>(&self, version: &'a ModVersion) -> Option<&'a ModFile> {
        version.files.iter().find(|f| f.primary).or_else(|| version.files.first())
    }

    /// Download and install one file into the instance.
    pub async fn install_file(
        &self,
        downloader: &Arc<Downloader>,
        game_root: &Path,
        file: &ModFile,
        resource_type: ResourceType,
    ) -> Result<std::path::PathBuf, PlatformError> {
        let dest = install_path(game_root, resource_type, &file.filename);
        let mut req = DownloadRequest::new(file.url.clone(), &dest);
        if let Some(sha1) = file.hashes.get("sha1") {
            req = req.with_sha1(sha1.clone());
        }
        downloader.download(&req).await?;
        Ok(dest)
    }
}

impl Default for ModrinthApi {
    fn default() -> Self {
        Self::new()
    }
}

// Minimal url encoding for the characters we generate ourselves.
fn urlencoding_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b':' | b'"' | b',' => {
                out.push(b as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// .mrpack (Modrinth modpack) import
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MrpackIndex {
    #[serde(default)]
    pub format_version: u32,
    #[serde(default)]
    pub game: String,
    #[serde(default)]
    pub version_id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub files: Vec<MrpackFile>,
    #[serde(default)]
    pub dependencies: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MrpackFile {
    pub path: String,
    #[serde(default)]
    pub hashes: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub env: Option<std::collections::BTreeMap<String, String>>,
    pub downloads: Vec<String>,
    #[serde(default)]
    pub file_size: u64,
}

impl MrpackIndex {
    pub fn parse(json: &serde_json::Value) -> Result<MrpackIndex, PlatformError> {
        Ok(serde_json::from_value(json.clone())?)
    }

    /// Read an index from an extracted .mrpack directory.
    pub fn from_dir(dir: &Path) -> Result<MrpackIndex, PlatformError> {
        let text = std::fs::read_to_string(dir.join("modrinth.index.json"))?;
        Ok(serde_json::from_str(&text)?)
    }

    /// The Minecraft version this pack targets.
    pub fn minecraft_version(&self) -> Option<&str> {
        self.dependencies.get("minecraft").map(|s| s.as_str())
    }

    /// The loader dependency, e.g. ("fabric-loader", "0.15.0").
    pub fn loader(&self) -> Option<(String, String)> {
        self.dependencies
            .iter()
            .find(|(k, _)| k.as_str() != "minecraft")
            .map(|(k, v)| (k.clone(), v.clone()))
    }

    /// Download all pack files into an instance's game root.
    pub async fn install_files(
        &self,
        downloader: &Arc<Downloader>,
        game_root: &Path,
    ) -> Result<(), PlatformError> {
        let mut requests = Vec::new();
        for file in &self.files {
            let url = file
                .downloads
                .first()
                .ok_or_else(|| PlatformError::Request(format!("no download for {}", file.path)))?;
            let dest = game_root.join(&file.path);
            let mut req = DownloadRequest::new(url.clone(), dest);
            if let Some(sha1) = file.hashes.get("sha1") {
                req = req.with_sha1(sha1.clone());
            }
            requests.push(req);
        }
        downloader
            .download_all(requests, 8, |_| {})
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_mrpack_index() {
        let json = serde_json::json!({
            "formatVersion": 1,
            "game": "minecraft",
            "versionId": "1.0",
            "name": "Test Pack",
            "files": [
                {
                    "path": "mods/sodium.jar",
                    "hashes": {"sha1": "abc", "sha512": "def"},
                    "downloads": ["https://cdn.modrinth.com/sodium.jar"],
                    "fileSize": 1234
                }
            ],
            "dependencies": {"minecraft": "1.20.1", "fabric-loader": "0.15.0"}
        });
        let index = MrpackIndex::parse(&json).unwrap();
        assert_eq!(index.name, "Test Pack");
        assert_eq!(index.minecraft_version(), Some("1.20.1"));
        assert_eq!(index.loader().map(|(k, _)| k), Some("fabric-loader".to_string()));
        assert_eq!(index.files.len(), 1);
    }
}
