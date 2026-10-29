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
        // Explicit override, then auto-detect an existing PrismLauncher
        // install so its instances, icons and settings are used in place.
        if let Some(root) = std::env::var_os("PRISM_RS_DATA").map(PathBuf::from) {
            return Paths { root };
        }
        for candidate in self::prism_data_candidates() {
            if candidate.join("instances").is_dir() {
                return Paths { root: candidate };
            }
        }
        Paths { root: dirs_data().join("prismlauncher-rs") }
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

/// Known data-directory locations of PrismLauncher itself, in probe order.
fn prism_data_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        out.push(home.join(".local/share/PrismLauncher"));
        out.push(home.join(".local/share/prismlauncher"));
        out.push(home.join(".var/app/org.prismlauncher.PrismLauncher/data/PrismLauncher"));
        #[cfg(target_os = "macos")]
        out.push(home.join("Library/Application Support/PrismLauncher"));
    }
    if let Some(xdg) = std::env::var_os("XDG_DATA_HOME") {
        out.push(PathBuf::from(xdg).join("PrismLauncher"));
    }
    out
}
