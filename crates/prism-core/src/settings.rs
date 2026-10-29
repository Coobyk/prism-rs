//! Global launcher settings: `prismlauncher.cfg` INI with typed accessors.
//! Mirrors `launcher/settings/` + Application's registered settings.

use crate::ini::IniFile;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct Settings {
    pub ini: IniFile,
}

impl Default for Settings {
    fn default() -> Self {
        let mut ini = IniFile::default();
        // Defaults mirroring PrismLauncher's LaunchPreferences.
        ini.set("MinMemAlloc", "512");
        ini.set("MaxMemAlloc", "4096");
        ini.set("MinecraftWinWidth", "854");
        ini.set("MinecraftWinHeight", "480");
        ini.set("LaunchMaximized", "false");
        ini.set("AutoDetectJava", "true");
        ini.set("AutomaticJava", "true");
        ini.set("CloseAfterLaunch", "false");
        ini.set("ShowConsole", "true");
        ini.set("ConsoleMaxLines", "100000");
        ini.set("Language", "en");
        ini.set("Theme", "dark");
        Settings { ini }
    }
}

impl Settings {
    pub fn load(path: &Path) -> Settings {
        let mut s = Settings::default();
        if let Ok(loaded) = IniFile::load(path) {
            for (k, v) in loaded.entries {
                s.ini.set(&k, v);
            }
        }
        s
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        self.ini.save(path)
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.ini.get(key)
    }

    pub fn get_or(&self, key: &str, default: &str) -> String {
        self.ini.get_or(key, default)
    }

    pub fn get_bool(&self, key: &str, default: bool) -> bool {
        self.ini
            .get(key)
            .map(|v| v == "true" || v == "1")
            .unwrap_or(default)
    }

    pub fn get_int(&self, key: &str, default: i64) -> i64 {
        self.ini.get(key).and_then(|v| v.parse().ok()).unwrap_or(default)
    }

    pub fn set(&mut self, key: &str, value: impl Into<String>) {
        self.ini.set(key, value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_overrides() {
        let s = Settings::default();
        assert_eq!(s.get_int("MaxMemAlloc", 0), 4096);
        assert!(s.get_bool("AutoDetectJava", false));

        let mut s2 = Settings::default();
        s2.set("MaxMemAlloc", "8192");
        let text = s2.ini.serialize();
        let s3 = Settings::load_from_text(&text);
        assert_eq!(s3.get_int("MaxMemAlloc", 0), 8192);
        // Defaults survive a reload of a file that lacks them.
        assert_eq!(s3.get_int("MinMemAlloc", 0), 512);
    }
}

impl Settings {
    fn load_from_text(text: &str) -> Settings {
        let mut s = Settings::default();
        for (k, v) in IniFile::parse(text).entries {
            s.ini.set(&k, v);
        }
        s
    }
}
