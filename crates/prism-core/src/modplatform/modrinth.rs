//! Modrinth API v2 client: search, project versions, download/install, and
//! `.mrpack` modpack import. Mirrors `launcher/modplatform/modrinth/`.

use super::{install_path, PlatformError, ResourceType};
use crate::datapacks::World;
use crate::loaders::Loader;
use crate::net::{DownloadRequest, Downloader};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
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
    pub categories: Vec<String>,
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

pub struct StagedDataPack {
    _directory: tempfile::TempDir,
    path: PathBuf,
}

impl StagedDataPack {
    pub fn install(self, world: &World) -> std::io::Result<()> {
        world.add_zip(&self.path)
    }
}

impl ModrinthApi {
    pub async fn version_files(
        &self,
        hashes: &[String],
    ) -> Result<std::collections::BTreeMap<String, ModVersion>, PlatformError> {
        Ok(self
            .http
            .post(format!("{API_BASE}/version_files"))
            .json(&serde_json::json!({"hashes": hashes, "algorithm": "sha1"}))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }

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
        let mut facets = vec![[format!("project_type:{}", project_type.modrinth_project_type())]];
        if let Some(gv) = game_version {
            facets.push([format!("versions:{gv}")]);
        }
        let facets = serde_json::to_string(&facets)?;
        Ok(self
            .http
            .get(format!("{API_BASE}/search"))
            .query(&[("query", query), ("facets", facets.as_str())])
            .query(&[("limit", limit)])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }

    /// List versions of a project, optionally filtered.
    pub async fn versions(
        &self,
        project_id_or_slug: &str,
        loaders: &[&str],
        game_versions: &[&str],
    ) -> Result<Vec<ModVersion>, PlatformError> {
        let mut url = reqwest::Url::parse(API_BASE).expect("valid Modrinth base URL");
        url.path_segments_mut()
            .expect("Modrinth base URL has a path")
            .extend(["project", project_id_or_slug, "version"]);
        let mut request = self.http.get(url);
        if !loaders.is_empty() {
            request = request.query(&[("loaders", serde_json::to_string(loaders)?)]);
        }
        if !game_versions.is_empty() {
            request = request.query(&[("game_versions", serde_json::to_string(game_versions)?)]);
        }
        Ok(request.send().await?.error_for_status()?.json().await?)
    }

    pub(super) async fn version(&self, id: &str) -> Result<ModVersion, PlatformError> {
        let mut url = reqwest::Url::parse(API_BASE).expect("valid Modrinth base URL");
        url.path_segments_mut()
            .expect("Modrinth base URL has a path")
            .extend(["version", id]);
        Ok(self.http.get(url).send().await?.error_for_status()?.json().await?)
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
        if resource_type == ResourceType::DataPack {
            return Err(PlatformError::Request("data packs require a world".into()));
        }
        Ok(downloader
            .download(&install_request(game_root, file, resource_type)?)
            .await?)
    }

    /// Install the first compatible version returned by Modrinth.
    pub async fn install_compatible(
        &self,
        downloader: &Arc<Downloader>,
        game_root: &Path,
        project: &SearchResult,
        resource_type: ResourceType,
        mc_version: &str,
        loader: &Loader,
    ) -> Result<std::path::PathBuf, PlatformError> {
        if resource_type == ResourceType::DataPack {
            return Err(PlatformError::Request("data packs require a world".into()));
        }
        if project.project_type != resource_type.modrinth_project_type() {
            return Err(PlatformError::Request("wrong project type".into()));
        }
        let loaders: Vec<_> = modrinth_loader(resource_type, loader).into_iter().collect();
        let versions = self.versions(&project.project_id, &loaders, &[mc_version]).await?;
        let version = versions.first().ok_or_else(|| {
            PlatformError::Request(format!("no compatible version for Minecraft {mc_version}"))
        })?;
        let file = self.primary_file(version)
            .ok_or_else(|| PlatformError::Request("version has no downloadable file".into()))?;
        let path = self.install_file(downloader, game_root, file, resource_type).await?;
        if resource_type == ResourceType::Mod {
            super::updates::record_modrinth(game_root, &project.title, version, file);
        }
        Ok(path)
    }

    pub async fn download_datapack(
        &self,
        downloader: &Arc<Downloader>,
        project: &SearchResult,
        mc_version: &str,
    ) -> Result<StagedDataPack, PlatformError> {
        if !is_datapack_project(project) {
            return Err(PlatformError::Request("wrong project type".into()));
        }
        let versions = self.versions(&project.project_id, &["datapack"], &[mc_version]).await?;
        let file = datapack_file(&versions, mc_version).ok_or_else(|| {
            PlatformError::Request(format!("no compatible data pack ZIP for Minecraft {mc_version}"))
        })?;
        let directory = tempfile::tempdir()?;
        let request = install_request(directory.path(), file, ResourceType::DataPack)?;
        let path = downloader.download(&request).await?;
        Ok(StagedDataPack { _directory: directory, path })
    }
}

fn is_datapack_project(project: &SearchResult) -> bool {
    project.project_type == "datapack"
        || (project.project_type == "mod" && project.categories.iter().any(|category| category == "datapack"))
}

fn datapack_file<'a>(versions: &'a [ModVersion], mc_version: &str) -> Option<&'a ModFile> {
    versions.iter()
        .filter(|version| {
            version.loaders.iter().any(|loader| loader == "datapack")
                && version.game_versions.iter().any(|version| version == mc_version)
        })
        .find_map(|version| {
            let mut zips = version.files.iter().filter(|file| file.filename.ends_with(".zip"));
            zips.clone().find(|file| file.primary).or_else(|| zips.next())
        })
}

impl Default for ModrinthApi {
    fn default() -> Self {
        Self::new()
    }
}

pub(super) fn modrinth_loader(resource_type: ResourceType, loader: &Loader) -> Option<&'static str> {
    if resource_type != ResourceType::Mod {
        return None;
    }
    match loader {
        Loader::Fabric { .. } => Some("fabric"),
        Loader::Quilt { .. } => Some("quilt"),
        Loader::Forge { .. } => Some("forge"),
        Loader::NeoForge { .. } => Some("neoforge"),
        Loader::None => None,
    }
}

fn install_request(
    game_root: &Path,
    file: &ModFile,
    resource_type: ResourceType,
) -> Result<DownloadRequest, PlatformError> {
    let name = file.filename.as_str();
    if Path::new(name).file_name().and_then(|n| n.to_str()) != Some(name)
        || name.contains(['/', '\\', ':'])
        || name.chars().any(char::is_control)
    {
        return Err(PlatformError::Request("invalid download filename".into()));
    }
    let dest = install_path(game_root, resource_type, name);
    let mut request = DownloadRequest::new(file.url.clone(), dest);
    if let Some(sha1) = file.hashes.get("sha1") {
        request = request.with_sha1(sha1.clone());
    }
    Ok(request)
}

// ---------------------------------------------------------------------------
// .mrpack (Modrinth modpack) import
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MrpackIndex {
    #[serde(default)]
    #[serde(rename = "formatVersion", alias = "format_version")]
    pub format_version: u32,
    #[serde(default)]
    pub game: String,
    #[serde(default)]
    #[serde(rename = "versionId", alias = "version_id")]
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
    #[serde(rename = "fileSize", alias = "file_size")]
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
    fn minecraft_mod_loaders_do_not_filter_pack_versions() {
        let forge = Loader::Forge { version: "47.4.0".into() };
        assert_eq!(modrinth_loader(ResourceType::Mod, &forge), Some("forge"));
        assert_eq!(modrinth_loader(ResourceType::ResourcePack, &forge), None);
        assert_eq!(modrinth_loader(ResourceType::ShaderPack, &forge), None);
    }

    #[test]
    fn datapack_downloads_accept_v2_mod_hits_but_choose_only_compatible_datapack_zips() {
        let mut hit: SearchResult = serde_json::from_value(serde_json::json!({
            "slug": "terralith", "title": "Terralith", "description": "",
            "project_id": "8oi3bsk5", "project_type": "mod",
            "categories": ["datapack", "fabric", "forge", "worldgen"]
        })).unwrap();
        assert!(is_datapack_project(&hit));
        hit.categories.clear();
        assert!(!is_datapack_project(&hit));
        hit.project_type = "datapack".into();
        assert!(is_datapack_project(&hit));

        let version = |loader: &str, mc: &str, filename: &str| -> ModVersion {
            serde_json::from_value(serde_json::json!({
                "id": "id", "project_id": "id", "name": "test", "version_number": "1.0",
                "loaders": [loader], "game_versions": [mc],
                "files": [{"filename": filename, "url": "https://example.com", "size": 1, "primary": true}]
            })).unwrap()
        };
        let versions = [
            version("fabric", "1.20.1", "mod.jar"),
            version("datapack", "1.21", "wrong-version.zip"),
            version("datapack", "1.20.1", "wrong-format.jar"),
            version("datapack", "1.20.1", "datapack.zip"),
        ];
        assert_eq!(datapack_file(&versions, "1.20.1").unwrap().filename, "datapack.zip");
        assert!(datapack_file(&versions, "1.19").is_none());
    }

    #[test]
    fn pack_downloads_use_their_own_directory_and_checksum() {
        let file = ModFile {
            url: "https://cdn.modrinth.com/pack.zip".into(),
            filename: "Cool Pack.zip".into(),
            primary: true,
            size: 123,
            hashes: [("sha1".into(), "abc".into())].into(),
        };
        let root = Path::new("game");
        for (kind, folder) in [
            (ResourceType::ResourcePack, "resourcepacks"),
            (ResourceType::ShaderPack, "shaderpacks"),
        ] {
            let request = install_request(root, &file, kind).unwrap();
            assert_eq!(request.dest, root.join(folder).join("Cool Pack.zip"));
            assert_eq!(request.sha1.as_deref(), Some("abc"));
        }

        for name in ["", ".", "..", "/pack.zip", "../pack.zip", "..\\pack.zip", "C:pack.zip", "pack.zip/"] {
            let mut file = file.clone();
            file.filename = name.into();
            assert!(install_request(root, &file, ResourceType::ResourcePack).is_err(), "{name:?}");
        }
    }

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
