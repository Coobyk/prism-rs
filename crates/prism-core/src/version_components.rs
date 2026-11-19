use crate::{
    loaders::Loader,
    worlds::{plain_name, FileStamp},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

pub(crate) fn patched_client(root: &Path, client: &Path, mods: &[PathBuf]) -> io::Result<PathBuf> {
    let dir = archive_dir(root)?;
    let file = tempfile::NamedTempFile::new_in(&dir)?;
    let mut output = zip::ZipWriter::new(file);
    let mut written = HashSet::new();
    for path in mods
        .iter()
        .rev()
        .map(PathBuf::as_path)
        .chain(std::iter::once(client))
    {
        let mut archive = zip::ZipArchive::new(fs::File::open(path)?)?;
        for index in 0..archive.len() {
            let entry = archive.by_index_raw(index)?;
            let name = entry.name();
            let upper = name.to_ascii_uppercase();
            let signature = upper.strip_prefix("META-INF/").is_some_and(|name| {
                !name.contains('/')
                    && (name.starts_with("SIG-")
                        || [".SF", ".RSA", ".DSA", ".EC"]
                            .iter()
                            .any(|ext| name.ends_with(ext)))
            });
            if entry.is_dir()
                || entry.enclosed_name().is_none()
                || name.contains('\\')
                || signature
                || (path != client && upper == "META-INF/MANIFEST.MF")
                || !written.insert(name.to_owned())
            {
                continue;
            }
            output.raw_copy_file(entry)?;
        }
    }
    let destination = dir.join("minecraft-patched.jar");
    output.finish()?.persist(&destination)?;
    Ok(destination)
}

const PACK: &str = "mmc-pack.json";
const ARCHIVES: &str = "version-files";
const EXTRA: &str = "prismRsVersionFiles";
pub const MINECRAFT: &str = "net.minecraft";
pub const LWJGL: &str = "org.lwjgl";
pub const LWJGL3: &str = "org.lwjgl3";
const LOADER_UIDS: &[&str] = &[
    "net.fabricmc.fabric-loader",
    "org.quiltmc.quilt-loader",
    "net.minecraftforge",
    "net.neoforged",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ArchiveKind {
    JarMod,
    Classpath,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Archive {
    pub name: String,
    pub kind: ArchiveKind,
    id: uuid::Uuid,
}

impl Archive {
    fn path(&self, dir: &Path) -> PathBuf {
        dir.join(format!("{}.jar", self.id))
    }
}

#[derive(Clone, Debug)]
pub struct Pack {
    root: PathBuf,
    config: FileStamp,
    stamp: FileStamp,
    data: Value,
    archives: Vec<Archive>,
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn stamp(path: &Path) -> io::Result<FileStamp> {
    let meta = fs::symlink_metadata(path)?;
    if !meta.is_file() {
        return Err(invalid("Expected a regular file, not a symbolic link."));
    }
    Ok((&meta).into())
}

pub(crate) fn archive_dir(root: &Path) -> io::Result<PathBuf> {
    let dir = root.join(ARCHIVES);
    if !fs::symlink_metadata(&dir)?.is_dir() {
        return Err(invalid(
            "Version files must be stored in a local directory.",
        ));
    }
    Ok(dir)
}

pub fn version_id(value: &str) -> io::Result<&str> {
    let value = value.trim();
    if !plain_name(value)
        || !value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'+' | b' ')
        })
    {
        return Err(invalid("Enter a valid version ID."));
    }
    Ok(value)
}

impl Pack {
    pub fn load(root: &Path) -> io::Result<Self> {
        let config = stamp(&root.join("instance.cfg"))?;
        let path = root.join(PACK);
        let stamp = stamp(&path)?;
        let data: Value = serde_json::from_reader(fs::File::open(path)?)?;
        if !data["components"].is_array() {
            return Err(invalid("Invalid component list."));
        }
        let archives = match data.get(EXTRA) {
            Some(value) => serde_json::from_value(value.clone())?,
            None => Vec::new(),
        };
        Ok(Self {
            root: root.into(),
            config,
            stamp,
            data,
            archives,
        })
    }

    pub fn components(&self) -> impl Iterator<Item = (&str, &str)> {
        self.data["components"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|row| {
                Some((
                    row["uid"].as_str()?,
                    row["version"]
                        .as_str()
                        .or_else(|| row["cachedVersion"].as_str())
                        .unwrap_or(""),
                ))
            })
    }

    pub fn version(&self, uid: &str) -> Option<&str> {
        self.components()
            .find_map(|(key, version)| (key == uid && !version.is_empty()).then_some(version))
    }

    pub fn lwjgl(&self) -> Option<(&str, &str)> {
        self.components()
            .find(|(uid, version)| matches!(*uid, LWJGL | LWJGL3) && !version.is_empty())
    }

    pub fn archives(&self) -> &[Archive] {
        &self.archives
    }

    pub fn set_minecraft(mut self, version: &str) -> io::Result<Self> {
        let version = version_id(version)?;
        self.upsert(MINECRAFT, version);
        self.save()
    }

    pub fn set_lwjgl(mut self, uid: &str, version: Option<&str>) -> io::Result<Self> {
        if !matches!(uid, LWJGL | LWJGL3) {
            return Err(invalid("Unknown LWJGL component."));
        }
        let version = version.map(version_id).transpose()?;
        if let Some(version) = version {
            let other = if uid == LWJGL { LWJGL3 } else { LWJGL };
            self.rows().retain(|row| row["uid"] != other);
            self.upsert(uid, version);
        } else {
            self.rows()
                .retain(|row| !matches!(row["uid"].as_str(), Some(LWJGL | LWJGL3)));
        }
        self.save()
    }

    pub fn set_loader(mut self, loader: &Loader) -> io::Result<Self> {
        self.rows().retain(|row| {
            let uid = row["uid"].as_str().unwrap_or_default();
            uid == loader.uid() || !LOADER_UIDS.contains(&uid)
        });
        if let Some(version) = loader.version() {
            self.upsert(loader.uid(), version_id(version)?);
        }
        self.save()
    }

    fn rows(&mut self) -> &mut Vec<Value> {
        self.data["components"].as_array_mut().unwrap()
    }

    fn upsert(&mut self, uid: &str, version: &str) {
        if let Some(row) = self.rows().iter_mut().find(|row| row["uid"] == uid) {
            row["version"] = json!(version);
            row["cachedVersion"] = json!(version);
        } else {
            self.rows().push(json!({"uid": uid, "version": version, "important": uid == MINECRAFT || LOADER_UIDS.contains(&uid)}));
        }
    }

    pub fn add_archive(mut self, source: &Path, kind: ArchiveKind) -> io::Result<Self> {
        let name = source
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| invalid("Select a named .jar or .zip file."))?;
        let ext = source
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("");
        if !ext.eq_ignore_ascii_case("jar")
            && !(kind == ArchiveKind::JarMod && ext.eq_ignore_ascii_case("zip"))
        {
            return Err(invalid("Select a JAR, or a ZIP for a jar mod."));
        }
        self.check()?;
        match fs::create_dir(self.root.join(ARCHIVES)) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
        let dir = archive_dir(&self.root)?;
        let entry = Archive {
            kind,
            name: name.into(),
            id: uuid::Uuid::new_v4(),
        };
        let mut file = tempfile::NamedTempFile::new_in(&dir)?;
        io::copy(&mut fs::File::open(source)?, file.as_file_mut())?;
        zip::ZipArchive::new(file.reopen()?)
            .map_err(|_| invalid("The selected file is not a valid archive."))?;
        let path = entry.path(&dir);
        file.persist_noclobber(&path)?;
        self.archives.push(entry);
        match self.save() {
            Ok(pack) => Ok(pack),
            Err(error) => {
                let _ = fs::remove_file(path);
                Err(error)
            }
        }
    }

    pub fn move_archive(mut self, index: usize, offset: isize) -> io::Result<Self> {
        let kind = self
            .archives
            .get(index)
            .map(|entry| entry.kind)
            .ok_or_else(|| invalid("This entry cannot move in that direction."))?;
        let adjacent = match offset {
            -1 => (0..index)
                .rev()
                .find(|&other| self.archives[other].kind == kind),
            1 => (index + 1..self.archives.len()).find(|&other| self.archives[other].kind == kind),
            _ => None,
        }
        .ok_or_else(|| invalid("This entry cannot move in that direction."))?;
        self.archives.swap(index, adjacent);
        self.save()
    }

    pub fn remove_archive(mut self, index: usize) -> io::Result<Self> {
        if index >= self.archives.len() {
            return Err(invalid("The selected entry no longer exists."));
        }
        let entry = self.archives.remove(index);
        let path = archive_dir(&self.root).ok().map(|dir| entry.path(&dir));
        let saved = self.save()?;
        if let Some(path) = path {
            if let Err(error) = fs::remove_file(path) {
                if error.kind() != io::ErrorKind::NotFound {
                    tracing::warn!(
                        "Version entry removed, but its archive could not be deleted: {error}"
                    );
                }
            }
        }
        Ok(saved)
    }

    pub(crate) fn archive_paths(&self, kind: ArchiveKind) -> io::Result<Vec<PathBuf>> {
        if !self.archives.iter().any(|entry| entry.kind == kind) {
            return Ok(Vec::new());
        }
        let dir = archive_dir(&self.root)?;
        self.archives
            .iter()
            .filter(|entry| entry.kind == kind)
            .map(|entry| {
                let path = entry.path(&dir);
                stamp(&path).map_err(|error| {
                    io::Error::new(error.kind(), format!("{}: {error}", entry.name))
                })?;
                Ok(path)
            })
            .collect()
    }

    fn check(&self) -> io::Result<()> {
        if stamp(&self.root.join(PACK))? != self.stamp
            || stamp(&self.root.join("instance.cfg"))? != self.config
        {
            return Err(io::Error::other(
                "The instance changed. Reload Version and try again.",
            ));
        }
        Ok(())
    }

    fn save(mut self) -> io::Result<Self> {
        self.check()?;
        self.data[EXTRA] = serde_json::to_value(&self.archives)?;
        let mut file = tempfile::NamedTempFile::new_in(&self.root)?;
        serde_json::to_writer_pretty(&mut file, &self.data)?;
        writeln!(file)?;
        self.check()?;
        file.persist(self.root.join(PACK))?;
        Self::load(&self.root)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn instance(root: &Path) {
        fs::write(root.join("instance.cfg"), "").unwrap();
        fs::write(
            root.join(PACK),
            json!({"components": [
            {"uid": MINECRAFT, "version": "1.20.4", "important": true},
            {"uid": "custom.unknown", "version": "1", "opaque": true}
        ], "formatVersion": 1})
            .to_string(),
        )
        .unwrap();
    }

    #[test]
    fn component_edits_preserve_unrelated_values_and_reject_stale_saves() {
        let root = tempfile::tempdir().unwrap();
        instance(root.path());
        let initial = Pack::load(root.path()).unwrap();
        let saved = initial
            .clone()
            .set_minecraft("1.20.3")
            .unwrap()
            .set_loader(&Loader::Quilt {
                version: "0.26.4".into(),
            })
            .unwrap()
            .set_lwjgl(LWJGL3, Some("3.3.3"))
            .unwrap();
        assert!(initial.set_minecraft("1.19").is_err());
        assert_eq!(saved.version(MINECRAFT), Some("1.20.3"));
        assert_eq!(saved.version("org.quiltmc.quilt-loader"), Some("0.26.4"));
        assert_eq!(saved.lwjgl(), Some((LWJGL3, "3.3.3")));
        assert_eq!(saved.data["components"][1]["opaque"], true);
    }

    fn write_jar(path: &Path, entries: &[(&str, &str)]) {
        let mut jar = zip::ZipWriter::new(fs::File::create(path).unwrap());
        for (name, content) in entries {
            jar.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            jar.write_all(content.as_bytes()).unwrap();
        }
        jar.finish().unwrap();
    }

    #[test]
    fn archive_order_controls_the_client_overlay_without_changing_the_base() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        instance(root);
        let client = root.join("client.jar");
        let first = root.join("first.zip");
        let second = root.join("second.jar");
        write_jar(
            &client,
            &[
                ("Main.class", "original"),
                ("keep.txt", "keep"),
                ("META-INF/CLIENT.SF", "signature"),
            ],
        );
        write_jar(
            &first,
            &[
                ("Main.class", "first"),
                ("only-first", "yes"),
                ("../outside", "skip"),
            ],
        );
        write_jar(&second, &[("Main.class", "second")]);
        let original = fs::read(&client).unwrap();
        let pack = Pack::load(root)
            .unwrap()
            .add_archive(&first, ArchiveKind::JarMod)
            .unwrap()
            .add_archive(&client, ArchiveKind::Classpath)
            .unwrap()
            .add_archive(&second, ArchiveKind::JarMod)
            .unwrap()
            .move_archive(0, 1)
            .unwrap();
        assert_eq!(pack.archives()[2].name, "first.zip");
        assert_eq!(pack.archive_paths(ArchiveKind::Classpath).unwrap().len(), 1);
        let output = patched_client(
            root,
            &client,
            &pack.archive_paths(ArchiveKind::JarMod).unwrap(),
        )
        .unwrap();
        let mut zip = zip::ZipArchive::new(fs::File::open(output).unwrap()).unwrap();
        for (name, value) in [
            ("Main.class", "first"),
            ("keep.txt", "keep"),
            ("only-first", "yes"),
        ] {
            let mut buffer = String::new();
            zip.by_name(name)
                .unwrap()
                .read_to_string(&mut buffer)
                .unwrap();
            assert_eq!(buffer, value);
        }
        assert!(zip.by_name("META-INF/CLIENT.SF").is_err());
        assert!(zip.by_name("../outside").is_err());
        assert_eq!(fs::read(&client).unwrap(), original);
        let removed = pack
            .archive_paths(ArchiveKind::JarMod)
            .unwrap()
            .pop()
            .unwrap();
        let pack = pack.remove_archive(2).unwrap();
        assert!(!removed.exists());
        assert_eq!(pack.archive_paths(ArchiveKind::JarMod).unwrap().len(), 1);
    }
}
