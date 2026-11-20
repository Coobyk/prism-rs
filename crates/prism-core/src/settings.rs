//! Global launcher settings: `prismlauncher.cfg` INI with typed accessors.
//! Mirrors `launcher/settings/` + Application's registered settings.

use crate::ini::IniFile;
use std::io::Write;
use std::path::Path;

#[derive(Clone)]
pub struct Settings {
    pub ini: IniFile,
}

#[derive(Clone, Copy)]
pub enum ApiKey {
    CurseForge,
    Imgur,
}

impl ApiKey {
    pub fn setting(self) -> &'static str {
        match self {
            Self::CurseForge => "FlameKeyOverride",
            Self::Imgur => "ImgurClientID",
        }
    }

    pub fn environment(self) -> &'static str {
        match self {
            Self::CurseForge => crate::modplatform::flame::API_KEY_ENV,
            Self::Imgur => crate::screenshots::IMGUR_CLIENT_ID_ENV,
        }
    }

    pub fn is_valid(self, value: &str) -> bool {
        !value.is_empty() && match self {
            Self::CurseForge => reqwest::header::HeaderValue::from_str(value).is_ok(),
            Self::Imgur => value.bytes().all(|byte| byte.is_ascii_alphanumeric()),
        }
    }

    fn from_environment(self) -> Option<String> {
        std::env::var(self.environment()).ok().filter(|value| !value.trim().is_empty())
    }

    pub fn has_environment_override(self) -> bool {
        self.from_environment().is_some()
    }

    fn resolve(self, settings: &Settings, env: Option<&str>) -> Option<String> {
        let value = env.map(str::trim).filter(|value| !value.is_empty())
            .or_else(|| settings.get(self.setting()).map(str::trim))?;
        self.is_valid(value).then(|| value.to_owned())
    }

    pub fn configured(self, settings: &Settings) -> Option<String> {
        self.resolve(settings, self.from_environment().as_deref())
    }

    pub fn load(self) -> Option<String> {
        self.configured(&Settings::load(&crate::Paths::new().global_config()))
    }
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
        ini.set("ApplicationTheme", "system");
        ini.set("IconTheme", "default");
        ini.set("JProfilerPath", "");
        ini.set("JProfilerPort", "42042");
        ini.set("MCEditPath", "");
        ini.set("JVisualVMPath", "");
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
        let path = if std::fs::symlink_metadata(path).is_ok_and(|meta| meta.is_symlink()) {
            path.canonicalize()?
        } else {
            path.to_path_buf()
        };
        let parent = path.parent().filter(|dir| !dir.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(parent)?;
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        file.write_all(self.ini.serialize().as_bytes())?;
        file.persist(path)?;
        Ok(())
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

    #[test]
    fn proxy_password_preserves_surrounding_whitespace_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("prismlauncher.cfg");
        let mut settings = Settings::default();
        for password in [" fixture ", " ", "\u{a0}fixture\u{a0}", "\tfixture\t"] {
            settings.set("ProxyPass", password);
            settings.save(&path).unwrap();
            assert_eq!(Settings::load(&path).get("ProxyPass"), Some(password));
        }
    }

    #[test]
    fn api_keys_use_environment_first_without_overwriting_saved_values() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("prismlauncher.cfg");
        let mut settings = Settings::default();
        for key in [ApiKey::CurseForge, ApiKey::Imgur] {
            settings.set(key.setting(), String::from("savedFixture"));
        }
        settings.save(&path).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(path.metadata().unwrap().permissions().mode() & 0o077, 0);
        }
        let settings = Settings::load(&path);
        for key in [ApiKey::CurseForge, ApiKey::Imgur] {
            assert!(key.resolve(&settings, Some("envFixture")).as_deref() == Some("envFixture"));
            assert!(key.resolve(&settings, Some(" ")).as_deref() == Some("savedFixture"));
            assert!(key.resolve(&settings, None).as_deref() == Some("savedFixture"));
            assert!(key.resolve(&settings, Some("bad\nvalue")).is_none());
        }
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
