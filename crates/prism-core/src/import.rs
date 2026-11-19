//! Instance import: `.mrpack` (Modrinth), CurseForge zips, and MultiMC/Prism
//! instance zips. Mirrors `launcher/InstanceImportTask.cpp`.

use crate::instance::{Instance, InstanceList};
use crate::loaders::Loader;
use crate::modplatform::flame::{FlameApi, FlameFile, FlameManifest, FlameManifestFile};
use crate::modplatform::modrinth::MrpackIndex;
use crate::modplatform::{PlatformError, ResourceType};
use crate::net::{DownloadRequest, Downloader, NetError};
use crate::Paths;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("unsupported or unrecognized pack")]
    UnknownFormat,
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("zip: {0}")]
    Zip(String),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("net: {0}")]
    Net(#[from] crate::net::NetError),
    #[error("platform: {0}")]
    Platform(#[from] crate::modplatform::PlatformError),
    #[error("{0}")]
    Other(String),
}

#[derive(Debug)]
pub struct ImportOutcome {
    pub instance: Instance,
    pub missing_files: Vec<MissingCurseForgeFile>,
    pub curseforge_api_key_missing: bool,
}

#[derive(Default)]
struct CurseForgeFiles {
    missing_files: Vec<MissingCurseForgeFile>,
    api_key_missing: bool,
}

pub const CURSEFORGE_MISSING_FILES_REPORT: &str = "curseforge-missing-files.txt";

#[derive(Debug, PartialEq, Eq)]
pub struct MissingCurseForgeFile {
    pub project_id: i64,
    pub file_id: i64,
    pub filename: Option<String>,
}

impl std::fmt::Display for MissingCurseForgeFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let project = self.project_id;
        let file = self.file_id;
        match &self.filename {
            Some(filename) => write!(f, "{filename} (CurseForge project {project}, file {file})"),
            None => write!(f, "CurseForge project {project}, file {file}"),
        }
    }
}

/// Detect the pack format of an (extracted) directory.
pub fn detect_format(dir: &Path) -> &'static str {
    if dir.join("modrinth.index.json").exists() {
        return "modrinth";
    }
    if dir.join("manifest.json").exists() {
        return "curseforge";
    }
    if dir.join("instance.cfg").exists() {
        return "multimc";
    }
    if dir.join("mmc-pack.json").exists() {
        return "multimc";
    }
    "unknown"
}

/// Extract a zip archive into `dest`.
pub fn extract_zip(zip_path: &Path, dest: &Path) -> Result<(), ImportError> {
    let file = std::fs::File::open(zip_path).map_err(ImportError::Io)?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| ImportError::Zip(e.to_string()))?;
    std::fs::create_dir_all(dest).map_err(ImportError::Io)?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| ImportError::Zip(e.to_string()))?;
        let Some(name) = entry.enclosed_name().map(|n| n.to_path_buf()) else {
            continue;
        };
        let out_path = dest.join(name);
        if entry.is_dir() {
            std::fs::create_dir_all(&out_path).map_err(ImportError::Io)?;
        } else {
            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent).map_err(ImportError::Io)?;
            }
            let mut out = std::fs::File::create(&out_path).map_err(ImportError::Io)?;
            std::io::copy(&mut entry, &mut out).map_err(ImportError::Io)?;
        }
    }
    Ok(())
}

/// Import an instance from a `.mrpack`/`.zip` file or an extracted directory.
/// Returns the created instance and any CurseForge files to download manually.
pub fn import_sync(
    paths: &Paths,
    source: &Path,
    downloader: Option<&Arc<Downloader>>,
) -> Result<ImportOutcome, ImportError> {
    import_with_curseforge_installer(paths, source, downloader, |files, game_root| {
        let api = FlameApi::new();
        install_curseforge_manifest_files(
            files,
            game_root,
            downloader,
            api.has_api_key().then_some(&api),
        )
    })
}

fn install_curseforge_manifest_files(
    files: &[FlameManifestFile],
    game_root: &Path,
    downloader: Option<&Arc<Downloader>>,
    api: Option<&FlameApi>,
) -> Result<CurseForgeFiles, ImportError> {
    if files.is_empty() {
        return Ok(CurseForgeFiles::default());
    }
    let Some(api) = api else {
        return Ok(CurseForgeFiles {
            missing_files: files
                .iter()
                .map(|entry| MissingCurseForgeFile {
                    project_id: entry.projectID,
                    file_id: entry.fileID,
                    filename: None,
                })
                .collect(),
            api_key_missing: true,
        });
    };
    let downloader = downloader.ok_or_else(|| {
        ImportError::Other("network downloader required for modpack files".to_string())
    })?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let missing_files = runtime.block_on(install_curseforge_files(
        files, game_root, downloader, api, |project, file| async move {
            let file = api.file(project, file).await?;
            let resource_type = if has_curseforge_download(&file) {
                api.project_resource_type(project).await?
            } else {
                ResourceType::Mod
            };
            Ok((file, resource_type))
        },
    ))?;
    Ok(CurseForgeFiles {
        missing_files,
        api_key_missing: false,
    })
}

fn import_with_curseforge_installer(
    paths: &Paths,
    source: &Path,
    downloader: Option<&Arc<Downloader>>,
    install_files: impl FnOnce(
        &[FlameManifestFile],
        &Path,
    ) -> Result<CurseForgeFiles, ImportError>,
) -> Result<ImportOutcome, ImportError> {
    // Extract zips to a temp dir first.
    let temp;
    let dir: &Path = if source.is_dir() {
        source
    } else {
        temp = paths.cache_dir().join("import-tmp");
        if temp.exists() {
            let _ = std::fs::remove_dir_all(&temp);
        }
        extract_zip(source, &temp)?;
        &temp
    };

    let instance = match detect_format(dir) {
        "modrinth" => import_modrinth(paths, dir, downloader),
        "multimc" => import_multimc(paths, dir),
        "curseforge" => return import_curseforge(paths, dir, install_files),
        _ => Err(ImportError::UnknownFormat),
    }?;
    Ok(ImportOutcome {
        instance,
        missing_files: Vec::new(),
        curseforge_api_key_missing: false,
    })
}

fn import_curseforge(
    paths: &Paths,
    dir: &Path,
    install_files: impl FnOnce(
        &[FlameManifestFile],
        &Path,
    ) -> Result<CurseForgeFiles, ImportError>,
) -> Result<ImportOutcome, ImportError> {
    let manifest = FlameManifest::from_dir(dir)?;
    if manifest.manifestType != "minecraftModpack" {
        return Err(ImportError::UnknownFormat);
    }
    let mc_version = manifest
        .minecraft_version()
        .filter(|version| !version.is_empty())
        .ok_or_else(|| {
            ImportError::Other("CurseForge manifest has no Minecraft version".to_string())
        })?;
    let loader = match manifest.loader_version() {
        Some((kind, version)) => match kind.as_str() {
            "forge" => Loader::Forge { version },
            "fabric" => Loader::Fabric { version },
            "neoforge" => Loader::NeoForge { version },
            "quilt" => Loader::Quilt { version },
            _ => {
                return Err(ImportError::Other(format!(
                    "unsupported CurseForge mod loader: {kind}"
                )));
            }
        },
        None if manifest.minecraft["modLoaders"]
            .as_array()
            .is_some_and(|loaders| !loaders.is_empty()) =>
        {
            return Err(ImportError::Other("invalid CurseForge mod loader".to_string()));
        }
        None => Loader::None,
    };
    let overrides = curseforge_overrides(dir, &manifest.overrides)?;
    let name = if manifest.name.trim().is_empty() {
        "Imported Modpack"
    } else {
        &manifest.name
    };
    let instance = InstanceList::create(paths, name, "default", &mc_version)?;
    let result = (|| {
        crate::instance::set_loader(&instance, &loader)?;
        let game_root = instance.game_root();
        let files = install_files(&manifest.files, &game_root)?;
        if let Some(overrides) = overrides {
            copy_dir_recursive(&overrides, &game_root)?;
        }
        if !files.missing_files.is_empty() {
            let report: String = files.missing_files.iter().map(|file| format!("{file}\n")).collect();
            std::fs::write(instance.root.join(CURSEFORGE_MISSING_FILES_REPORT), report)?;
        }
        Ok(files)
    })();
    match result {
        Ok(files) => Ok(ImportOutcome {
            instance,
            missing_files: files.missing_files,
            curseforge_api_key_missing: files.api_key_missing,
        }),
        Err(error) => {
            let _ = std::fs::remove_dir_all(&instance.root);
            Err(error)
        }
    }
}

fn curseforge_overrides(dir: &Path, overrides: &str) -> Result<Option<PathBuf>, ImportError> {
    if overrides.is_empty() {
        return Ok(None);
    }
    let invalid = || ImportError::Other(format!("invalid CurseForge overrides path: {overrides}"));
    let path = Path::new(overrides);
    if overrides.contains(['\\', ':'])
        || !path.components().all(|c| matches!(c, Component::Normal(_)))
    {
        return Err(invalid());
    }
    let root = dir.canonicalize()?;
    let path = match dir.join(path).canonicalize() {
        Ok(path) => path,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    if path == root || !path.starts_with(&root) || !path.is_dir() {
        return Err(invalid());
    }
    Ok(Some(path))
}

fn has_curseforge_download(file: &FlameFile) -> bool {
    file.downloadUrl.as_deref().is_some_and(|url| !url.is_empty())
}

async fn install_curseforge_files<F>(
    files: &[FlameManifestFile],
    game_root: &Path,
    downloader: &Arc<Downloader>,
    api: &FlameApi,
    resolve: impl Fn(i64, i64) -> F,
) -> Result<Vec<MissingCurseForgeFile>, ImportError>
where
    F: std::future::Future<Output = Result<(FlameFile, ResourceType), PlatformError>>,
{
    let mut missing = Vec::new();
    for entry in files {
        let (file, resource_type) = resolve(entry.projectID, entry.fileID).await?;
        if has_curseforge_download(&file) {
            match api
                .install_file(downloader, game_root, &file, resource_type)
                .await
            {
                Ok(_) => continue,
                Err(PlatformError::Net(NetError::Status { status: 403, .. })) => {}
                Err(error) => return Err(error.into()),
            }
        }
        missing.push(MissingCurseForgeFile {
            project_id: entry.projectID,
            file_id: entry.fileID,
            filename: (!file.fileName.is_empty()).then_some(file.fileName),
        });
    }
    Ok(missing)
}

fn import_modrinth(
    paths: &Paths,
    dir: &Path,
    downloader: Option<&Arc<Downloader>>,
) -> Result<Instance, ImportError> {
    let index = MrpackIndex::from_dir(dir)?;
    let name = if index.name.is_empty() {
        "Imported Modpack".to_string()
    } else {
        index.name.clone()
    };
    let mc_version = index
        .minecraft_version()
        .unwrap_or("1.20.1")
        .to_string();

    let inst = InstanceList::create(paths, &name, "default", &mc_version)
        .map_err(|e| ImportError::Io(e))?;

    // Loader component.
    if let Some((dep_uid, version)) = index.loader() {
        let loader = match dep_uid.as_str() {
            "fabric-loader" => Loader::Fabric { version },
            "quilt-loader" => Loader::Quilt { version },
            "forge" => Loader::Forge { version },
            "neoforge" => Loader::NeoForge { version },
            _ => Loader::None,
        };
        crate::instance::set_loader(&inst, &loader).map_err(ImportError::Io)?;
    }

    // Download the pack files.
    if let Some(dl) = downloader {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| ImportError::Other(e.to_string()))?;
        rt.block_on(async {
            index
                .install_files(dl, &inst.game_root())
                .await
        })?;
    } else {
        return Err(ImportError::Other(
            "network downloader required for modpack files".to_string(),
        ));
    }

    // Copy overrides/ (everything else in the pack).
    let overrides = dir.join("overrides");
    if overrides.exists() {
        copy_dir_recursive(&overrides, &inst.game_root())?;
    }

    Ok(inst)
}

fn import_multimc(paths: &Paths, dir: &Path) -> Result<Instance, ImportError> {
    // The zip contains one or more instance directories at the root (or the
    // instance at the top level).
    let candidates: Vec<PathBuf> = if dir.join("instance.cfg").exists() {
        vec![dir.to_path_buf()]
    } else {
        std::fs::read_dir(dir)
            .map_err(ImportError::Io)?
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.join("instance.cfg").exists())
            .collect()
    };
    let Some(src) = candidates.first() else {
        return Err(ImportError::UnknownFormat);
    };
    let inst = Instance::load(src).ok_or(ImportError::UnknownFormat)?;

    // Copy into the instances dir under a fresh unique name.
    let dest_root = paths.instances_dir();
    std::fs::create_dir_all(&dest_root).map_err(ImportError::Io)?;
    let base: String = inst
        .name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '_' })
        .collect();
    let mut candidate = dest_root.join(&base);
    let mut n = 1;
    while candidate.exists() {
        n += 1;
        candidate = dest_root.join(format!("{base}{n}"));
    }
    copy_dir_recursive(src, &candidate)?;
    let imported = Instance::load(&candidate).ok_or(ImportError::UnknownFormat)?;
    Ok(imported)
}

pub fn copy_dir_recursive(src: &Path, dest: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let to = dest.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_recursive(&entry.path(), &to)?;
        } else {
            std::fs::copy(entry.path(), &to)?;
        }
    }
    Ok(())
}

/// Download helper for pack file lists (used by importers).
pub fn download_requests_from_urls(urls: Vec<(String, PathBuf, Option<String>)>) -> Vec<DownloadRequest> {
    urls.into_iter()
        .map(|(url, dest, sha1)| {
            let r = DownloadRequest::new(url, dest);
            match sha1 {
                Some(s) => r.with_sha1(s),
                None => r,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn detects_formats() {
        let dir = std::env::temp_dir().join(format!("prism-import-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(detect_format(&dir), "unknown");
        std::fs::write(dir.join("modrinth.index.json"), "{}").unwrap();
        assert_eq!(detect_format(&dir), "modrinth");
        std::fs::remove_file(dir.join("modrinth.index.json")).unwrap();
        std::fs::write(dir.join("manifest.json"), "{}").unwrap();
        assert_eq!(detect_format(&dir), "curseforge");
        std::fs::remove_file(dir.join("manifest.json")).unwrap();
        std::fs::write(dir.join("instance.cfg"), "name=x").unwrap();
        assert_eq!(detect_format(&dir), "multimc");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn imports_curseforge_zip_with_overrides_and_missing_file_report() {
        let paths = Paths {
            root: std::env::temp_dir().join(format!("prism-import-{}", uuid::Uuid::new_v4())),
        };
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(paths.root.clone());
        paths.ensure().unwrap();
        let source = paths.root.join("pack.zip");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&source).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        zip.start_file("manifest.json", options).unwrap();
        zip.write_all(br#"{
            "manifestType": "minecraftModpack",
            "manifestVersion": 1,
            "name": "Test CurseForge",
            "minecraft": {"version": "1.20.1", "modLoaders": [{"id": "forge-47.2.0", "primary": true}]},
            "overrides": "custom",
            "files": [{"projectID": 42, "fileID": 73, "required": true}]
        }"#).unwrap();
        zip.start_file("custom/config/test.toml", options).unwrap();
        zip.write_all(b"enabled = true").unwrap();
        zip.finish().unwrap();

        let outcome = import_with_curseforge_installer(&paths, &source, None, |files, game_root| {
            let downloader = Arc::new(Downloader::new());
            let api = FlameApi::new();
            let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
            let missing_files = runtime.block_on(install_curseforge_files(files, game_root, &downloader, &api, |project, file| {
                assert_eq!((project, file), (42, 73));
                std::future::ready(Ok((serde_json::from_str::<FlameFile>(
                    r#"{"id":73,"fileName":"blocked.jar","downloadUrl":null}"#,
                ).unwrap(), ResourceType::Mod)))
            }))?;
            Ok(CurseForgeFiles { missing_files, api_key_missing: false })
        }).unwrap();
        let instance = outcome.instance;
        assert_eq!(Instance::load(&instance.root).unwrap().name, "Test CurseForge");
        assert_eq!(crate::instance::loader_of(&instance), Loader::Forge { version: "47.2.0".into() });
        let pack: serde_json::Value = serde_json::from_slice(
            &std::fs::read(instance.root.join("mmc-pack.json")).unwrap(),
        ).unwrap();
        assert_eq!(pack["components"][0]["version"], "1.20.1");
        assert_eq!(std::fs::read_to_string(instance.game_root().join("config/test.toml")).unwrap(), "enabled = true");
        assert_eq!(outcome.missing_files, vec![MissingCurseForgeFile {
            project_id: 42,
            file_id: 73,
            filename: Some("blocked.jar".into()),
        }]);
        assert_eq!(
            std::fs::read_to_string(instance.root.join(CURSEFORGE_MISSING_FILES_REPORT)).unwrap(),
            "blocked.jar (CurseForge project 42, file 73)\n",
        );
    }

    #[test]
    fn imports_overrides_and_reports_all_files_without_a_curseforge_api_key() {
        let paths = Paths {
            root: std::env::temp_dir().join(format!("prism-import-{}", uuid::Uuid::new_v4())),
        };
        let source = paths.root.join("pack");
        let overrides = source.join("overrides/config");
        std::fs::create_dir_all(&overrides).unwrap();
        std::fs::write(overrides.join("test.toml"), "enabled = true").unwrap();
        std::fs::write(source.join("manifest.json"), br#"{
            "manifestType":"minecraftModpack",
            "minecraft":{"version":"1.20.1"},
            "name":"Offline CurseForge",
            "files":[{"projectID":42,"fileID":73,"required":true},{"projectID":43,"fileID":74,"required":false}]
        }"#).unwrap();
        let outcome = import_with_curseforge_installer(&paths, &source, None, |files, game_root| {
            install_curseforge_manifest_files(files, game_root, None, None)
        }).unwrap();
        assert!(outcome.curseforge_api_key_missing);
        assert_eq!(outcome.missing_files.len(), 2);
        assert!(outcome.instance.game_root().join("config/test.toml").is_file());
        assert_eq!(
            std::fs::read_to_string(outcome.instance.root.join(CURSEFORGE_MISSING_FILES_REPORT)).unwrap(),
            "CurseForge project 42, file 73\nCurseForge project 43, file 74\n"
        );
        std::fs::remove_dir_all(&paths.root).unwrap();
    }
}
