//! Launcher data directory layout, mirroring PrismLauncher's directory
//! structure (Application::m_root / data paths).

use std::path::{Path, PathBuf};

/// Where the launcher keeps its data. Overridable with `PRISM_RS_DATA`.
#[derive(Debug, Clone)]
pub struct Paths {
    pub root: PathBuf,
}

impl Paths {
    pub fn new() -> Paths {
        let root = std::env::var_os("PRISM_RS_DATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                dirs_data().join("prismlauncher-rs")
            });
        Paths { root }
    }

    pub fn instances_dir(&self) -> PathBuf {
        self.root.join("instances")
    }

    pub fn icons_dir(&self) -> PathBuf {
        self.root.join("icons")
    }

    pub fn accounts_file(&self) -> PathBuf {
        self.root.join("accounts.json")
    }

    pub fn global_config(&self) -> PathBuf {
        self.root.join("prismlauncher.cfg")
    }

    pub fn cache_dir(&self) -> PathBuf {
        self.root.join("cache")
    }

    pub fn java_dir(&self) -> PathBuf {
        self.root.join("java")
    }

    pub fn logs_dir(&self) -> PathBuf {
        self.root.join("logs")
    }

    pub fn skins_dir(&self) -> PathBuf {
        self.root.join("skins")
    }

    pub fn metadata_dir(&self) -> PathBuf {
        self.cache_dir().join("metadata")
    }

    pub fn assets_dir(&self) -> PathBuf {
        self.cache_dir().join("assets")
    }

    pub fn libraries_dir(&self) -> PathBuf {
        self.cache_dir().join("libraries")
    }

    /// Create the directory skeleton on first run.
    pub fn ensure(&self) -> std::io::Result<()> {
        for dir in [
            self.instances_dir(),
            self.icons_dir(),
            self.cache_dir(),
            self.java_dir(),
            self.logs_dir(),
            self.skins_dir(),
        ] {
            std::fs::create_dir_all(dir)?;
        }
        Ok(())
    }
}

impl Default for Paths {
    fn default() -> Self {
        Self::new()
    }
}

fn dirs_data() -> PathBuf {
    // XDG data home or platform equivalent, without pulling in a full
    // directories crate.
    if let Some(xdg) = std::env::var_os("XDG_DATA_HOME") {
        return PathBuf::from(xdg);
    }
    if let Some(home) = std::env::var_os("HOME") {
        return Path::new(&home).join(".local/share");
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(home) = std::env::var_os("HOME") {
            return Path::new(&home).join("Library/Application Support");
        }
    }
    PathBuf::from(".")
}
