//! Instance model: `instance.cfg` reading/writing, instance folder scanning
//! and the instance list. Mirrors PrismLauncher's `BaseInstance` +
//! `InstanceList` (minus Qt model machinery, which the UI layer provides).

use crate::horizon::{HorizonInstance, mark_link};
use crate::ini::IniFile;
use crate::Paths;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstanceKind {
    Java,
    Horizon { id: String, home: PathBuf, token: Option<uuid::Uuid> },
}

/// A Prism instance directory, holding a Java game or a link to a Horizon game.
#[derive(Debug, Clone)]
pub struct Instance {
    /// Directory name; also the stable id used by the UI.
    pub id: String,
    pub kind: InstanceKind,
    /// Prism directory containing `instance.cfg` and, for Java, the component pack and game.
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
    pub(crate) const SHORTCUT_TOKEN_FILE: &str = ".prism-rs-shortcut-token";

    pub fn shortcut_token(&self) -> std::io::Result<String> {
        std::fs::read_to_string(self.root.join(Self::SHORTCUT_TOKEN_FILE))
    }

    pub fn ensure_shortcut_token(&self) -> std::io::Result<String> {
        use std::io::{ErrorKind, Write};
        match self.shortcut_token() {
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            result => return result,
        }
        let token = uuid::Uuid::new_v4().to_string();
        let mut file = tempfile::NamedTempFile::new_in(&self.root)?;
        file.write_all(token.as_bytes())?;
        match file.persist_noclobber(self.root.join(Self::SHORTCUT_TOKEN_FILE)) {
            Ok(_) => Ok(token),
            Err(error) if error.error.kind() == ErrorKind::AlreadyExists => self.shortcut_token(),
            Err(error) => Err(error.error),
        }
    }

    /// Load a single instance from a directory. Returns None when the
    /// directory does not look like an instance (no `instance.cfg`).
    pub fn load(dir: &Path) -> Option<Instance> {
        let cfg_path = dir.join("instance.cfg");
        let cfg = IniFile::load(&cfg_path).ok()?;
        let id = dir.file_name()?.to_string_lossy().to_string();
        let kind = if cfg.get("InstanceType") == Some("Horizon") {
            let home = PathBuf::from(cfg.get("HorizonGameHome")?);
            let id = cfg.get("HorizonInstanceId")?;
            if !home.is_absolute() || id.is_empty() {
                return None;
            }
            let token = cfg.get("HorizonLinkToken").and_then(|value| value.parse().ok());
            InstanceKind::Horizon { id: id.to_string(), home, token }
        } else {
            InstanceKind::Java
        };
        Some(Instance {
            id,
            kind,
            name: cfg.get_or("name", &dir.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()),
            icon_key: cfg.get_or("iconKey", "default"),
            notes: cfg.get_or("notes", ""),
            total_time_played: cfg.get("totalTimePlayed").and_then(|v| v.parse().ok()).unwrap_or(0),
            last_launch: cfg.get("lastLaunchTime").and_then(|v| v.parse().ok()).unwrap_or(0),
            root: dir.to_path_buf(),
            group: None,
        })
    }

    /// The Minecraft game directory. PrismLauncher (and MultiMC) use
    /// `minecraft` on Linux/Windows; older layouts may have `.minecraft`.
    pub fn game_root(&self) -> PathBuf {
        if let InstanceKind::Horizon { home, .. } = &self.kind {
            return home.clone();
        }
        let mc = self.root.join("minecraft");
        if mc.is_dir() {
            mc
        } else {
            self.root.join(".minecraft")
        }
    }

    pub fn mods_dir(&self) -> PathBuf {
        self.game_root().join(if self.is_horizon() { "horizon-mods" } else { "mods" })
    }

    pub fn is_horizon(&self) -> bool {
        matches!(self.kind, InstanceKind::Horizon { .. })
    }

    pub fn save_horizon_home(&mut self, new_home: PathBuf) -> std::io::Result<()> {
        let InstanceKind::Horizon { id, token, .. } = &self.kind else {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "Not a Horizon link"));
        };
        self.edit_config(|cfg| {
            if cfg.get("HorizonInstanceId") != Some(id.as_str())
                || cfg.get("HorizonLinkToken").and_then(|value| value.parse().ok()) != *token
            {
                return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "Horizon link changed"));
            }
            cfg.set("HorizonGameHome", new_home.to_string_lossy().into_owned());
            Ok(())
        })?;
        if let InstanceKind::Horizon { home, .. } = &mut self.kind {
            *home = new_home;
        }
        Ok(())
    }

    pub fn rename(&mut self, name: String) -> std::io::Result<()> {
        self.edit_config(|cfg| {
            cfg.set("name", name.as_str());
            Ok(())
        })?;
        self.name = name;
        Ok(())
    }

    fn edit_config(
        &self,
        edit: impl FnOnce(&mut IniFile) -> std::io::Result<()>,
    ) -> std::io::Result<()> {
        use std::io::Write;
        let lock = std::fs::File::options()
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.root.with_file_name(".config-write.lock"))?;
        fs2::FileExt::lock_exclusive(&lock)?;
        let path = self.root.join("instance.cfg");
        let mut cfg = IniFile::load(&path)?;
        edit(&mut cfg)?;
        let mut file = tempfile::NamedTempFile::new_in(&self.root)?;
        file.write_all(cfg.serialize().as_bytes())?;
        file.persist(path).map_err(|error| error.error)?;
        Ok(())
    }

    pub fn worlds(&self) -> Vec<String> {
        let mut worlds: Vec<_> = std::fs::read_dir(self.game_root().join("saves"))
            .into_iter()
            .flatten()
            .flatten()
            .filter(|entry| entry.path().is_dir() && entry.path().join("level.dat").is_file())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        worlds.sort_by_key(|name| (name.to_lowercase(), name.clone()));
        worlds
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
        match &self.kind {
            InstanceKind::Java => cfg.set("InstanceType", "OneSix"),
            InstanceKind::Horizon { id, home, token } => {
                cfg.set("InstanceType", "Horizon");
                cfg.set("HorizonInstanceId", id.as_str());
                cfg.set("HorizonGameHome", home.to_string_lossy().into_owned());
                if let Some(token) = token {
                    cfg.set("HorizonLinkToken", token.to_string());
                }
            }
        }
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
        std::fs::create_dir_all(dir.join("minecraft"))?;

        let inst = Instance {
            id: dir.file_name().unwrap().to_string_lossy().to_string(),
            kind: InstanceKind::Java,
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

    pub fn link_horizon(paths: &Paths, horizon: HorizonInstance) -> std::io::Result<Instance> {
        let token = mark_link(&horizon.home)?;
        let dir = unique_dir_name(paths.instances_dir(), &horizon.name);
        let inst = Instance {
            id: dir.file_name().unwrap().to_string_lossy().to_string(),
            kind: InstanceKind::Horizon { id: horizon.id, home: horizon.home, token: Some(token) },
            root: dir,
            name: horizon.name,
            icon_key: "default".to_string(),
            notes: String::new(),
            total_time_played: 0,
            last_launch: 0,
            group: None,
        };
        inst.save()?;
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

/// Read the loader component from an instance's mmc-pack.json.
pub fn loader_of(inst: &Instance) -> crate::loaders::Loader {
    let pack_path = inst.root.join("mmc-pack.json");
    let Ok(text) = std::fs::read_to_string(&pack_path) else {
        return crate::loaders::Loader::None;
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
        return crate::loaders::Loader::None;
    };
    for comp in v["components"].as_array().unwrap_or(&vec![]) {
        let uid = comp["uid"].as_str().unwrap_or_default();
        let ver = comp["version"].as_str().unwrap_or_default().to_string();
        match uid {
            "net.fabricmc.fabric-loader" => return crate::loaders::Loader::Fabric { version: ver },
            "org.quiltmc.quilt-loader" => return crate::loaders::Loader::Quilt { version: ver },
            "net.minecraftforge" => return crate::loaders::Loader::Forge { version: ver },
            "net.neoforged" => return crate::loaders::Loader::NeoForge { version: ver },
            _ => {}
        }
    }
    crate::loaders::Loader::None
}

/// Set (or clear) the loader component in an instance's mmc-pack.json.
pub fn set_loader(inst: &Instance, loader: &crate::loaders::Loader) -> std::io::Result<()> {
    let pack_path = inst.root.join("mmc-pack.json");
    let mut pack: serde_json::Value = std::fs::read_to_string(&pack_path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or(serde_json::json!({"components": [], "formatVersion": 1}));

    let components = pack["components"]
        .as_array_mut()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "bad mmc-pack.json"))?;
    let loader_uids = [
        "net.fabricmc.fabric-loader",
        "org.quiltmc.quilt-loader",
        "net.minecraftforge",
        "net.neoforged",
    ];
    components.retain(|c| !loader_uids.contains(&c["uid"].as_str().unwrap_or_default()));

    let uid = loader.uid();
    if let Some(version) = loader.version() {
        components.push(serde_json::json!({
            "uid": uid,
            "version": version,
            "important": true
        }));
    }
    std::fs::write(pack_path, serde_json::to_string_pretty(&pack).unwrap_or_default())?;
    Ok(())
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
    fn worlds_filter_and_sort() {
        let (paths, _guard) = temp_paths();
        let inst = InstanceList::create(&paths, "Worlds", "default", "1.20.4").unwrap();
        assert!(inst.worlds().is_empty());
        let saves = inst.game_root().join("saves");
        for name in ["zebra", "Alpha", "empty"] {
            std::fs::create_dir_all(saves.join(name)).unwrap();
            if name != "empty" {
                std::fs::write(saves.join(name).join("level.dat"), []).unwrap();
            }
        }
        std::fs::write(saves.join("file"), []).unwrap();
        assert_eq!(inst.worlds(), ["Alpha", "zebra"]);
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
    fn horizon_link_roundtrip_and_removal_leave_game_files_alone() {
        let (paths, _guard) = temp_paths();
        let home = paths.root.join("Horizon game");
        std::fs::create_dir(&home).unwrap();
        std::fs::write(home.join("options.txt"), "").unwrap();
        let horizon = serde_json::from_value(serde_json::json!({
            "id": "native", "name": "Native", "home": home, "java": null
        })).unwrap();
        let inst = InstanceList::link_horizon(&paths, horizon).unwrap();
        let mut reloaded = Instance::load(&inst.root).unwrap();
        assert!(matches!(&reloaded.kind, InstanceKind::Horizon { id, home: linked_home, token: Some(_) }
            if id == "native" && linked_home == &home));
        assert_eq!(reloaded.kind, inst.kind);
        assert_eq!(reloaded.game_root(), home);
        assert!(!reloaded.root.join("mmc-pack.json").exists());
        reloaded.name = "New Prism name".into();
        reloaded.save().unwrap();
        assert_eq!(Instance::load(&inst.root).unwrap().kind, reloaded.kind);

        InstanceList::remove(&paths, &inst.id).unwrap();
        assert!(!inst.root.exists());
        assert!(home.join("options.txt").is_file());
    }

    #[test]
    fn moving_a_horizon_link_keeps_the_old_home_until_save_succeeds() {
        let (paths, _guard) = temp_paths();
        let old_home = paths.root.join("original");
        let new_home = paths.root.join("moved");
        std::fs::create_dir(&old_home).unwrap();
        let horizon = serde_json::from_value(serde_json::json!({
            "id": "native", "name": "Native", "home": old_home, "java": null
        })).unwrap();
        let mut instance = InstanceList::link_horizon(&paths, horizon).unwrap();
        let mut stale_gui_instance = instance.clone();
        let cfg = instance.root.join("instance.cfg");
        let backup = cfg.with_extension("bak");
        std::fs::rename(&cfg, &backup).unwrap();
        std::fs::create_dir(&cfg).unwrap();

        assert!(instance.save_horizon_home(new_home.clone()).is_err());
        assert_eq!(instance.game_root(), old_home);

        std::fs::remove_dir(&cfg).unwrap();
        std::fs::rename(&backup, &cfg).unwrap();
        instance.save_horizon_home(new_home.clone()).unwrap();
        stale_gui_instance.rename("Renamed".into()).unwrap();
        assert_eq!(instance.game_root(), new_home);
        let stored = Instance::load(&instance.root).unwrap();
        assert_eq!(stored.game_root(), new_home);
        assert_eq!(stored.name, "Renamed");
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
