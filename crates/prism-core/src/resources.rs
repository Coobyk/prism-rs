//! Local resource scanning: list mod / resource pack / shader pack files and
//! read their embedded metadata (fabric.mod.json, mods.toml, pack.mcmeta).
//! Mirrors `launcher/minecraft/mod/`.

use super::modplatform::{LocalResource, ResourceType};
use serde_json::Value;
use std::path::Path;
use std::path::PathBuf;

/// Scan a resource folder and parse metadata for each candidate file.
pub fn scan_folder(folder: &Path, resource_type: ResourceType) -> Vec<LocalResource> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(folder) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !is_resource_file(&path) {
            continue;
        }
        let file_name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let metadata = read_jar_metadata(&path).unwrap_or_default();
        out.push(LocalResource {
            file_name,
            path,
            title: metadata.title,
            version: metadata.version,
            loader: metadata.loader,
            description: metadata.description,
            mod_id: metadata.mod_id,
            authors: metadata.authors,
            homepage: metadata.homepage,
        });
    }
    // Keep .disabled entries too, marked by name, sorted by filename.
    out.sort_by(|a, b| a.file_name.cmp(&b.file_name));
    let _ = resource_type;
    out
}

fn is_resource_file(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    if path.is_dir() {
        return false;
    }
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
pub fn toggle_enabled(path: &PathBuf) -> std::io::Result<PathBuf> {
    let new_path = if let Some(stripped) = path.to_string_lossy().strip_suffix(".disabled") {
        PathBuf::from(stripped)
    } else {
        PathBuf::from(format!("{}.disabled", path.to_string_lossy()))
    };
    std::fs::rename(path, &new_path)?;
    Ok(new_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toml_scalars() {
        let text = "modLoader=\"javafml\"\n[[mods]]\nmodId=\"example\"\ndisplayName = \"Example Mod\"\nversion=\"1.2.3\"\n";
        assert_eq!(toml_scalar(text, "displayName").as_deref(), Some("Example Mod"));
        assert_eq!(toml_scalar(text, "version").as_deref(), Some("1.2.3"));
        assert_eq!(toml_scalar(text, "missing"), None);
    }
}
