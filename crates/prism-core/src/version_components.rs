use crate::{
    component::Component,
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
    patches: std::collections::BTreeMap<String, Option<FileStamp>>,
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
        let mut patches = std::collections::BTreeMap::new();
        for row in data["components"].as_array().unwrap() {
            let uid = crate::component::uid(
                row["uid"]
                    .as_str()
                    .ok_or_else(|| invalid("Missing component UID"))?,
            )?;
            if patches
                .insert(
                    uid.to_owned(),
                    optional_stamp(&root.join("patches").join(format!("{uid}.json")))?,
                )
                .is_some()
            {
                return Err(invalid("Duplicate component UID"));
            }
        }
        Ok(Self {
            root: root.into(),
            config,
            stamp,
            data,
            archives,
            patches,
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

    pub fn enabled_components(&self) -> impl Iterator<Item = (&str, &str)> {
        self.components().filter(|(uid, _)| {
            !self.data["components"]
                .as_array()
                .unwrap()
                .iter()
                .any(|row| {
                    row["uid"] == *uid && row["disabled"] == true && row["important"] != true
                })
        })
    }

    pub fn is_custom(&self, uid: &str) -> bool {
        self.patches.get(uid).is_some_and(Option::is_some)
    }

    pub fn dependency_only(&self, uid: &str) -> bool {
        self.data["components"].as_array().unwrap().iter()
            .any(|row| row["uid"] == uid && row["dependencyOnly"] == true)
    }

    pub fn has_meta(&self, uid: &str) -> bool {
        matches!(uid, MINECRAFT | LWJGL | LWJGL3) || LOADER_UIDS.contains(&uid)
    }

    pub fn patch(&self, uid: &str) -> io::Result<Option<Component>> {
        crate::component::uid(uid)?;
        self.check()?;
        if !self.is_custom(uid) {
            return Ok(None);
        }
        let text = fs::read_to_string(self.root.join("patches").join(format!("{uid}.json")))?;
        Component::parse(&text, Some(uid)).map(Some)
    }

    pub fn patch_text(&self, uid: &str) -> io::Result<String> {
        crate::component::uid(uid)?;
        self.check()?;
        if !self.is_custom(uid) {
            return Err(invalid("The component is not customized."));
        }
        fs::read_to_string(self.root.join("patches").join(format!("{uid}.json")))
    }

    pub fn save_patch(mut self, uid: &str, text: &str) -> io::Result<Self> {
        let component = Component::parse(text, Some(uid))?;
        component.validate_launch()?;
        if !self.patches.contains_key(uid) {
            return Err(invalid("Component no longer exists"));
        }
        self.write_component(uid, text.as_bytes(), Vec::new())
    }

    pub fn customize(mut self, uid: &str, document: &Value) -> io::Result<Self> {
        if !self.has_meta(uid) || self.is_custom(uid) || !self.patches.contains_key(uid) {
            return Err(invalid("This component cannot be customized."));
        }
        let text = serde_json::to_vec_pretty(document)?;
        Component::parse(std::str::from_utf8(&text).unwrap(), Some(uid))?.validate_launch()?;
        self.write_component(uid, &text, Vec::new())
    }

    pub fn add_empty(self, uid: &str, name: &str) -> io::Result<Self> {
        let document = json!({"formatVersion":1,"uid":uid,"name":name,"version":"1"});
        self.import_document(&serde_json::to_string_pretty(&document)?)
    }

    pub fn import_component(self, source: &Path) -> io::Result<Self> {
        self.import_document(&fs::read_to_string(source)?)
    }

    fn import_document(mut self, text: &str) -> io::Result<Self> {
        let component = Component::parse(text, None)?;
        component.validate_launch()?;
        let uid = component.id();
        if self.patches.contains_key(uid) {
            return Err(invalid("A component with this UID already exists."));
        }
        if optional_stamp(&self.root.join("patches").join(format!("{uid}.json")))?.is_some() {
            return Err(invalid("A patch with this UID already exists."));
        }
        self.patches.insert(uid.to_owned(), None);
        self.rows().push(json!({"uid":uid,"cachedName":component.0["name"],"cachedVersion":component.0["version"]}));
        self.write_component(uid, text.as_bytes(), Vec::new())
    }

    pub fn revert(self, uid: &str) -> io::Result<Self> {
        if !self.has_meta(uid) || !self.is_custom(uid) {
            return Err(invalid("This component cannot be reverted."));
        }
        let path = self.root.join("patches").join(format!("{uid}.json"));
        self.commit_files(vec![(path, None)])
    }

    pub fn remove_component(mut self, uid: &str) -> io::Result<Self> {
        if self.has_meta(uid) {
            return Err(invalid("Meta components must be reverted, not removed."));
        }
        let component = self
            .patch(uid)?
            .ok_or_else(|| invalid("Local component not found"))?;
        let mut artifacts = component.local_artifacts(&self.root)?;
        for (other, _) in self.components().filter(|(other, _)| *other != uid) {
            if let Some(other) = self.patch(other)? {
                let shared = other.local_artifacts(&self.root)?;
                artifacts.retain(|path| !shared.contains(path));
            }
        }
        let mut files: Vec<_> = artifacts.into_iter().map(|path| (path, None)).collect();
        files.push((self.root.join("patches").join(format!("{uid}.json")), None));
        self.rows().retain(|row| row["uid"] != uid);
        self.commit_files(files)
    }

    pub fn install_custom_jar(mut self, source: &Path) -> io::Result<Self> {
        let (stem, bytes) = jar_source(source)?;
        let uid = "customjar";
        let document = json!({"formatVersion":1,"uid":uid,"name":format!("{stem} (custom jar)"),
            "mainJar":{"name":"custom:customjar:1","MMC-displayname":stem,"MMC-hint":"local"}});
        if !self.patches.contains_key(uid) {
            if optional_stamp(&self.root.join("patches/customjar.json"))?.is_some() {
                return Err(invalid("An unregistered customjar patch already exists."));
            }
            self.patches.insert(uid.into(), None);
            self.rows()
                .push(json!({"uid":uid,"cachedName":document["name"]}));
        }
        let artifact = self.root.join("libraries/customjar-1.jar");
        self.write_component(
            uid,
            &serde_json::to_vec_pretty(&document)?,
            vec![(artifact, Some(bytes))],
        )
    }

    pub fn install_agent(mut self, source: &Path) -> io::Result<Self> {
        let (stem, bytes) = jar_source(source)?;
        let id = uuid::Uuid::new_v4();
        let uid = format!("custom.agent.{id}");
        let filename = format!("{id}.jar");
        let document = json!({"formatVersion":1,"uid":uid,"name":format!("{stem} (agent)"),
            "+agents":[{"name":format!("custom.agents:{id}:1"),"MMC-filename":filename,"MMC-displayname":stem,"MMC-hint":"local"}]});
        self.patches.insert(uid.clone(), None);
        self.rows()
            .push(json!({"uid":uid,"cachedName":document["name"]}));
        let artifact = self.root.join("libraries").join(filename);
        self.write_component(
            &uid,
            &serde_json::to_vec_pretty(&document)?,
            vec![(artifact, Some(bytes))],
        )
    }

    fn write_component(
        &mut self,
        uid: &str,
        text: &[u8],
        mut files: Vec<(PathBuf, Option<Vec<u8>>)>,
    ) -> io::Result<Self> {
        files.push((
            self.root.join("patches").join(format!("{uid}.json")),
            Some(text.to_vec()),
        ));
        self.clone().commit_files(files)
    }

    fn commit_files(mut self, mut files: Vec<(PathBuf, Option<Vec<u8>>)>) -> io::Result<Self> {
        self.check()?;
        self.data[EXTRA] = serde_json::to_value(&self.archives)?;
        files.push((
            self.root.join(PACK),
            Some(serde_json::to_vec_pretty(&self.data)?),
        ));
        let mut staged = Vec::new();
        for (path, contents) in &files {
            let parent = path.parent().unwrap();
            match fs::create_dir(parent) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e),
            }
            if !fs::symlink_metadata(parent)?.is_dir() {
                return Err(invalid("Component storage must be a local directory."));
            }
            let previous = match optional_stamp(path)? {
                Some(_) => Some(fs::read(path)?),
                None => None,
            };
            let temp = contents
                .as_ref()
                .map(|bytes| -> io::Result<_> {
                    let mut file = tempfile::NamedTempFile::new_in(parent)?;
                    file.write_all(bytes)?;
                    Ok(file)
                })
                .transpose()?;
            staged.push(StagedFile { path: path.clone(), file: temp, previous });
        }
        self.check()?;
        publish_files(staged)?;
        Self::load(&self.root)
    }

    pub fn set_minecraft(mut self, version: &str) -> io::Result<Self> {
        let version = version_id(version)?;
        self.upsert(MINECRAFT, version);
        if self.is_custom(MINECRAFT) {
            let patch = self.root.join("patches").join(format!("{MINECRAFT}.json"));
            return self.commit_files(vec![(patch, None)]);
        }
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
        for (uid, expected) in &self.patches {
            let dir = self.root.join("patches");
            if let Ok(meta) = fs::symlink_metadata(&dir) {
                if !meta.is_dir() {
                    return Err(invalid("Patches must be a local directory."));
                }
            }
            if optional_stamp(&dir.join(format!("{uid}.json")))? != *expected {
                return Err(io::Error::other(
                    "The component changed. Reload Version and try again.",
                ));
            }
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

struct StagedFile {
    path: PathBuf,
    file: Option<tempfile::NamedTempFile>,
    previous: Option<Vec<u8>>,
}

fn publish_files(mut staged: Vec<StagedFile>) -> io::Result<()> {
    for index in 0..staged.len() {
        let entry = &mut staged[index];
        let result = match entry.file.take() {
            Some(file) => file.persist(&entry.path).map(|_| ()).map_err(|e| e.error),
            None => match fs::remove_file(&entry.path) {
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
                result => result,
            },
        };
        if let Err(error) = result {
            for entry in staged[..index].iter().rev() {
                let rollback = match &entry.previous {
                    Some(bytes) => (|| -> io::Result<()> {
                        let mut file = tempfile::NamedTempFile::new_in(entry.path.parent().unwrap())?;
                        file.write_all(bytes)?;
                        file.persist(&entry.path)?;
                        Ok(())
                    })(),
                    None => fs::remove_file(&entry.path),
                };
                if let Err(rollback) = rollback {
                    return Err(io::Error::other(format!("{error}; rollback failed: {rollback}")));
                }
            }
            return Err(error);
        }
    }
    Ok(())
}

fn optional_stamp(path: &Path) -> io::Result<Option<FileStamp>> {
    match stamp(path) {
        Ok(stamp) => Ok(Some(stamp)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

fn jar_source(source: &Path) -> io::Result<(String, Vec<u8>)> {
    if !source
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("jar"))
    {
        return Err(invalid("Select a .jar file."));
    }
    let name = source
        .file_stem()
        .and_then(|name| name.to_str())
        .ok_or_else(|| invalid("Select a named JAR."))?
        .to_owned();
    let bytes = fs::read(source)?;
    zip::ZipArchive::new(std::io::Cursor::new(&bytes))
        .map_err(|_| invalid("The selected file is not a valid JAR archive."))?;
    Ok((name, bytes))
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
    fn local_components_round_trip_reject_duplicates_and_stale_edits() {
        let root = tempfile::tempdir().unwrap();
        instance(root.path());
        let pack = Pack::load(root.path())
            .unwrap()
            .add_empty("local.test", "Test")
            .unwrap();
        let text = pack.patch_text("local.test").unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&text).unwrap(),
            json!({"formatVersion":1,"uid":"local.test","name":"Test","version":"1"})
        );
        assert!(pack.clone().add_empty("local.test", "Duplicate").is_err());
        assert!(pack.clone().add_empty("../bad", "Bad").is_err());
        assert!(pack.clone().save_patch("local.test", "").is_err());
        assert!(pack
            .clone()
            .save_patch("local.test", &text.replace("local.test", "other"))
            .is_err());
        let saved = pack
            .clone()
            .save_patch("local.test", &text.replace("Test", "Changed"))
            .unwrap();
        assert!(pack.save_patch("local.test", &text).is_err());
        let saved = saved.remove_component("local.test").unwrap();
        assert!(!saved.is_custom("local.test"));
        assert!(!root.path().join("patches/local.test.json").exists());
    }

    #[test]
    fn customize_revert_and_deleted_instance_guard() {
        let root = tempfile::tempdir().unwrap();
        instance(root.path());
        let doc =
            json!({"uid":MINECRAFT,"version":"1.20.4","formatVersion":1,"mainClass":"custom.Main"});
        let pack = Pack::load(root.path())
            .unwrap()
            .customize(MINECRAFT, &doc)
            .unwrap();
        assert!(pack.is_custom(MINECRAFT));
        assert_eq!(pack.patch(MINECRAFT).unwrap().unwrap().0, doc);
        let pack = pack.revert(MINECRAFT).unwrap();
        assert_eq!(pack.version(MINECRAFT), Some("1.20.4"));
        assert!(!pack.is_custom(MINECRAFT));
        fs::remove_file(root.path().join("instance.cfg")).unwrap();
        assert!(pack.add_empty("test", "Deleted").is_err());
    }

    #[test]
    fn changing_minecraft_reverts_the_customized_document() {
        let root = tempfile::tempdir().unwrap();
        instance(root.path());
        let document = json!({"uid":MINECRAFT,"version":"1.20.4","formatVersion":1,"mainClass":"custom.Main"});
        let pack = Pack::load(root.path()).unwrap().customize(MINECRAFT, &document).unwrap();
        assert!(pack.clone().set_minecraft("../bad").is_err());
        assert!(pack.is_custom(MINECRAFT));
        let pack = pack.set_minecraft("1.20.1").unwrap();
        assert_eq!(pack.version(MINECRAFT), Some("1.20.1"));
        assert!(!pack.is_custom(MINECRAFT));
        assert!(!root.path().join("patches/net.minecraft.json").exists());
        assert!(pack.components().any(|(uid, _)| uid == "custom.unknown"));
    }

    #[test]
    fn installers_match_upstream_and_remove_unshared_artifacts() {
        let root = tempfile::tempdir().unwrap();
        instance(root.path());
        let source = root.path().join("sample.jar");
        write_jar(&source, &[("test", "data")]);
        let bytes = fs::read(&source).unwrap();
        let pack = Pack::load(root.path())
            .unwrap()
            .install_custom_jar(&source)
            .unwrap()
            .install_agent(&source)
            .unwrap();
        assert_eq!(
            fs::read(root.path().join("libraries/customjar-1.jar")).unwrap(),
            bytes
        );
        let main = pack.patch("customjar").unwrap().unwrap();
        assert_eq!(
            main.0,
            json!({"formatVersion":1,"uid":"customjar","name":"sample (custom jar)","mainJar":{"name":"custom:customjar:1","MMC-displayname":"sample","MMC-hint":"local"}})
        );
        let agent_uid = pack
            .components()
            .find(|(uid, _)| uid.starts_with("custom.agent."))
            .unwrap()
            .0
            .to_owned();
        let agent = pack.patch(&agent_uid).unwrap().unwrap();
        let id = agent_uid.strip_prefix("custom.agent.").unwrap();
        assert_eq!(
            agent.0,
            json!({"formatVersion":1,"uid":agent_uid,"name":"sample (agent)","+agents":[{"name":format!("custom.agents:{id}:1"),"MMC-filename":format!("{id}.jar"),"MMC-displayname":"sample","MMC-hint":"local"}]})
        );
        assert_eq!(
            fs::read(root.path().join(format!("libraries/{id}.jar"))).unwrap(),
            bytes
        );
        let pack = pack.install_custom_jar(&source).unwrap();
        assert_eq!(
            pack.components()
                .filter(|(uid, _)| *uid == "customjar")
                .count(),
            1
        );
        let pack = pack
            .remove_component(&agent_uid)
            .unwrap()
            .remove_component("customjar")
            .unwrap();
        assert!(!root.path().join(format!("libraries/{id}.jar")).exists());
        assert!(!root.path().join("libraries/customjar-1.jar").exists());
        assert!(!pack.is_custom("customjar"));
    }

    #[test]
    fn failed_install_does_not_publish_a_partial_component() {
        let root = tempfile::tempdir().unwrap();
        instance(root.path());
        let source = root.path().join("source.jar");
        write_jar(&source, &[("test", "data")]);
        let pack = Pack::load(root.path()).unwrap();
        fs::write(root.path().join("patches"), "not a directory").unwrap();
        assert!(pack.install_custom_jar(&source).is_err());
        assert!(!root.path().join("libraries/customjar-1.jar").exists());
        fs::remove_file(root.path().join("patches")).unwrap();
        fs::write(&source, "not a jar").unwrap();
        assert!(Pack::load(root.path())
            .unwrap()
            .install_agent(&source)
            .is_err());
    }

    #[test]
    fn publication_failure_rolls_back_replacements_and_new_artifacts() {
        let root = tempfile::tempdir().unwrap();
        let existing = root.path().join("existing.json");
        let added = root.path().join("added.jar");
        let blocked = root.path().join("blocked.json");
        fs::write(&existing, "original").unwrap();
        fs::create_dir(&blocked).unwrap();
        let staged = [(&existing, Some(b"original".to_vec())), (&added, None), (&blocked, None)]
            .into_iter().map(|(path, previous)| {
                let mut file = tempfile::NamedTempFile::new_in(root.path()).unwrap();
                file.write_all(b"replacement").unwrap();
                StagedFile { path: path.clone(), file: Some(file), previous }
            }).collect();
        assert!(publish_files(staged).is_err());
        assert_eq!(fs::read_to_string(existing).unwrap(), "original");
        assert!(!added.exists());
        assert!(blocked.is_dir());
    }

    #[test]
    fn malformed_patch_can_be_opened_and_repaired() {
        let root = tempfile::tempdir().unwrap();
        instance(root.path());
        let pack = Pack::load(root.path()).unwrap().add_empty("local.broken", "Broken").unwrap();
        let text = pack.patch_text("local.broken").unwrap();
        fs::write(root.path().join("patches/local.broken.json"), "{broken").unwrap();
        let pack = Pack::load(root.path()).unwrap();
        assert_eq!(pack.patch_text("local.broken").unwrap(), "{broken");
        assert!(pack.patch("local.broken").is_err());
        let pack = pack.save_patch("local.broken", &text).unwrap();
        assert_eq!(pack.patch("local.broken").unwrap().unwrap().id(), "local.broken");
    }

    #[test]
    fn disabled_components_are_not_resolved_and_import_keeps_source_bytes() {
        let root = tempfile::tempdir().unwrap();
        instance(root.path());
        let source = root.path().join("source.json");
        let text = "{\"formatVersion\":1,\"uid\":\"local.import\",\"name\":\"Imported\",\"version\":\"1\"}\n";
        fs::write(&source, text).unwrap();
        let mut pack = Pack::load(root.path())
            .unwrap()
            .import_component(&source)
            .unwrap();
        assert_eq!(pack.patch_text("local.import").unwrap(), text);
        for row in pack.rows() {
            if row["uid"] == "local.import" {
                row["disabled"] = json!(true);
            }
        }
        let pack = pack.save().unwrap();
        assert!(!pack
            .enabled_components()
            .any(|(uid, _)| uid == "local.import"));
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
