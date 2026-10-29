//! Local resource scanning: list mod / resource pack / shader pack files and
//! read their embedded metadata (fabric.mod.json, mods.toml, pack.mcmeta).
//! Mirrors `launcher/minecraft/mod/`.

use super::modplatform::{LocalResource, ResourceType};
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
        let (title, version, loader, description) = read_metadata(&path);
        out.push(LocalResource {
            file_name,
            path,
            title,
            version,
            loader,
            description,
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
    name.ends_with(".jar") || name.ends_with(".zip") || name.ends_with(".litemod")
}

/// Read embedded metadata from a jar/zip. Tolerant: returns Nones on any
/// failure.
fn read_metadata(path: &Path) -> (Option<String>, Option<String>, Option<String>, Option<String>) {
    match read_jar_metadata(path) {
        Ok(meta) => meta,
        Err(_) => (None, None, None, None),
    }
}

fn read_jar_metadata(
    path: &Path,
) -> Result<(Option<String>, Option<String>, Option<String>, Option<String>), String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;

    // Fabric: fabric.mod.json
    if let Ok(mut f) = archive.by_name("fabric.mod.json") {
        let mut text = String::new();
        use std::io::Read;
        f.read_to_string(&mut text).map_err(|e| e.to_string())?;
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
            return Ok((
                v.get("name").and_then(|x| x.as_str()).map(String::from),
                v.get("version").and_then(|x| x.as_str()).map(String::from),
                Some("fabric".to_string()),
                v.get("description").and_then(|x| x.as_str()).map(String::from),
            ));
        }
    }

    // Forge/NeoForge: META-INF/mods.toml or mods.toml (basic TOML-ish scan)
    for name in ["META-INF/mods.toml", "mods.toml"] {
        if let Ok(mut f) = archive.by_name(name) {
            let mut text = String::new();
            use std::io::Read;
            f.read_to_string(&mut text).map_err(|e| e.to_string())?;
            return Ok((
                toml_scalar(&text, "displayName"),
                toml_scalar(&text, "version"),
                Some("forge".to_string()),
                toml_scalar(&text, "description"),
            ));
        }
    }

    // Legacy Forge: mcmod.info
    if let Ok(mut f) = archive.by_name("mcmod.info") {
        let mut text = String::new();
        use std::io::Read;
        f.read_to_string(&mut text).map_err(|e| e.to_string())?;
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
            let first = v.as_array().and_then(|a| a.first()).cloned().unwrap_or_default();
            return Ok((
                first.get("name").and_then(|x| x.as_str()).map(String::from),
                first.get("version").and_then(|x| x.as_str()).map(String::from),
                Some("forge".to_string()),
                first.get("description").and_then(|x| x.as_str()).map(String::from),
            ));
        }
    }

    // Quilt: quilt.mod.json
    if let Ok(mut f) = archive.by_name("quilt.mod.json") {
        let mut text = String::new();
        use std::io::Read;
        f.read_to_string(&mut text).map_err(|e| e.to_string())?;
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
            let loader = &v["quilt_loader"];
            return Ok((
                loader.get("id").and_then(|x| x.as_str()).map(String::from),
                loader.get("version").and_then(|x| x.as_str()).map(String::from),
                Some("quilt".to_string()),
                v["metadata"].get("description").and_then(|x| x.as_str()).map(String::from),
            ));
        }
    }

    Ok((None, None, None, None))
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
