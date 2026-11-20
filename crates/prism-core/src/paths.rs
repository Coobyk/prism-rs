//! Launcher data directory layout, mirroring PrismLauncher's directory
//! structure (Application::m_root / data paths).

use std::fs::{self, File, OpenOptions};
use std::io;
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

    pub fn themes_dir(&self) -> PathBuf {
        self.root.join("themes")
    }

    pub fn icon_themes_dir(&self) -> PathBuf {
        self.root.join("iconthemes")
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

    fn metadata_lock_file(&self) -> io::Result<File> {
        let cache = self.cache_dir();
        fs::create_dir_all(&cache)?;
        OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(cache.join(".metadata.lock"))
    }

    pub fn lock_metadata_cache(&self) -> io::Result<File> {
        let lock = self.metadata_lock_file()?;
        fs2::FileExt::lock_shared(&lock)?;
        Ok(lock)
    }

    pub fn clear_metadata_cache(&self) -> io::Result<()> {
        let lock = self.metadata_lock_file()?;
        if let Err(error) = fs2::FileExt::try_lock_exclusive(&lock) {
            return if error.raw_os_error() == fs2::lock_contended_error().raw_os_error() {
                Err(io::Error::other(
                    "Metadata is in use. Wait for launcher preparation or component editing to finish, then try again.",
                ))
            } else {
                Err(error)
            };
        }
        match fs::remove_dir_all(self.metadata_dir()) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
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
            self.themes_dir(),
            self.icon_themes_dir(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clears_only_the_metadata_cache_and_only_when_not_in_use() {
        let root = tempfile::tempdir().unwrap();
        let paths = Paths {
            root: root.path().into(),
        };
        let metadata = paths.metadata_dir().join("net.minecraft");
        fs::create_dir_all(&metadata).unwrap();
        fs::write(metadata.join("1.20.1.json"), "cached version").unwrap();
        fs::create_dir_all(paths.libraries_dir()).unwrap();
        fs::write(paths.libraries_dir().join("library.jar"), "cached library").unwrap();
        fs::write(paths.global_config(), "settings").unwrap();

        let lock = paths.lock_metadata_cache().unwrap();
        assert!(paths
            .clear_metadata_cache()
            .unwrap_err()
            .to_string()
            .contains("in use"));
        assert!(metadata.join("1.20.1.json").is_file());
        drop(lock);

        paths.clear_metadata_cache().unwrap();
        paths.clear_metadata_cache().unwrap();
        assert!(!paths.metadata_dir().exists());
        assert!(paths.libraries_dir().join("library.jar").is_file());
        assert!(paths.global_config().is_file());
    }
}
