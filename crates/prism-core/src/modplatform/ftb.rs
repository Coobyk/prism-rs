//! FTB public catalog and client-side pack installation.

use crate::import::{
    CurseForgeKeyIssue, ImportError, ImportOutcome, MissingCurseForgeFile,
    CURSEFORGE_MISSING_FILES_REPORT,
};
use crate::ini::IniFile;
use crate::instance::{set_loader, InstanceList};
use crate::loaders::Loader;
use crate::modplatform::{flame::FlameApi, PlatformError};
use crate::net::{DownloadRequest, Downloader};
use crate::Paths;
use futures::{stream, StreamExt, TryStreamExt};
use serde::Deserialize;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;

const API: &str = "https://api.feed-the-beast.com/v1/modpacks/public";

#[cfg(test)]
#[path = "ftb_tests.rs"]
mod tests;

fn invalid(message: impl Into<String>) -> ImportError {
    ImportError::Other(message.into())
}

#[derive(Clone, Debug, Deserialize)]
pub struct Catalog {
    pub packs: Vec<u64>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Author {
    pub name: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct VersionInfo {
    pub id: u64,
    pub name: String,
    #[serde(default)]
    pub updated: u64,
    #[serde(default, rename = "type")]
    pub release_type: String,
    #[serde(default)]
    pub targets: Vec<Target>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Artwork {
    pub url: String,
    #[serde(rename = "type")]
    pub kind: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Pack {
    pub id: u64,
    pub name: String,
    #[serde(default)]
    pub synopsis: String,
    #[serde(default)]
    pub authors: Vec<Author>,
    #[serde(default)]
    pub plays: u64,
    #[serde(default)]
    pub installs: u64,
    #[serde(default)]
    pub art: Vec<Artwork>,
    #[serde(default)]
    pub tags: Vec<Author>,
    pub versions: Vec<VersionInfo>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sort {
    Plays,
    Installs,
    Name,
}

pub fn filter_catalog(packs: &[Pack], search: &str, sort: Sort) -> Vec<Pack> {
    let search = search.trim().to_lowercase();
    let mut result: Vec<_> = packs
        .iter()
        .filter(|p| p.name.to_lowercase().contains(&search))
        .cloned()
        .collect();
    result.sort_by(|a, b| match sort {
        Sort::Plays => b.plays.cmp(&a.plays).then(a.id.cmp(&b.id)),
        Sort::Installs => b.installs.cmp(&a.installs).then(a.id.cmp(&b.id)),
        Sort::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
    });
    result
}

#[derive(Clone, Debug, Deserialize)]
pub struct Target {
    pub name: String,
    pub version: String,
    #[serde(rename = "type", default)]
    pub kind: String,
}

pub fn loader(name: &str, version: &str) -> Result<Loader, ImportError> {
    if version.trim().is_empty() {
        return Err(invalid("Empty FTB loader version"));
    }
    let version = version.to_owned();
    match name.to_ascii_lowercase().as_str() {
        "forge" => Ok(Loader::Forge { version }),
        "neoforge" => Ok(Loader::NeoForge { version }),
        "fabric" => Ok(Loader::Fabric { version }),
        "quilt" => Ok(Loader::Quilt { version }),
        _ => Err(invalid(format!("Unsupported FTB loader: {name}"))),
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct CurseForgeFile {
    #[serde(default, deserialize_with = "numeric_id")]
    pub project: i64,
    #[serde(default, deserialize_with = "numeric_id")]
    pub file: i64,
}

fn numeric_id<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<i64, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Id {
        Number(i64),
        Text(String),
    }
    match Option::<Id>::deserialize(deserializer)? {
        Some(Id::Number(value)) => Ok(value),
        Some(Id::Text(value)) => value.parse().map_err(serde::de::Error::custom),
        None => Ok(0),
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct File {
    pub path: String,
    pub name: String,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub sha1: String,
    #[serde(default)]
    pub serveronly: bool,
    #[serde(default)]
    pub curseforge: Option<CurseForgeFile>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Manifest {
    pub id: u64,
    pub parent: u64,
    pub name: String,
    pub targets: Vec<Target>,
    pub files: Vec<File>,
}

pub(crate) fn safe_path(value: &str) -> Result<PathBuf, ImportError> {
    let path = Path::new(value);
    if value.contains(['\\', ':', '\0'])
        || path.is_absolute()
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
    {
        return Err(invalid(format!("Unsafe FTB path: {value}")));
    }
    let clean: PathBuf = path
        .components()
        .filter(|c| matches!(c, Component::Normal(_)))
        .collect();
    if clean.as_os_str().is_empty() {
        return Err(invalid("Empty FTB file path"));
    }
    Ok(clean)
}

#[derive(Clone)]
pub struct FtbApi {
    downloader: Arc<Downloader>,
    base: String,
    catalog: Arc<Mutex<Option<Vec<Pack>>>>,
    progress: Option<Arc<dyn Fn(String) + Send + Sync>>,
}

impl FtbApi {
    pub fn new(downloader: Arc<Downloader>) -> Self {
        Self {
            downloader,
            base: API.into(),
            catalog: Arc::new(Mutex::new(None)),
            progress: None,
        }
    }

    pub fn with_progress(mut self, progress: impl Fn(String) + Send + Sync + 'static) -> Self {
        self.progress = Some(Arc::new(progress));
        self
    }

    fn report(&self, message: String) {
        if let Some(progress) = &self.progress {
            progress(message);
        }
    }

    #[cfg(test)]
    fn with_endpoint(downloader: Arc<Downloader>, base: String) -> Self {
        Self {
            base,
            ..Self::new(downloader)
        }
    }

    async fn get<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T, ImportError> {
        let response = self
            .downloader
            .client()
            .get(format!("{}{path}", self.base))
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(PlatformError::from)?;
        let response = response.error_for_status().map_err(PlatformError::from)?;
        let bytes = response.bytes().await.map_err(PlatformError::from)?;
        serde_json::from_slice(&bytes)
            .map_err(|error| invalid(format!("Invalid FTB response for {path}: {error}")))
    }

    pub async fn pack(&self, id: u64) -> Result<Pack, ImportError> {
        let pack: Pack = self.get(&format!("/modpack/{id}")).await?;
        if pack.id != id {
            return Err(invalid("FTB returned the wrong pack"));
        }
        Ok(pack)
    }

    pub async fn catalog(&self) -> Result<Vec<Pack>, ImportError> {
        let mut cached = self.catalog.lock().await;
        if let Some(packs) = cached.as_ref() {
            return Ok(packs.clone());
        }
        let catalog: Catalog = self.get("/modpack/all").await?;
        let packs: Vec<_> = stream::iter(catalog.packs.into_iter().map(|id| self.pack(id)))
            .buffered(3)
            .try_collect()
            .await?;
        *cached = Some(packs.clone());
        Ok(packs)
    }

    pub async fn install(
        &self,
        paths: &Paths,
        id: u64,
        version: Option<u64>,
    ) -> Result<ImportOutcome, ImportError> {
        self.report("Fetching FTB manifest…".into());
        let pack = self.pack(id).await?;
        let selected = match version {
            Some(id) => pack.versions.iter().find(|v| v.id == id),
            None => pack.versions.iter().max_by_key(|v| (v.updated, v.id)),
        }
        .ok_or_else(|| invalid("No matching FTB version"))?;
        let manifest: Manifest = self.get(&format!("/modpack/{id}/{}", selected.id)).await?;
        if manifest.parent != id || manifest.id != selected.id {
            return Err(invalid("FTB manifest identity mismatch"));
        }
        self.install_manifest(paths, &pack, &manifest, &FlameApi::new())
            .await
    }

    async fn install_manifest(
        &self,
        paths: &Paths,
        pack: &Pack,
        manifest: &Manifest,
        flame: &FlameApi,
    ) -> Result<ImportOutcome, ImportError> {
        let mc = manifest
            .targets
            .iter()
            .find(|t| t.kind == "game" && t.name == "minecraft")
            .filter(|t| !t.version.trim().is_empty())
            .ok_or_else(|| invalid("FTB manifest has no Minecraft target"))?;
        let loaders = manifest
            .targets
            .iter()
            .filter(|t| t.kind == "modloader")
            .map(|t| loader(&t.name, &t.version))
            .collect::<Result<Vec<_>, _>>()?;
        if loaders.len() > 1 {
            return Err(invalid("Multiple FTB loaders are not supported"));
        }
        let mut files = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for file in manifest.files.iter().filter(|f| !f.serveronly) {
            if file.name.is_empty() || file.name.contains(['/', '\\']) {
                return Err(invalid("Invalid FTB filename"));
            }
            let joined = if file.path.is_empty() {
                file.name.clone()
            } else {
                format!("{}/{}", file.path.trim_end_matches('/'), file.name)
            };
            let path = safe_path(&joined)?;
            if !seen.insert(path.clone()) {
                return Err(invalid("Duplicate FTB file path"));
            }
            if !file.sha1.is_empty()
                && (file.sha1.len() != 40 || !file.sha1.bytes().all(|b| b.is_ascii_hexdigit()))
            {
                return Err(invalid("Invalid FTB SHA-1"));
            }
            files.push((file, path));
        }
        let title = if pack.name.trim().is_empty() {
            "FTB Modpack"
        } else {
            &pack.name
        };
        let instance = InstanceList::create(paths, title, "default", &mc.version)?;
        let result = async {
            if let Some(loader) = loaders.first() {
                set_loader(&instance, loader)?;
            }
            let mut missing = Vec::new();
            let mut key_issue = None;
            let total = files.len();
            for (index, (file, path)) in files.into_iter().enumerate() {
                self.report(format!(
                    "Downloading and verifying {}/{}: {}",
                    index + 1,
                    total,
                    path.display()
                ));
                let mut url = file.url.clone().filter(|s| !s.trim().is_empty());
                if url.is_none() {
                    if let Some(cf) = file
                        .curseforge
                        .as_ref()
                        .filter(|cf| cf.project > 0 && cf.file > 0)
                    {
                        if flame.has_api_key() {
                            match flame.file(cf.project, cf.file).await {
                                Ok(resolved) => {
                                    url = resolved.downloadUrl.filter(|s| !s.trim().is_empty())
                                }
                                Err(PlatformError::CurseForgeApiKeyRejected(_)) => {
                                    key_issue = Some(CurseForgeKeyIssue::Rejected)
                                }
                                Err(error) => return Err(error.into()),
                            }
                        } else {
                            key_issue = Some(CurseForgeKeyIssue::Missing);
                        }
                    }
                }
                if let Some(url) = url {
                    let mut request = DownloadRequest::new(url, instance.game_root().join(&path));
                    if !file.sha1.is_empty() {
                        request = request.with_sha1(file.sha1.to_lowercase());
                    }
                    self.downloader.download(&request).await?;
                } else {
                    let cf = file.curseforge.clone().unwrap_or_default();
                    missing.push(MissingCurseForgeFile {
                        project_id: cf.project,
                        file_id: cf.file,
                        filename: Some(path.to_string_lossy().into_owned()),
                    });
                }
            }
            if !missing.is_empty() {
                let report = missing.iter().map(|f| format!("{f}\n")).collect::<String>();
                std::fs::write(instance.root.join(CURSEFORGE_MISSING_FILES_REPORT), report)?;
            }
            write_provenance(
                &instance.root,
                pack.id,
                &pack.name,
                manifest.id,
                &manifest.name,
            )?;
            Ok(ImportOutcome {
                instance: instance.clone(),
                missing_files: missing,
                curseforge_key_issue: key_issue,
            })
        }
        .await;
        if result.is_err() {
            let _ = std::fs::remove_dir_all(&instance.root);
        }
        result
    }
}

pub(crate) fn write_provenance(
    root: &Path,
    pack: u64,
    name: &str,
    version: u64,
    version_name: &str,
) -> Result<(), ImportError> {
    let path = root.join("instance.cfg");
    let mut cfg = IniFile::load(&path)?;
    cfg.set("ManagedPack", "true");
    cfg.set("ManagedPackType", "ftb");
    cfg.set("ManagedPackID", pack.to_string());
    cfg.set("ManagedPackName", name);
    cfg.set("ManagedPackVersionID", version.to_string());
    cfg.set("ManagedPackVersionName", version_name);
    cfg.save(&path)?;
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
pub struct Source {
    pub pack: u64,
    pub version: Option<u64>,
}

impl Source {
    pub fn parse(value: &str) -> Result<Option<Self>, ImportError> {
        let Some(rest) = value.strip_prefix("ftb:") else {
            return Ok(None);
        };
        let parts: Vec<_> = rest.split(':').collect();
        let number = |s: &str| s.parse::<u64>().ok().filter(|n| *n > 0);
        let pack = parts
            .first()
            .and_then(|s| number(s))
            .ok_or_else(|| invalid("Use ftb:<pack-id>[:<version-id>]"))?;
        let version = match parts.as_slice() {
            [_] => None,
            [_, v] => Some(number(v).ok_or_else(|| invalid("Invalid FTB version ID"))?),
            _ => return Err(invalid("Use ftb:<pack-id>[:<version-id>]")),
        };
        Ok(Some(Self { pack, version }))
    }
}
