//! Minecraft's per-instance servers.dat list.
use crate::nbt::{self, Compound, Tag};
use std::{fs, io, path::Path};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum ResourcePackPolicy {
    #[default]
    Prompt,
    Enabled,
    Disabled,
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct ServerEntry {
    pub name: String,
    pub address: String,
    pub icon: Option<String>,
    pub accept_textures: ResourcePackPolicy,
    pub extra: Compound,
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct ServerList {
    pub entries: Vec<ServerEntry>,
    pub extra: Compound,
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[derive(Debug)]
pub struct ServerListSnapshot {
    contents: Option<Vec<u8>>,
}

impl ServerListSnapshot {
    fn read(game_root: &Path) -> io::Result<Option<Vec<u8>>> {
        match fs::read(game_root.join("servers.dat")) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub fn load(game_root: &Path) -> io::Result<(ServerList, Self)> {
        let contents = Self::read(game_root)?;
        let list = Self::parse(&contents)?;
        Ok((list, Self { contents }))
    }

    fn parse(contents: &Option<Vec<u8>>) -> io::Result<ServerList> {
        contents
            .as_deref()
            .map(ServerList::from_bytes)
            .transpose()
            .map(Option::unwrap_or_default)
    }

    /// Returns true when an external change was reloaded instead of saving.
    pub fn save_or_reload(&mut self, game_root: &Path, list: &mut ServerList) -> io::Result<bool> {
        let contents = Self::read(game_root)?;
        if contents != self.contents {
            *list = Self::parse(&contents)?;
            self.contents = contents;
            return Ok(true);
        }
        let contents = list.to_bytes()?;
        list.save(game_root)?;
        self.contents = Some(contents);
        Ok(false)
    }
}

impl ServerList {
    pub fn load(game_root: &Path) -> io::Result<Self> {
        match fs::read(game_root.join("servers.dat")) {
            Ok(bytes) => Self::from_bytes(&bytes),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e),
        }
    }

    pub fn from_bytes(bytes: &[u8]) -> io::Result<Self> {
        let (_, root) = nbt::read(bytes)?;
        let Tag::Compound(root) = root else {
            return Err(invalid("servers.dat root is not a compound"));
        };
        let mut result = Self::default();
        for (key, value) in root {
            if key != "servers" {
                result.extra.push((key, value));
                continue;
            }
            let Tag::List(id, entries) = value else {
                return Err(invalid("servers is not a list"));
            };
            if id != 10 && !(id == 0 && entries.is_empty()) {
                return Err(invalid("servers must contain compounds"));
            }
            for entry in entries {
                let Tag::Compound(tags) = entry else {
                    return Err(invalid("invalid server entry"));
                };
                let mut server = ServerEntry::default();
                for (key, value) in tags {
                    match (key.as_str(), value) {
                        ("name", Tag::String(v)) => server.name = v,
                        ("ip", Tag::String(v)) => server.address = v,
                        ("icon", Tag::String(v)) => server.icon = Some(v),
                        ("acceptTextures", Tag::Byte(v @ (0 | 1))) => {
                            server.accept_textures = if v == 1 {
                                ResourcePackPolicy::Enabled
                            } else {
                                ResourcePackPolicy::Disabled
                            };
                        }
                        (_, v) => server.extra.push((key, v)),
                    }
                }
                result.entries.push(server);
            }
        }
        Ok(result)
    }

    pub fn to_bytes(&self) -> io::Result<Vec<u8>> {
        let entries = self
            .entries
            .iter()
            .map(|entry| {
                let mut tags = Vec::new();
                tags.push(("name".into(), Tag::String(entry.name.clone())));
                tags.push(("ip".into(), Tag::String(entry.address.clone())));
                if let Some(icon) = &entry.icon {
                    tags.push(("icon".into(), Tag::String(icon.clone())));
                }
                match entry.accept_textures {
                    ResourcePackPolicy::Prompt => (),
                    ResourcePackPolicy::Enabled => {
                        tags.push(("acceptTextures".into(), Tag::Byte(1)))
                    }
                    ResourcePackPolicy::Disabled => {
                        tags.push(("acceptTextures".into(), Tag::Byte(0)))
                    }
                }
                tags.extend(
                    entry
                        .extra
                        .iter()
                        .filter(|(key, _)| {
                            !matches!(key.as_str(), "name" | "ip")
                                && (key != "icon" || entry.icon.is_none())
                                && (key != "acceptTextures"
                                    || entry.accept_textures == ResourcePackPolicy::Prompt)
                        })
                        .cloned(),
                );
                Tag::Compound(tags)
            })
            .collect();
        let mut root = self.extra.clone();
        root.push(("servers".into(), Tag::List(10, entries)));
        nbt::write("", &Tag::Compound(root))
    }

    pub fn save(&self, game_root: &Path) -> io::Result<()> {
        use std::io::Write;
        let bytes = self.to_bytes()?;
        fs::create_dir_all(game_root)?;
        let temp = game_root.join(format!(".servers-{}.tmp", uuid::Uuid::new_v4()));
        let result = (|| {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temp, game_root.join("servers.dat"))
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temp);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn snapshot_saves_and_reloads_external_changes() {
        let root =
            std::env::temp_dir().join(format!("prism-servers-snapshot-{}", uuid::Uuid::new_v4()));
        let (mut list, mut snapshot) = ServerListSnapshot::load(&root).unwrap();
        assert_eq!(list, ServerList::default());
        list.entries.push(ServerEntry {
            name: "First".into(),
            ..Default::default()
        });
        assert!(!snapshot.save_or_reload(&root, &mut list).unwrap());
        assert_eq!(ServerList::load(&root).unwrap(), list);
        let (mut other, mut other_snapshot) = ServerListSnapshot::load(&root).unwrap();
        list.entries[0].address = "first.example.org".into();
        assert!(!snapshot.save_or_reload(&root, &mut list).unwrap());
        other.entries[0].name = "Stale edit".into();
        assert!(other_snapshot.save_or_reload(&root, &mut other).unwrap());
        assert_eq!(other, list);
        assert_eq!(ServerList::load(&root).unwrap(), list);
        other.entries[0].name = "Fresh edit".into();
        assert!(!other_snapshot.save_or_reload(&root, &mut other).unwrap());
        fs::remove_file(root.join("servers.dat")).unwrap();
        assert!(other_snapshot.save_or_reload(&root, &mut other).unwrap());
        assert_eq!(other, ServerList::default());
        assert!(!root.join("servers.dat").exists());
        assert!(!other_snapshot.save_or_reload(&root, &mut other).unwrap());
        fs::write(root.join("servers.dat"), b"invalid").unwrap();
        assert!(other_snapshot.save_or_reload(&root, &mut other).is_err());
        assert_eq!(fs::read(root.join("servers.dat")).unwrap(), b"invalid");
        assert_eq!(other, ServerList::default());
        assert!(ServerListSnapshot::load(&root).is_err());
        fs::remove_file(root.join("servers.dat")).unwrap();
        fs::create_dir(root.join("servers.dat")).unwrap();
        assert!(other_snapshot.save_or_reload(&root, &mut other).is_err());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn unexpected_fields_preserved_after_known_fields() {
        let extra = vec![
            ("name".into(), Tag::Int(42)),
            ("acceptTextures".into(), Tag::Byte(2)),
            ("ip".into(), Tag::Byte(1)),
            ("icon".into(), Tag::Int(3)),
        ];
        let bytes = nbt::write(
            "",
            &Tag::Compound(vec![(
                "servers".into(),
                Tag::List(10, vec![Tag::Compound(extra.clone())]),
            )]),
        )
        .unwrap();
        let mut list = ServerList::from_bytes(&bytes).unwrap();
        assert_eq!(list.entries[0].extra, extra);
        let unedited = ServerList::from_bytes(&list.to_bytes().unwrap()).unwrap();
        assert_eq!(
            unedited.entries[0].extra,
            vec![extra[1].clone(), extra[3].clone()]
        );
        list.entries[0].name = "Edited name".into();
        list.entries[0].address = "edited.example.org".into();
        list.entries[0].icon = Some("icon".into());
        list.entries[0].accept_textures = ResourcePackPolicy::Enabled;
        let encoded = list.to_bytes().unwrap();
        let (_, Tag::Compound(root)) = nbt::read(&encoded).unwrap() else {
            panic!()
        };
        let Tag::List(_, entries) = &root[0].1 else {
            panic!()
        };
        let Tag::Compound(tags) = &entries[0] else {
            panic!()
        };
        assert_eq!(
            tags[..4]
                .iter()
                .map(|(key, _)| key.as_str())
                .collect::<Vec<_>>(),
            ["name", "ip", "icon", "acceptTextures"]
        );
        assert_eq!(tags.len(), 4);
        assert_eq!(
            tags.iter()
                .map(|(key, _)| key)
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            tags.len()
        );
        list.entries[0].extra.clear();
        assert_eq!(ServerList::from_bytes(&encoded).unwrap(), list);
    }
    #[test]
    fn vanilla_fixture() {
        let bytes = b"\x0a\x00\x00\x09\x00\x07servers\x0a\x00\x00\x00\x01\x08\x00\x04name\x00\x04Test\x08\x00\x02ip\x00\x09localhost\x01\x00\x06hidden\x01\x00\x00";
        let list = ServerList::from_bytes(bytes).unwrap();
        assert_eq!(list.entries[0].name, "Test");
        assert_eq!(list.entries[0].address, "localhost");
        assert_eq!(list.entries[0].extra, vec![("hidden".into(), Tag::Byte(1))]);
        assert_eq!(
            ServerList::from_bytes(&list.to_bytes().unwrap()).unwrap(),
            list
        );
    }
    #[test]
    fn policies_and_unknown_tags() {
        for (policy, expected) in [
            (ResourcePackPolicy::Prompt, None),
            (ResourcePackPolicy::Enabled, Some(Tag::Byte(1))),
            (ResourcePackPolicy::Disabled, Some(Tag::Byte(0))),
        ] {
            let list = ServerList {
                entries: vec![ServerEntry {
                    name: "Test".into(),
                    address: "example.org:25565".into(),
                    icon: Some("base64".into()),
                    accept_textures: policy,
                    extra: vec![
                        ("hidden".into(), Tag::Byte(0)),
                        ("future".into(), Tag::LongArray(vec![42])),
                    ],
                }],
                extra: vec![(
                    "futureRoot".into(),
                    Tag::Compound(vec![("x".into(), Tag::Int(1))]),
                )],
            };
            let bytes = list.to_bytes().unwrap();
            assert_eq!(ServerList::from_bytes(&bytes).unwrap(), list);
            let (_, Tag::Compound(root)) = nbt::read(&bytes).unwrap() else {
                panic!()
            };
            let Tag::List(_, entries) = &root.last().unwrap().1 else {
                panic!()
            };
            let Tag::Compound(tags) = &entries[0] else {
                panic!()
            };
            assert_eq!(
                tags.iter()
                    .find(|(k, _)| k == "acceptTextures")
                    .map(|(_, v)| v.clone()),
                expected
            );
        }
    }
    #[test]
    fn missing_save_and_errors() {
        let root =
            std::env::temp_dir().join(format!("prism-servers-test-{}", uuid::Uuid::new_v4()));
        assert_eq!(ServerList::load(&root).unwrap(), ServerList::default());
        let list = ServerList::default();
        list.save(&root).unwrap();
        list.save(&root).unwrap();
        assert_eq!(ServerList::load(&root).unwrap(), list);
        fs::write(root.join("servers.dat"), b"bad").unwrap();
        assert!(ServerList::load(&root).is_err());
        assert!(list.save(&root.join("servers.dat")).is_err());
        fs::remove_dir_all(root).unwrap();
        assert!(ServerList::from_bytes(&nbt::write("", &Tag::Int(1)).unwrap()).is_err());
    }
}
