//! PrismLauncher palettes and custom theme discovery. Qt stylesheets are unsupported.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color(pub u32);

impl Color {
    /// Qt uses ARGB, not RGBA, for eight-digit color names.
    pub fn parse(value: &str) -> Option<Self> {
        let hex = value.strip_prefix('#')?;
        if !matches!(hex.len(), 6 | 8) || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let value = u32::from_str_radix(hex, 16).ok()?;
        Some(Self(if hex.len() == 6 {
            value | 0xff000000
        } else {
            value
        }))
    }

    pub fn is_light(self) -> bool {
        let channel = |shift: u32| {
            let v = ((self.0 >> shift) & 255) as f32 / 255.;
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(16) + 0.7152 * channel(8) + 0.0722 * channel(0) > 0.179
    }
}

#[derive(Debug, Clone)]
pub struct ApplicationTheme {
    pub id: String,
    pub name: String,
    pub colors: BTreeMap<String, Color>,
    pub fade_amount: f32,
}

impl ApplicationTheme {
    pub fn builtin(light: bool) -> Self {
        let values = if light {
            [
                0xf5f6f8, 0x20242b, 0xffffff, 0xedeff3, 0xffffff, 0x20242b, 0x20242b, 0xedeff3,
                0x20242b, 0xb42332, 0x1764a0, 0x2867a3, 0xffffff, 0x647080,
            ]
        } else {
            [
                0x232323, 0xdddddd, 0x1a1a1a, 0x242424, 0x2b2b2b, 0xdddddd, 0xdddddd, 0x2b2b2b,
                0xdddddd, 0xff8888, 0x8fb4d9, 0x3d5a80, 0xffffff, 0x888888,
            ]
        };
        let keys = [
            "Window",
            "WindowText",
            "Base",
            "AlternateBase",
            "ToolTipBase",
            "ToolTipText",
            "Text",
            "Button",
            "ButtonText",
            "BrightText",
            "Link",
            "Highlight",
            "HighlightedText",
            "fadeColor",
        ];
        Self {
            id: if light { "bright" } else { "dark" }.into(),
            name: if light { "Bright" } else { "Dark" }.into(),
            colors: keys
                .into_iter()
                .zip(values)
                .map(|(k, v)| (k.into(), Color(v | 0xff000000)))
                .collect(),
            fade_amount: 0.5,
        }
    }

    pub fn color(&self, name: &str) -> Color {
        self.colors.get(name).copied().unwrap_or(Color(0xff888888))
    }

    pub fn parse(id: &str, json: &str) -> Result<Self, String> {
        #[derive(Deserialize)]
        struct Document {
            name: Option<String>,
            #[serde(default)]
            colors: BTreeMap<String, serde_json::Value>,
        }
        let doc: Document = serde_json::from_str(json).map_err(|e| e.to_string())?;
        let mut parsed = BTreeMap::new();
        let mut fade_amount = None;
        for (key, value) in doc.colors {
            if key == "fadeAmount" {
                let amount = value
                    .as_f64()
                    .filter(|v| (0.0..=1.0).contains(v))
                    .ok_or_else(|| "Invalid fadeAmount".to_string())?;
                fade_amount = Some(amount as f32);
            } else {
                let color = value
                    .as_str()
                    .and_then(Color::parse)
                    .ok_or_else(|| format!("Invalid color {key}: {value}"))?;
                parsed.insert(key, color);
            }
        }
        let light = parsed.get("Window").is_some_and(|c| c.is_light());
        let mut theme = Self::builtin(light);
        theme.id = id.into();
        theme.name = doc
            .name
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| id.into());
        theme.colors.extend(parsed);
        theme.fade_amount = fade_amount.unwrap_or(theme.fade_amount);
        Ok(theme)
    }
}

#[derive(Debug, Clone)]
pub struct IconTheme {
    pub id: String,
    pub name: String,
    pub directory: PathBuf,
}

#[derive(Debug, Default)]
pub struct ThemeCatalog {
    pub applications: Vec<ApplicationTheme>,
    pub icons: Vec<IconTheme>,
    pub warnings: Vec<String>,
}

impl ThemeCatalog {
    pub fn load(paths: &crate::Paths) -> Self {
        let mut system = ApplicationTheme::builtin(false);
        system.id = "system".into();
        system.name = "System".into();
        let mut catalog = Self {
            applications: vec![
                system,
                ApplicationTheme::builtin(false),
                ApplicationTheme::builtin(true),
            ],
            ..Self::default()
        };
        for dir in directories(&paths.themes_dir(), &mut catalog.warnings) {
            let id = dir.file_name().unwrap().to_string_lossy().into_owned();
            if matches!(id.as_str(), "system" | "dark" | "bright") {
                catalog
                    .warnings
                    .push(format!("{}: reserved theme id", dir.display()));
                continue;
            }
            let file = dir.join("theme.json");
            if !file.exists() {
                continue;
            }
            match std::fs::read_to_string(&file)
                .map_err(|e| e.to_string())
                .and_then(|json| ApplicationTheme::parse(&id, &json))
            {
                Ok(theme) => catalog.applications.push(theme),
                Err(error) => catalog
                    .warnings
                    .push(format!("{}: {error}", file.display())),
            }
        }
        for directory in directories(&paths.icon_themes_dir(), &mut catalog.warnings) {
            let id = directory
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned();
            let mut name = id.clone();
            if let Ok(index) = std::fs::read_to_string(directory.join("index.theme")) {
                let mut in_theme = false;
                for line in index.lines().map(str::trim) {
                    if line.starts_with('[') {
                        in_theme = line == "[Icon Theme]";
                    }
                    if in_theme {
                        if let Some(value) =
                            line.strip_prefix("Name=").filter(|n| !n.trim().is_empty())
                        {
                            name = value.trim().into();
                        }
                    }
                }
            }
            catalog.icons.push(IconTheme {
                id,
                name,
                directory,
            });
        }
        catalog
    }

    pub fn resolve(&self, id: &str, system_light: bool) -> ApplicationTheme {
        if id == "system" {
            return ApplicationTheme::builtin(system_light);
        }
        self.applications
            .iter()
            .find(|t| t.id == id)
            .cloned()
            .unwrap_or_else(|| ApplicationTheme::builtin(system_light))
    }
}

fn directories(root: &Path, warnings: &mut Vec<String>) -> Vec<PathBuf> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return vec![],
        Err(e) => {
            warnings.push(format!("{}: {e}", root.display()));
            return vec![];
        }
    };
    let mut dirs = Vec::new();
    for entry in entries {
        match entry {
            Ok(entry) if entry.path().is_dir() => dirs.push(entry.path()),
            Ok(_) => (),
            Err(e) => warnings.push(format!("{}: {e}", root.display())),
        }
    }
    dirs.sort();
    dirs
}

pub fn icon_path(theme_dir: &Path, name: &str) -> Option<PathBuf> {
    let mut components = Path::new(name).components();
    if !matches!(components.next(), Some(Component::Normal(_))) || components.next().is_some() {
        return None;
    }
    let mut dirs = vec![theme_dir.to_path_buf(), theme_dir.join("scalable")];
    dirs.extend(directories(theme_dir, &mut Vec::new()));
    dirs.into_iter()
        .flat_map(|dir| {
            [
                dir.join(format!("{name}.svg")),
                dir.join(format!("{name}.png")),
            ]
        })
        .find(|path| path.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn themes_color_names() {
        assert_eq!(Color::parse("#123AbC"), Some(Color(0xff123abc)));
        assert_eq!(Color::parse("#80123456"), Some(Color(0x80123456)));
        for bad in ["red", "#fff", "#gg1122", "123456", "#123456789"] {
            assert_eq!(Color::parse(bad), None);
        }
        assert!(Color(0xffffffff).is_light());
        assert!(!Color(0xff232323).is_light());
    }

    #[test]
    fn themes_parsing_and_fallbacks() {
        let dark = ApplicationTheme::parse("mine", "{}").unwrap();
        assert_eq!(dark.name, "mine");
        assert_eq!(
            dark.color("Text"),
            ApplicationTheme::builtin(false).color("Text")
        );
        let bright = ApplicationTheme::parse("mine", r##"{"name":"Custom","colors":{"Window":"#ffffff","Link":"#80123456","fadeAmount":0.3},"qssFilePath":"x.qss"}"##).unwrap();
        assert_eq!(
            bright.color("Text"),
            ApplicationTheme::builtin(true).color("Text")
        );
        assert_eq!(bright.color("Link"), Color(0x80123456));
        assert_eq!(bright.fade_amount, 0.3);
        let colors: BTreeMap<_, _> = ApplicationTheme::builtin(false)
            .colors
            .keys()
            .map(|key| (key.clone(), "#123456"))
            .collect();
        let json = serde_json::json!({"name": "All roles", "colors": colors}).to_string();
        let all = ApplicationTheme::parse("all", &json).unwrap();
        assert!(all.colors.values().all(|c| *c == Color(0xff123456)));
        for bad in [
            "{",
            r#"{"colors":{"Text":"oops"}}"#,
            r#"{"colors":{"fadeAmount":2}}"#,
        ] {
            assert!(ApplicationTheme::parse("bad", bad).is_err());
        }
    }

    #[test]
    fn themes_listing_and_invalid_files() {
        let dir = tempfile::tempdir().unwrap();
        let paths = crate::Paths {
            root: dir.path().into(),
        };
        assert_eq!(ThemeCatalog::load(&paths).applications.len(), 3);
        paths.ensure().unwrap();
        for (id, contents) in [("valid", "{}"), ("invalid", "{")] {
            let folder = paths.themes_dir().join(id);
            std::fs::create_dir_all(&folder).unwrap();
            std::fs::write(folder.join("theme.json"), contents).unwrap();
        }
        let icons = paths.icon_themes_dir().join("test");
        std::fs::create_dir_all(icons.join("32x32")).unwrap();
        std::fs::create_dir_all(icons.join("scalable")).unwrap();
        std::fs::write(
            icons.join("index.theme"),
            "[Other]\nName=Wrong\n[Icon Theme]\nName=Test Icons\n",
        )
        .unwrap();
        for file in ["root.png", "scalable/vector.svg", "32x32/sized.png"] {
            std::fs::write(icons.join(file), "").unwrap();
        }
        let catalog = ThemeCatalog::load(&paths);
        assert_eq!(catalog.applications.len(), 4);
        assert_eq!(catalog.warnings.len(), 1);
        assert_eq!(catalog.icons[0].name, "Test Icons");
        std::fs::create_dir_all(paths.icon_themes_dir().join("unnamed")).unwrap();
        assert_eq!(ThemeCatalog::load(&paths).icons[1].name, "unnamed");
        for name in ["root", "vector", "sized"] {
            assert!(icon_path(&icons, name).is_some());
        }
        assert!(icon_path(&icons, "missing").is_none());
        assert!(icon_path(&icons, "../root").is_none());
        assert_eq!(catalog.resolve("missing", true).id, "bright");
        assert_eq!(catalog.resolve("system", false).id, "dark");
        let mut settings = crate::settings::Settings::default();
        assert_eq!(settings.get("ApplicationTheme"), Some("system"));
        assert_eq!(settings.get("IconTheme"), Some("default"));
        settings.set("ApplicationTheme", "valid");
        settings.set("IconTheme", "test");
        settings.save(&paths.global_config()).unwrap();
        let settings = crate::settings::Settings::load(&paths.global_config());
        assert_eq!(settings.get("ApplicationTheme"), Some("valid"));
        assert_eq!(settings.get("IconTheme"), Some("test"));
    }
}
