use prism_core::{ini::IniFile, settings::Settings};
use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::sync::LazyLock;

type Catalog = HashMap<String, String>;

macro_rules! catalog {
    ($file:literal) => {
        LazyLock::new(|| {
            serde_json::from_str(include_str!($file)).expect("invalid bundled translation catalog")
        })
    };
}

static DE: LazyLock<Catalog> = catalog!("translations/de.json");
static FR: LazyLock<Catalog> = catalog!("translations/fr.json");
static JA: LazyLock<Catalog> = catalog!("translations/ja.json");

pub const SETTING: &str = "Language";

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Locale {
    #[default]
    English,
    German,
    French,
    Japanese,
}

impl Locale {
    pub const ALL: [Self; 4] = [Self::English, Self::German, Self::French, Self::Japanese];

    pub fn code(self) -> &'static str {
        match self {
            Self::English => "en",
            Self::German => "de",
            Self::French => "fr",
            Self::Japanese => "ja",
        }
    }

    pub fn native_name(self) -> &'static str {
        match self {
            Self::English => "English",
            Self::German => "Deutsch",
            Self::French => "Français",
            Self::Japanese => "日本語",
        }
    }

    pub fn resolve(setting: Option<&str>) -> Self {
        #[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
        {
            let preferred = std::env::var("LANGUAGE").ok();
            let category = ["LC_ALL", "LC_MESSAGES", "LANG"]
                .into_iter()
                .filter_map(|name| std::env::var(name).ok())
                .find(|value| !value.trim().is_empty());
            Self::resolve_from(
                setting,
                preferred
                    .iter()
                    .flat_map(|values| values.split(':'))
                    .chain(category.as_deref()),
            )
        }
        #[cfg(not(all(unix, not(any(target_vendor = "apple", target_os = "android")))))]
        Self::resolve_from(setting, sys_locale::get_locales())
    }

    fn resolve_from(setting: Option<&str>, locales: impl IntoIterator<Item = impl AsRef<str>>) -> Self {
        Self::configured(setting).unwrap_or_else(|| {
            locales.into_iter()
                .find_map(|value| Self::parse(value.as_ref()))
                .unwrap_or_default()
        })
    }

    pub fn configured(setting: Option<&str>) -> Option<Self> {
        setting
            .filter(|value| !value.trim().is_empty())
            .map(|value| Self::parse(value).unwrap_or_default())
    }

    pub fn save_selection(selection: Option<Self>, path: &Path) -> io::Result<()> {
        let ini = match IniFile::load(path) {
            Ok(ini) => ini,
            Err(error) if error.kind() == io::ErrorKind::NotFound => IniFile::default(),
            Err(error) => return Err(error),
        };
        let mut settings = Settings { ini };
        if let Some(locale) = selection {
            settings.set(SETTING, locale.code());
        } else {
            settings.ini.remove(SETTING);
        }
        settings.save(path)
    }

    fn parse(value: &str) -> Option<Self> {
        let language = value.trim().split(['-', '_', '.', '@']).next()?;
        if language.eq_ignore_ascii_case("C") || language.eq_ignore_ascii_case("POSIX") {
            return Some(Self::English);
        }
        Self::ALL
            .into_iter()
            .find(|locale| language.eq_ignore_ascii_case(locale.code()))
    }

    pub fn text(self, source: &'static str) -> &'static str {
        let catalog = match self {
            Self::English => return source,
            Self::German => &DE,
            Self::French => &FR,
            Self::Japanese => &JA,
        };
        catalog.get(source).map(String::as_str).unwrap_or(source)
    }

    pub fn format(self, source: &'static str, values: &[&str]) -> String {
        let mut result = String::new();
        for part in self.text(source).split_inclusive('}') {
            let replacement = part
                .strip_suffix('}')
                .and_then(|part| part.rsplit_once('{'))
                .and_then(|(text, index)| Some((text, values.get(index.parse::<usize>().ok()?)?)));
            if let Some((text, value)) = replacement {
                result.push_str(text);
                result.push_str(value);
            } else {
                result.push_str(part);
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_stored_locale_variants() {
        for (input, expected) in [
            ("de_DE.UTF-8@euro", Locale::German),
            ("FR-ca", Locale::French),
            ("ja_JP", Locale::Japanese),
            ("en_GB", Locale::English),
            ("C.UTF-8", Locale::English),
            ("POSIX", Locale::English),
            ("es_MX", Locale::English),
        ] {
            assert_eq!(Locale::resolve(Some(input)), expected, "{input}");
        }
    }

    #[test]
    fn looks_up_catalogs_and_falls_back_to_source() {
        for locale in [Locale::German, Locale::French, Locale::Japanese] {
            assert_ne!(locale.text("Language"), "Language");
            assert_eq!(locale.text("Uncatalogued text"), "Uncatalogued text");
        }
        assert_eq!(Locale::English.text("Language"), "Language");
    }

    #[test]
    fn picks_first_supported_system_locale_unless_configured() {
        let system = ["nl-NL", "fr-CA", "de"];
        assert_eq!(Locale::resolve_from(None, system), Locale::French);
        assert_eq!(Locale::resolve_from(Some(""), system), Locale::French);
        assert_eq!(Locale::resolve_from(Some("ja"), system), Locale::Japanese);
        assert_eq!(Locale::resolve_from(Some("invalid"), system), Locale::English);
        assert_eq!(Locale::resolve_from(None, ["nl-NL"]), Locale::English);
    }

    #[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
    #[test]
    fn posix_locale_uses_first_nonempty_category() {
        const CHILD: &str = "PRISM_RS_TEST_POSIX_LOCALE";
        if std::env::var_os(CHILD).is_some() {
            assert_eq!(Locale::resolve(None), Locale::English);
            return;
        }
        for (all, messages) in [("es_ES", "fr_FR"), ("", "es_ES")] {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "i18n::tests::posix_locale_uses_first_nonempty_category"])
                .env(CHILD, "1")
                .env_remove("LANGUAGE")
                .env("LC_ALL", all)
                .env("LC_MESSAGES", messages)
                .env("LANG", "de_DE")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "LC_ALL={all:?} LC_MESSAGES={messages:?}: {}",
                String::from_utf8_lossy(&output.stdout)
            );
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn saves_only_language_and_does_not_overwrite_unreadable_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("prismlauncher.cfg");
        Locale::save_selection(Some(Locale::English), &path).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap().trim(), "Language=en");

        std::fs::write(&path, "[General]\nApplicationTheme=bright\nLanguage=en\n").unwrap();
        Locale::save_selection(Some(Locale::German), &path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("[General]\nApplicationTheme=bright\nLanguage=de"));

        std::fs::write(&path, [0xff]).unwrap();
        assert!(Locale::save_selection(Some(Locale::French), &path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), [0xff]);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn clearing_a_legacy_override_restores_system_locale_and_stays_unset_on_other_saves() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("prismlauncher.cfg");
        std::fs::write(&path, "ApplicationTheme=dark\nLanguage=en\n").unwrap();
        assert_eq!(
            Locale::resolve_from(Settings::load(&path).get(SETTING), ["de"]),
            Locale::English
        );

        Locale::save_selection(None, &path).unwrap();
        let mut settings = Settings::load(&path);
        assert_eq!(settings.get(SETTING), None);
        assert_eq!(settings.get("ApplicationTheme"), Some("dark"));
        assert_eq!(Locale::resolve_from(settings.get(SETTING), ["de"]), Locale::German);

        settings.set("MaxMemAlloc", "2048");
        settings.save(&path).unwrap();
        assert_eq!(IniFile::load(&path).unwrap().get(SETTING), None);
    }

    #[test]
    fn formats_translated_templates_without_replacing_inserted_text() {
        assert_eq!(
            Locale::Japanese.format("{0} • Running", &["Instance {0}"]),
            "Instance {0} • 実行中"
        );
        assert_eq!(Locale::English.format("{1}/{0}", &["a", "b"]), "b/a");
    }
}
