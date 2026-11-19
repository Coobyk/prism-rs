//! Import an existing FTB App game directory without modifying its source.

use crate::import::{ImportError, ImportOutcome};
use crate::ini::IniFile;
use crate::instance::{set_loader, InstanceList};
use crate::loaders::Loader;
use crate::modplatform::ftb::{loader, write_provenance, Target};
use crate::Paths;
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[cfg(test)]
#[path = "import_ftb_tests.rs"]
mod tests;

#[derive(Clone, Debug, Deserialize)]
pub struct AppInstance {
    pub name: String,
    pub id: u64,
    #[serde(rename = "versionId")]
    pub version_id: u64,
    pub version: String,
    #[serde(rename = "mcVersion")]
    pub minecraft: String,
    #[serde(default, rename = "modLoader")]
    pub mod_loader: String,
    #[serde(default, rename = "jvmArgs")]
    pub jvm_args: Option<String>,
    #[serde(default, rename = "totalPlayTime")]
    pub playtime: u64,
    #[serde(skip)]
    pub path: PathBuf,
}

impl AppInstance {
    pub fn read(path: &Path) -> Result<Self, ImportError> {
        let mut instance: Self =
            serde_json::from_slice(&std::fs::read(path.join("instance.json"))?)?;
        if instance.minecraft.trim().is_empty() || instance.name.trim().is_empty() {
            return Err(ImportError::Other(
                "FTB instance has no name or Minecraft version".into(),
            ));
        }
        instance.path = path.to_owned();
        Ok(instance)
    }

    fn loader(&self) -> Result<Loader, ImportError> {
        if !self.mod_loader.trim().is_empty() {
            if let Some((kind, version)) = self.mod_loader.split_once('-') {
                return loader(kind, version);
            }
        }
        let path = [
            self.path.join(".ftbapp/version.json"),
            self.path.join("version.json"),
        ]
        .into_iter()
        .find(|p| p.is_file());
        let Some(path) = path else {
            return Ok(Loader::None);
        };
        #[derive(Deserialize)]
        struct Version {
            targets: Vec<Target>,
        }
        let version: Version = serde_json::from_slice(&std::fs::read(path)?)?;
        for target in version.targets {
            if target.kind == "modloader"
                || matches!(
                    target.name.as_str(),
                    "forge" | "fabric" | "neoforge" | "quilt"
                )
            {
                return loader(&target.name, &target.version);
            }
        }
        Ok(Loader::None)
    }
}

pub fn discover_root() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    #[cfg(target_os = "macos")]
    let home = home.join("Library/Application Support");
    discover_in(&home.join(".ftba"))
}

fn discover_in(root: &Path) -> Option<PathBuf> {
    for relative in ["storage/settings.json", "bin/settings.json"] {
        let path = root.join(relative);
        if !path.is_file() {
            continue;
        }
        let value: serde_json::Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
        return value
            .get("instanceLocation")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(PathBuf::from);
    }
    None
}

pub fn scan(root: &Path) -> Result<Vec<AppInstance>, ImportError> {
    let mut instances = Vec::new();
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            if let Ok(instance) = AppInstance::read(&entry.path()) {
                instances.push(instance);
            }
        }
    }
    instances.sort_by_key(|i| i.name.to_lowercase());
    Ok(instances)
}

fn copy_game(source: &Path, dest: &Path) -> Result<(), ImportError> {
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            return Err(ImportError::Other(format!(
                "Refusing FTB symlink: {}",
                entry.path().display()
            )));
        } else if kind.is_dir() {
            copy_game(&entry.path(), &dest.join(entry.file_name()))?;
        } else if kind.is_file() {
            std::fs::copy(entry.path(), dest.join(entry.file_name()))?;
        } else {
            return Err(ImportError::Other(
                "Unsupported file in FTB instance".into(),
            ));
        }
    }
    Ok(())
}

fn import_icon(source: &Path, root: &Path, paths: &Paths) -> Result<Option<String>, ImportError> {
    let jpg = source.join("folder.jpg");
    let (bytes, extension) = if jpg.is_file() {
        (std::fs::read(jpg)?, "jpg")
    } else {
        let logo = source.join(".ftbapp/logo");
        if !logo.is_file() {
            return Ok(None);
        }
        let bytes = std::fs::read(logo)?;
        let Some((&kind, data)) = bytes.split_first() else {
            return Ok(None);
        };
        let extension = match kind {
            0 => "png",
            1 => "jpg",
            2 => "gif",
            3 => "webp",
            _ => return Ok(None),
        };
        (data.to_vec(), extension)
    };
    let icon = root.join(format!("ftb-icon.{extension}"));
    std::fs::write(&icon, bytes)?;
    if extension == "gif" {
        return Ok(None);
    }
    Ok(Some(crate::icons::add_icon(&paths.icons_dir(), &icon)?))
}

pub fn import(paths: &Paths, source: &Path) -> Result<ImportOutcome, ImportError> {
    let app = AppInstance::read(source)?;
    let loader = app.loader()?;
    let canonical = source.canonicalize()?;
    std::fs::create_dir_all(paths.instances_dir())?;
    if paths
        .instances_dir()
        .canonicalize()?
        .starts_with(&canonical)
    {
        return Err(ImportError::Other(
            "Import destination is inside the FTB source".into(),
        ));
    }
    let mut instance = InstanceList::create(paths, &app.name, "default", &app.minecraft)?;
    let result = (|| {
        copy_game(source, &instance.game_root())?;
        set_loader(&instance, &loader)?;
        instance.total_time_played = app.playtime / 1000;
        instance.save()?;
        write_provenance(
            &instance.root,
            app.id,
            &app.name,
            app.version_id,
            &app.version,
        )?;
        let path = instance.root.join("instance.cfg");
        let mut cfg = IniFile::load(&path)?;
        if let Some(args) = app.jvm_args.filter(|s| !s.trim().is_empty()) {
            cfg.set("OverrideJavaArgs", "true");
            cfg.set("JvmArgs", args);
        }
        cfg.save(&path)?;
        if let Some(key) = import_icon(source, &instance.root, paths)? {
            instance.icon_key = key;
            instance.save()?;
        }
        Ok(ImportOutcome {
            instance: instance.clone(),
            missing_files: Vec::new(),
            curseforge_key_issue: None,
        })
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&instance.root);
    }
    result
}
