//! INI handling for MultiMC/Prism-style config files (`instance.cfg`,
//! `prismlauncher.cfg`).
//!
//! These files are QSettings-format INIs: optionally sectioned
//! (`[General]`, `[UI]`, …). Reading exposes a flat key→value view (keys are
//! unique across sections in these files); writing preserves the original
//! section structure, updating keys in place and appending new ones to the
//! configured default section.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::path::Path;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct IniFile {
    /// Flat view: key → value (across all sections).
    pub entries: BTreeMap<String, String>,
    /// Preserved structure in file order; `""` = keys before any header.
    sections: Vec<(String, Vec<(String, String)>)>,
    /// Where `set` appends brand-new keys. Empty string = no section header.
    default_section: String,
}

impl IniFile {
    pub fn parse(text: &str) -> IniFile {
        let mut sections: Vec<(String, Vec<(String, String)>)> = vec![(String::new(), Vec::new())];
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with(';') || trimmed.starts_with('#') {
                continue;
            }
            if trimmed.starts_with('[') && trimmed.ends_with(']') {
                sections.push((trimmed[1..trimmed.len() - 1].to_string(), Vec::new()));
                continue;
            }
            let Some((key, value)) = trimmed.split_once('=') else {
                continue;
            };
            let key = unquote(key.trim());
            let value = unescape_value(&unquote(value.trim()));
            sections.last_mut().unwrap().1.push((key.clone(), value.clone()));
        }

        let mut entries = BTreeMap::new();
        let mut key_section = BTreeMap::new();
        for (name, kvs) in &sections {
            for (k, v) in kvs {
                if !entries.contains_key(k) {
                    entries.insert(k.clone(), v.clone());
                    key_section.insert(k.clone(), name.clone());
                }
            }
        }

        IniFile {
            entries,
            sections,
            default_section: String::new(),
        }
    }

    /// Keys added with `set` go into this section (e.g. `General` when
    /// mirroring Prism's own files).
    pub fn with_default_section(mut self, section: &str) -> IniFile {
        self.default_section = section.to_string();
        self
    }

    pub fn load(path: &Path) -> std::io::Result<IniFile> {
        let text = std::fs::read_to_string(path)?;
        Ok(IniFile::parse(&text))
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        let parent = path.parent().filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)?;
        let permissions = match fs::metadata(path) {
            Ok(metadata) => Some(metadata.permissions()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        let temp = parent.join(format!(".prism-ini-{}.tmp", uuid::Uuid::new_v4()));
        let mut file = fs::OpenOptions::new().write(true).create_new(true).open(&temp)?;
        let result = (|| {
            if let Some(permissions) = permissions {
                file.set_permissions(permissions)?;
            }
            file.write_all(self.serialize().as_bytes())?;
            file.sync_all()
        })();
        drop(file);
        let result = result.and_then(|()| fs::rename(&temp, path));
        if result.is_err() {
            let _ = fs::remove_file(&temp);
        }
        result
    }

    pub fn serialize(&self) -> String {
        // Work on a copy of the section list so new keys can be appended
        // without touching the caller.
        let mut sections = self.sections.clone();
        for (key, value) in &self.entries {
            let already = sections
                .iter()
                .any(|(_, kvs)| kvs.iter().any(|(k, _)| k == key));
            if already {
                continue;
            }
            let target = self.default_section.clone();
            match sections.iter_mut().find(|(name, _)| *name == target) {
                Some((_, kvs)) => kvs.push((key.clone(), value.clone())),
                None => sections.push((target, vec![(key.clone(), value.clone())])),
            }
        }

        let mut out = String::new();
        for (name, kvs) in &sections {
            if !name.is_empty() {
                out.push_str(&format!("[{name}]\n"));
            }
            for (k, v) in kvs {
                out.push_str(&escape_key(k));
                out.push('=');
                out.push_str(&escape_value(v));
                out.push('\n');
            }
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
        let value = value.into();
        // Keep the preserved section structure in step: update the key where
        // it already lives, otherwise append it to the default section.
        let mut updated = false;
        for (_, kvs) in self.sections.iter_mut() {
            for (k, v) in kvs.iter_mut() {
                if k == key {
                    *v = value.clone();
                    updated = true;
                }
            }
        }
        if !updated {
            let target = self.default_section.clone();
            match self
                .sections
                .iter_mut()
                .find(|(name, _)| *name == target)
            {
                Some((_, kvs)) => kvs.push((key.to_string(), value.clone())),
                None => self
                    .sections
                    .push((target, vec![(key.to_string(), value.clone())])),
            }
        }
        self.entries.insert(key.to_string(), value);
    }

    pub fn remove(&mut self, key: &str) {
        self.entries.remove(key);
        for (_, kvs) in self.sections.iter_mut() {
            kvs.retain(|(k, _)| k != key);
        }
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
    let mut value = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        let decoded = if c == '\\' {
            match chars.peek() {
                Some('n') => Some('\n'),
                Some('t') => Some('\t'),
                Some('\\') => Some('\\'),
                _ => None,
            }
        } else {
            None
        };
        if let Some(decoded) = decoded {
            chars.next();
            value.push(decoded);
        } else {
            value.push(c);
        }
    }
    value
}

fn escape_key(s: &str) -> String {
    s.to_string()
}

fn escape_value(s: &str) -> String {
    // Match QSettings: quote only when necessary (special characters, or
    // leading/trailing whitespace). Values with interior spaces stay bare,
    // like `name=26.3 Coobyk`.
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
    fn saves_replace_the_file_and_clean_up_after_a_failed_rename() {
        let dir = std::env::temp_dir().join(format!("prism-ini-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&dir).unwrap();
        let path = dir.join("instance.cfg");
        fs::write(&path, "name=old\n").unwrap();
        #[cfg(unix)]
        let original = fs::File::open(&path).unwrap();
        let ini = IniFile::parse("name=new\n");

        ini.save(&path).unwrap();
        assert_eq!(IniFile::load(&path).unwrap().get("name"), Some("new"));
        #[cfg(unix)]
        {
            let mut text = String::new();
            std::io::Read::read_to_string(&mut &original, &mut text).unwrap();
            assert_eq!(text, "name=old\n");
        }

        let blocked = dir.join("blocked.cfg");
        fs::create_dir(&blocked).unwrap();
        fs::write(blocked.join("keep"), "untouched").unwrap();
        assert!(ini.save(&blocked).is_err());
        assert_eq!(fs::read_to_string(blocked.join("keep")).unwrap(), "untouched");
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 2);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn windows_paths_roundtrip_without_decoding_literal_escapes() {
        let mut ini = IniFile::default();
        ini.set("HorizonLauncherPath", r"C:\tools\horizon-launcher.exe");
        ini.set("HorizonGameHome", r"C:\new\test");
        ini.set("notes", "new\nline\ttab");
        let parsed = IniFile::parse(&ini.serialize());
        assert_eq!(parsed, ini);
    }

    #[test]
    fn preserves_sections() {
        let text = "[General]\nname=26.3 Coobyk\nMaxMemAlloc=8192\n\n[UI]\nmods_Page\\Columns=\"AAAA\"\n";
        let mut ini = IniFile::parse(text);
        assert_eq!(ini.get("name"), Some("26.3 Coobyk"));
        assert_eq!(ini.get("mods_Page\\Columns"), Some("AAAA"));
        assert_eq!(ini.get("MaxMemAlloc"), Some("8192"));

        // Editing a [General] key must not flatten the file.
        ini.set("MaxMemAlloc", "16384");
        let out = ini.serialize();
        assert!(out.starts_with("[General]\n"), "sections preserved: {out}");
        // QSettings quoting: the base64 value needed quotes in the source
        // file but re-serializes bare since it needs no escaping now.
        assert!(out.contains("[UI]\nmods_Page\\Columns=AAAA\n"), "UI section kept: {out}");
        assert!(out.contains("name=26.3 Coobyk\n"), "unquoted value kept: {out}");
        assert!(out.contains("MaxMemAlloc=16384\n"));
    }

    #[test]
    fn new_keys_go_to_default_section() {
        let text = "[General]\nname=x\n";
        let mut ini = IniFile::parse(text).with_default_section("General");
        ini.set("notes", "hello");
        let out = ini.serialize();
        assert!(out.contains("[General]\nname=x\nnotes=hello\n"), "{out}");
    }

    #[test]
    fn parses_instance_cfg() {
        let text = "General/Version=1.0\nInstanceType=OneSix\niconKey=default\nname=Test\n";
        let ini = IniFile::parse(text);
        assert_eq!(ini.get("InstanceType"), Some("OneSix"));
        assert_eq!(ini.get("name"), Some("Test"));
    }
}
