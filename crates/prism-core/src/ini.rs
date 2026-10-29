//! Minimal INI handling for MultiMC/Prism-style config files.
//!
//! These files are flat `key=value` sections-less INIs (see
//! `settings/INISettingsObject.cpp` in PrismLauncher). Values may be quoted;
//! escape sequences are limited to what Qt's INI parser accepts.

use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct IniFile {
    pub entries: BTreeMap<String, String>,
}

impl IniFile {
    pub fn parse(text: &str) -> IniFile {
        let mut entries = BTreeMap::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with(';') || line.starts_with('#') || line.starts_with('[')
            {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            entries.insert(unquote(key.trim()), unescape_value(value.trim()));
        }
        IniFile { entries }
    }

    pub fn load(path: &Path) -> std::io::Result<IniFile> {
        let text = std::fs::read_to_string(path)?;
        Ok(IniFile::parse(&text))
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, self.serialize())
    }

    pub fn serialize(&self) -> String {
        let mut out = String::new();
        for (key, value) in &self.entries {
            out.push_str(&escape_key(key));
            out.push('=');
            out.push_str(&escape_value(value));
            out.push('\n');
        }
        out
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries.get(key).map(|s| s.as_str())
    }

    pub fn get_or(&self, key: &str, default: &str) -> String {
        self.get(key).unwrap_or(default).to_string()
    }

    pub fn set(&mut self, key: &str, value: impl Into<String>) {
        self.entries.insert(key.to_string(), value.into());
    }

    pub fn remove(&mut self, key: &str) {
        self.entries.remove(key);
    }
}

fn unquote(s: &str) -> String {
    let s = s.trim();
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

fn unescape_value(s: &str) -> String {
    s.replace("\\n", "\n").replace("\\t", "\t").replace("\\\\", "\\")
}

fn escape_key(s: &str) -> String {
    s.to_string()
}

fn escape_value(s: &str) -> String {
    let needs_quotes = s.is_empty()
        || s.starts_with(' ')
        || s.ends_with(' ')
        || s.contains(',')
        || s.contains(';')
        || s.contains('#')
        || s.contains('=')
        || s.contains('\n');
    let escaped = s.replace('\\', "\\\\").replace('\n', "\\n").replace('\t', "\\t");
    if needs_quotes {
        format!("\"{escaped}\"")
    } else {
        escaped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let mut ini = IniFile::default();
        ini.set("name", "My Instance");
        ini.set("totalTimePlayed", "1928");
        ini.set("notes", "line1\nline2");
        let text = ini.serialize();
        let parsed = IniFile::parse(&text);
        assert_eq!(parsed, ini);
        assert_eq!(parsed.get("name"), Some("My Instance"));
        assert_eq!(parsed.get("notes"), Some("line1\nline2"));
    }

    #[test]
    fn parses_instance_cfg() {
        let text = "General/Version=1.0\nInstanceType=OneSix\niconKey=default\nname=Test\n";
        let ini = IniFile::parse(text);
        assert_eq!(ini.get("InstanceType"), Some("OneSix"));
        assert_eq!(ini.get("name"), Some("Test"));
    }
}
