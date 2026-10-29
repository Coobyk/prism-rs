//! Instance model: `instance.cfg` reading/writing, instance folder scanning
//! and the instance list. Mirrors PrismLauncher's `BaseInstance` +
//! `InstanceList` (minus Qt model machinery, which the UI layer provides).

use crate::ini::IniFile;
use crate::Paths;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// A Minecraft instance directory (a "OneSix"-style instance).
#[derive(Debug, Clone)]
pub struct Instance {
    /// Directory name; also the stable id used by the UI.
    pub id: String,
    /// Root of the instance (contains `instance.cfg`, `mmc-pack.json`, `.minecraft`).
    pub root: PathBuf,
    pub name: String,
    pub icon_key: String,
    pub notes: String,
    /// Seconds of total play time.
    pub total_time_played: u64,
    /// Unix timestamp (seconds) of the last launch, 0 = never.
    pub last_launch: u64,
    /// Group name from `instgroups.json`, if any.
    pub group: Option<String>,
}

impl Instance {
    /// Load a single instance from a directory. Returns None when the
    /// directory does not look like an instance (no `instance.cfg`).
    pub fn load(dir: &Path) -> Option<Instance> {
        let cfg_path = dir.join("instance.cfg");
        let cfg = IniFile::load(&cfg_path).ok()?;
        let id = dir.file_name()?.to_string_lossy().to_string();
        Some(Instance {
            id,
            name: cfg.get_or("name", &dir.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()),
            icon_key: cfg.get_or("iconKey", "default"),
            notes: cfg.get_or("notes", ""),
            total_time_played: cfg.get("totalTimePlayed").and_then(|v| v.parse().ok()).unwrap_or(0),
            last_launch: cfg.get("lastLaunchTime").and_then(|v| v.parse().ok()).unwrap_or(0),
            root: dir.to_path_buf(),
            group: None,
        })
    }

    /// The Minecraft game directory (`gameRoot`, `.minecraft` unless overridden).
    pub fn game_root(&self) -> PathBuf {
        self.root.join(".minecraft")
    }

    pub fn mods_dir(&self) -> PathBuf {
        self.game_root().join("mods")
    }

    pub fn save(&self) -> std::io::Result<()> {
        let mut cfg = IniFile::load(&self.root.join("instance.cfg")).unwrap_or_default();
        cfg.set("name", self.name.clone());
        cfg.set("iconKey", self.icon_key.clone());
        cfg.set("notes", self.notes.clone());
        cfg.set("totalTimePlayed", self.total_time_played.to_string());
        if self.last_launch > 0 {
            cfg.set("lastLaunchTime", self.last_launch.to_string());
        }
        cfg.set("InstanceType", "OneSix");
        cfg.save(&self.root.join("instance.cfg"))
    }
}

/// Persistent instance grouping, stored in `instgroups.json` inside the
/// instances directory. Mirrors PrismLauncher's format, including the legacy
/// "hidden" key.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InstanceGroups {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hidden: Option<bool>,
    #[serde(default)]
    pub groups: std::collections::BTreeMap<String, Group>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Group {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hidden: Option<bool>,
    #[serde(default)]
    pub instances: Vec<String>,
}

impl InstanceGroups {
    pub fn load(instances_dir: &Path) -> InstanceGroups {
        let path = instances_dir.join("instgroups.json");
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, instances_dir: &Path) -> std::io::Result<()> {
        let path = instances_dir.join("instgroups.json");
        std::fs::create_dir_all(instances_dir)?;
        std::fs::write(path, serde_json::to_string_pretty(self).unwrap_or_default())
    }

    pub fn group_of(&self, instance_id: &str) -> Option<String> {
        self.groups
            .iter()
            .find(|(_, g)| g.instances.iter().any(|i| i == instance_id))
            .map(|(name, _)| name.clone())
    }

    pub fn set_group(&mut self, instance_id: &str, group: Option<&str>) {
        for g in self.groups.values_mut() {
            g.instances.retain(|i| i != instance_id);
        }
        self.groups.retain(|_, g| !g.instances.is_empty());
        if let Some(name) = group {
            self.groups
                .entry(name.to_string())
                .or_default()
                .instances
                .push(instance_id.to_string());
        }
    }
}

/// The set of instances under the instances directory.
#[derive(Debug, Clone, Default)]
pub struct InstanceList {
    pub instances: Vec<Instance>,
    pub groups: InstanceGroups,
}

impl InstanceList {
    /// Scan the instances directory and load every instance.
    pub fn load(paths: &Paths) -> std::io::Result<InstanceList> {
        let dir = paths.instances_dir();
        std::fs::create_dir_all(&dir)?;
        let groups = InstanceGroups::load(&dir);
        let mut instances = Vec::new();
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            if let Some(mut inst) = Instance::load(&entry.path()) {
                inst.group = groups.group_of(&inst.id);
                instances.push(inst);
            }
        }
        instances.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        Ok(InstanceList { instances, groups })
    }

    pub fn get(&self, id: &str) -> Option<&Instance> {
        self.instances.iter().find(|i| i.id == id)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut Instance> {
        self.instances.iter_mut().find(|i| i.id == id)
    }

    /// Create a new instance directory with the given name and Minecraft
    /// version. Writes `instance.cfg` and `mmc-pack.json`.
    pub fn create(
        paths: &Paths,
        name: &str,
        icon_key: &str,
        mc_version: &str,
    ) -> Result<Instance, std::io::Error> {
        let dir = unique_dir_name(paths.instances_dir(), name);
        std::fs::create_dir_all(dir.join(".minecraft"))?;

        let mut inst = Instance {
            id: dir.file_name().unwrap().to_string_lossy().to_string(),
            root: dir.clone(),
            name: name.to_string(),
            icon_key: icon_key.to_string(),
            notes: String::new(),
            total_time_played: 0,
            last_launch: 0,
            group: None,
        };
        inst.save()?;

        // mmc-pack.json: component list with just the Minecraft component;
        // loaders are appended later.
        let pack = serde_json::json!({
            "components": [
                { "cachedVersion": mc_version, "important": true, "uid": "net.minecraft", "version": mc_version }
            ],
            "formatVersion": 1
        });
        std::fs::write(dir.join("mmc-pack.json"), serde_json::to_string_pretty(&pack).unwrap())?;

        Ok(inst)
    }

    /// Delete an instance by moving it to the system trash (falling back to a
    /// `.trash` folder next to the instances dir), mirroring Prism's
    /// trash-instead-of-delete behavior.
    pub fn remove(paths: &Paths, id: &str) -> std::io::Result<PathBuf> {
        let dir = paths.instances_dir().join(id);
        if !dir.exists() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("instance {id} not found"),
            ));
        }
        let trash = paths.instances_dir().join(".trash");
        std::fs::create_dir_all(&trash)?;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let dest = trash.join(format!("{id}.{stamp}"));
        std::fs::rename(&dir, &dest)?;
        Ok(dest)
    }
}

fn unique_dir_name(instances_dir: PathBuf, name: &str) -> PathBuf {
    let base: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '_' })
        .collect();
    let base = if base.is_empty() { "instance".to_string() } else { base };
    let mut candidate = instances_dir.join(&base);
    let mut n = 1;
    while candidate.exists() {
        n += 1;
        candidate = instances_dir.join(format!("{base}{n}"));
    }
    candidate
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_paths() -> (Paths, tempfile_guard::TempDir) {
        let dir = tempfile_guard::TempDir::new();
        let paths = Paths { root: dir.path.clone() };
        (paths, dir)
    }

    /// Tiny RAII temp dir helper (no external test deps).
    mod tempfile_guard {
        use std::path::PathBuf;

        pub struct TempDir {
            pub path: PathBuf,
        }

        impl TempDir {
            pub fn new() -> TempDir {
                let stamp = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos();
                let path = std::env::temp_dir().join(format!("prism-core-test-{stamp}"));
                std::fs::create_dir_all(&path).unwrap();
                TempDir { path }
            }
        }

        impl Drop for TempDir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.path);
            }
        }
    }

    #[test]
    fn create_load_roundtrip() {
        let (paths, _guard) = temp_paths();
        let inst = InstanceList::create(&paths, "My Test", "default", "1.20.1").unwrap();
        assert_eq!(inst.name, "My Test");

        let list = InstanceList::load(&paths).unwrap();
        assert_eq!(list.instances.len(), 1);
        assert_eq!(list.instances[0].name, "My Test");
        assert!(list.instances[0].root.join("mmc-pack.json").exists());
    }

    #[test]
    fn groups_roundtrip() {
        let (paths, _guard) = temp_paths();
        let inst = InstanceList::create(&paths, "Inst", "default", "1.20.1").unwrap();
        let mut groups = InstanceGroups::default();
        groups.set_group(&inst.id, Some("Skyblock"));
        groups.save(&paths.instances_dir()).unwrap();

        let loaded = InstanceGroups::load(&paths.instances_dir());
        assert_eq!(loaded.group_of(&inst.id).as_deref(), Some("Skyblock"));
        assert_eq!(loaded.groups["Skyblock"].instances, vec![inst.id.clone()]);

        groups.set_group(&inst.id, None);
        assert!(loaded.group_of(&inst.id).is_none() || groups.group_of(&inst.id).is_none());
    }
}
