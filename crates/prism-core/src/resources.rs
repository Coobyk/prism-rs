//! Local resource scanning: list mod / resource pack / shader pack files and
//! read their embedded metadata (fabric.mod.json, mods.toml, pack.mcmeta).
//! Mirrors `launcher/minecraft/mod/`.

use super::modplatform::{LocalResource, ResourceType};
use serde_json::Value;
use std::io;
use std::path::Path;
use std::path::PathBuf;
use std::time::SystemTime;

/// Scan a resource folder and parse metadata for each candidate file.
pub fn scan_folder(folder: &Path, _resource_type: ResourceType) -> Vec<LocalResource> {
    resource_files(folder)
        .into_iter()
        .map(|(path, _)| resource_row(path))
        .collect()
}

/// A snapshot of a resource folder and its parsed rows.
pub struct FolderScan {
    folder: PathBuf,
    files: Vec<ResourceFile>,
    rows: Vec<LocalResource>,
}

impl FolderScan {
    /// Return a new snapshot only if the folder or its candidate files changed.
    pub fn refresh(folder: &Path, previous: Option<&Self>) -> Option<Self> {
        let files = resource_files(folder);
        if previous.is_some_and(|scan| scan.folder == folder && scan.files == files) {
            return None;
        }
        let rows = files.iter().map(|(path, _)| resource_row(path.clone())).collect();
        Some(Self { folder: folder.to_path_buf(), files, rows })
    }

    pub fn folder(&self) -> &Path {
        &self.folder
    }

    pub fn rows(&self) -> &[LocalResource] {
        &self.rows
    }
}

#[derive(PartialEq, Eq)]
struct FileStamp {
    len: u64,
    modified: Option<SystemTime>,
    created: Option<SystemTime>,
    #[cfg(unix)]
    changed: (i64, i64),
}

type ResourceFile = (PathBuf, Option<FileStamp>);

fn resource_files(folder: &Path) -> Vec<ResourceFile> {
    let mut files = Vec::new();
    let Ok(entries) = std::fs::read_dir(folder) else {
        return files;
    };
    for entry in entries.flatten() {
        if !entry.file_name().to_str().is_some_and(is_resource_name) {
            continue;
        }
        let path = entry.path();
        let metadata = std::fs::metadata(&path).ok();
        if metadata.as_ref().is_some_and(std::fs::Metadata::is_dir) {
            continue;
        }
        let stamp = metadata.map(|metadata| {
            #[cfg(unix)]
            use std::os::unix::fs::MetadataExt;
            FileStamp {
                len: metadata.len(),
                modified: metadata.modified().ok(),
                created: metadata.created().ok(),
                #[cfg(unix)]
                changed: (metadata.ctime(), metadata.ctime_nsec()),
            }
        });
        files.push((path, stamp));
    }
    // Keep .disabled entries too, marked by name, sorted by filename.
    files.sort_unstable_by(|(a, _), (b, _)| a.file_name().cmp(&b.file_name()));
    files
}

fn resource_row(path: PathBuf) -> LocalResource {
    let file_name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let metadata = read_jar_metadata(&path).unwrap_or_default();
    LocalResource {
        file_name,
        path,
        title: metadata.title,
        version: metadata.version,
        loader: metadata.loader,
        description: metadata.description,
        mod_id: metadata.mod_id,
        authors: metadata.authors,
        homepage: metadata.homepage,
    }
}

fn is_resource_name(name: &str) -> bool {
    let name = name.to_lowercase();
    let name = name.strip_suffix(".disabled").unwrap_or(&name);
    name.ends_with(".jar") || name.ends_with(".zip") || name.ends_with(".litemod")
}

#[derive(Default)]
struct ResourceMetadata {
    title: Option<String>,
    version: Option<String>,
    loader: Option<String>,
    description: Option<String>,
    mod_id: Option<String>,
    authors: Vec<String>,
    homepage: Option<String>,
}

fn string(value: &Value) -> Option<String> {
    value.as_str().filter(|s| !s.trim().is_empty()).map(String::from)
}

fn authors(value: &Value) -> Vec<String> {
    value.as_array().into_iter().flatten()
        .filter_map(|author| string(author).or_else(|| string(&author["name"])))
        .collect()
}

fn read_jar_metadata(path: &Path) -> Result<ResourceMetadata, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;

    // Fabric: fabric.mod.json
    if let Ok(mut f) = archive.by_name("fabric.mod.json") {
        let mut text = String::new();
        use std::io::Read;
        f.read_to_string(&mut text).map_err(|e| e.to_string())?;
        if let Ok(v) = serde_json::from_str::<Value>(&text) {
            return Ok(ResourceMetadata {
                title: string(&v["name"]),
                version: string(&v["version"]),
                loader: Some("fabric".into()),
                description: string(&v["description"]),
                mod_id: string(&v["id"]),
                authors: authors(&v["authors"]),
                homepage: string(&v["contact"]["homepage"]),
            });
        }
    }

    for name in ["META-INF/neoforge.mods.toml", "META-INF/mods.toml", "mods.toml"] {
        if let Ok(mut f) = archive.by_name(name) {
            let mut text = String::new();
            use std::io::Read;
            f.read_to_string(&mut text).map_err(|e| e.to_string())?;
            return Ok(ResourceMetadata {
                title: toml_scalar(&text, "displayName"),
                version: toml_scalar(&text, "version"),
                loader: Some("forge".into()),
                description: toml_scalar(&text, "description"),
                mod_id: toml_scalar(&text, "modId"),
                authors: toml_scalar(&text, "authors").into_iter()
                    .flat_map(|names| names.split(',').map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect::<Vec<_>>())
                    .collect(),
                homepage: toml_scalar(&text, "displayURL"),
            });
        }
    }

    // Legacy Forge: mcmod.info
    if let Ok(mut f) = archive.by_name("mcmod.info") {
        let mut text = String::new();
        use std::io::Read;
        f.read_to_string(&mut text).map_err(|e| e.to_string())?;
        if let Ok(v) = serde_json::from_str::<Value>(&text) {
            let first = v.as_array().or_else(|| v["modList"].as_array())
                .and_then(|list| list.first()).unwrap_or(&Value::Null);
            return Ok(ResourceMetadata {
                title: string(&first["name"]),
                version: string(&first["version"]),
                loader: Some("forge".into()),
                description: string(&first["description"]),
                mod_id: string(&first["modid"]),
                authors: authors(&first["authorList"]),
                homepage: string(&first["url"]),
            });
        }
    }

    // Quilt: quilt.mod.json
    if let Ok(mut f) = archive.by_name("quilt.mod.json") {
        let mut text = String::new();
        use std::io::Read;
        f.read_to_string(&mut text).map_err(|e| e.to_string())?;
        if let Ok(v) = serde_json::from_str::<Value>(&text) {
            let loader = &v["quilt_loader"];
            let metadata = &loader["metadata"];
            return Ok(ResourceMetadata {
                title: string(&metadata["name"]).or_else(|| string(&loader["id"])),
                version: string(&loader["version"]),
                loader: Some("quilt".into()),
                description: string(&metadata["description"]).or_else(|| string(&v["metadata"]["description"])),
                mod_id: string(&loader["id"]),
                authors: metadata["contributors"].as_object().into_iter()
                    .flat_map(|contributors| contributors.keys().cloned()).collect(),
                homepage: string(&metadata["contact"]["homepage"]),
            });
        }
    }

    Ok(ResourceMetadata::default())
}

/// Extract a bare `key = "value"` scalar from loosely-parsed TOML text.
fn toml_scalar(text: &str, key: &str) -> Option<String> {
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix(&format!("{key}")) {
            let rest = rest.trim_start();
            let Some(rest) = rest.strip_prefix('=') else { continue };
            let rest = rest.trim();
            let quoted = rest
                .strip_prefix('"')
                .and_then(|s| s.strip_suffix('"'))
                .or_else(|| rest.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')));
            return Some(quoted.unwrap_or(rest).to_string());
        }
    }
    None
}

/// Toggle a resource file's enabled state (`.disabled` suffix), mirroring
/// Prism's mod toggling.
pub fn toggle_enabled(path: &Path) -> io::Result<PathBuf> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Invalid resource file name"))?;
    let target_name = match name.strip_suffix(".disabled") {
        Some(stripped) => stripped.to_owned(),
        None => format!("{name}.disabled"),
    };
    let new_path = path.with_file_name(&target_name);
    match std::fs::symlink_metadata(&new_path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("{target_name} already exists; neither file was changed."),
            ));
        }
    }
    std::fs::rename(path, &new_path)?;
    Ok(new_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_mod(path: &Path, title: &str) {
        let file = std::fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        zip.start_file("fabric.mod.json", zip::write::SimpleFileOptions::default())
            .unwrap();
        write!(zip, r#"{{"name":"{title}"}}"#).unwrap();
        zip.finish().unwrap();
    }

    #[test]
    fn folder_scan_tracks_add_replace_toggle_and_remove() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("mods");
        let empty = FolderScan::refresh(&folder, None).unwrap();
        assert!(empty.rows().is_empty());
        assert!(FolderScan::refresh(&folder, Some(&empty)).is_none());

        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("ignored.txt"), b"not a resource").unwrap();
        std::fs::create_dir(folder.join("ignored.jar")).unwrap();
        assert!(FolderScan::refresh(&folder, Some(&empty)).is_none());

        let path = folder.join("example.jar");
        write_mod(&path, "Original");
        let added = FolderScan::refresh(&folder, Some(&empty)).unwrap();
        assert_eq!(added.rows().len(), 1);
        assert_eq!(added.rows()[0].title.as_deref(), Some("Original"));
        assert!(FolderScan::refresh(&folder, Some(&added)).is_none());

        write_mod(&path, "Replacement");
        let replaced = FolderScan::refresh(&folder, Some(&added)).unwrap();
        assert_eq!(replaced.rows()[0].title.as_deref(), Some("Replacement"));

        let disabled_path = toggle_enabled(&path).unwrap();
        let toggled = FolderScan::refresh(&folder, Some(&replaced)).unwrap();
        assert_eq!(toggled.rows()[0].path, disabled_path);
        assert_eq!(toggled.rows()[0].file_name, "example.jar.disabled");
        assert_eq!(toggled.rows()[0].title.as_deref(), Some("Replacement"));

        std::fs::remove_file(disabled_path).unwrap();
        let removed = FolderScan::refresh(&folder, Some(&toggled)).unwrap();
        assert!(removed.rows().is_empty());
        assert!(FolderScan::refresh(&folder, Some(&removed)).is_none());
    }

    #[test]
    fn folder_scan_does_not_reuse_a_different_folder() {
        let dir = tempfile::tempdir().unwrap();
        let mods = dir.path().join("mods");
        let packs = dir.path().join("resourcepacks");
        let scan = FolderScan::refresh(&mods, None).unwrap();
        let other = FolderScan::refresh(&packs, Some(&scan)).unwrap();
        assert_eq!(other.folder(), packs);
    }

    #[test]
    fn toggle_enabled_rejects_conflict_in_both_directions() {
        let dir = tempfile::tempdir().unwrap();
        let enabled = dir.path().join("mod.jar");
        let disabled = dir.path().join("mod.jar.disabled");
        std::fs::write(&enabled, b"enabled-original-payload").unwrap();
        std::fs::write(&disabled, b"disabled-distinct-payload").unwrap();

        for (source, target) in [(&enabled, &disabled), (&disabled, &enabled)] {
            let error = toggle_enabled(source).expect_err("toggle must not overwrite another file");
            assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
            assert!(error
                .to_string()
                .contains(target.file_name().unwrap().to_str().unwrap()));
            assert_eq!(std::fs::read(&enabled).unwrap(), b"enabled-original-payload");
            assert_eq!(std::fs::read(&disabled).unwrap(), b"disabled-distinct-payload");
        }
    }

    #[test]
    fn toggle_enabled_round_trip_without_conflict() {
        let dir = tempfile::tempdir().unwrap();
        let enabled = dir.path().join("mod.jar");
        let disabled = dir.path().join("mod.jar.disabled");
        std::fs::write(&enabled, b"original-payload").unwrap();

        for (source, target) in [(&enabled, &disabled), (&disabled, &enabled)] {
            assert_eq!(toggle_enabled(source).unwrap(), *target);
            assert!(!source.exists());
            assert_eq!(std::fs::read(target).unwrap(), b"original-payload");
        }
    }

    #[test]
    fn toml_scalars() {
        let text = "modLoader=\"javafml\"\n[[mods]]\nmodId=\"example\"\ndisplayName = \"Example Mod\"\nversion=\"1.2.3\"\n";
        assert_eq!(toml_scalar(text, "displayName").as_deref(), Some("Example Mod"));
        assert_eq!(toml_scalar(text, "version").as_deref(), Some("1.2.3"));
        assert_eq!(toml_scalar(text, "missing"), None);
    }
}
