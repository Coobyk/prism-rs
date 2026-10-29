//! Instance import: `.mrpack` (Modrinth), CurseForge zips, and MultiMC/Prism
//! instance zips. Mirrors `launcher/InstanceImportTask.cpp`.

use crate::instance::{Instance, InstanceList};
use crate::loaders::Loader;
use crate::modplatform::modrinth::MrpackIndex;
use crate::net::{DownloadRequest, Downloader};
use crate::Paths;
use std::path::{Path, PathBuf};
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
/// Returns the created instance.
pub fn import_sync(
    paths: &Paths,
    source: &Path,
    downloader: Option<&Arc<Downloader>>,
) -> Result<Instance, ImportError> {
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

    match detect_format(dir) {
        "modrinth" => import_modrinth(paths, dir, downloader),
        "multimc" => import_multimc(paths, dir),
        "curseforge" => Err(ImportError::Other(
            "CurseForge packs are not yet supported; use a Modrinth .mrpack".to_string(),
        )),
        _ => Err(ImportError::UnknownFormat),
    }
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
}
