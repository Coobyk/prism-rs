use crate::nbt::{self, Compound, Tag};
use chrono::{DateTime, Utc};
use flate2::{Compression, read::GzDecoder, write::GzEncoder};
use std::{
    fs, io,
    io::{Read, Write},
    path::{Path, PathBuf},
    time::SystemTime,
};

const MAX_LEVEL_NBT: u64 = 16 * 1024 * 1024;
const LEVEL_DAT: &str = "level.dat";

#[derive(Clone, Debug, PartialEq, Eq)]
struct FileIdentity {
    created: Option<SystemTime>,
    #[cfg(unix)]
    id: (u64, u64),
}

impl From<&fs::Metadata> for FileIdentity {
    fn from(meta: &fs::Metadata) -> Self {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        Self {
            created: meta.created().ok(),
            #[cfg(unix)]
            id: (meta.dev(), meta.ino()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FileStamp {
    identity: FileIdentity,
    modified: Option<SystemTime>,
    len: u64,
}

impl From<&fs::Metadata> for FileStamp {
    fn from(meta: &fs::Metadata) -> Self {
        Self {
            identity: meta.into(),
            modified: meta.modified().ok(),
            len: meta.len(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct World {
    game_root: PathBuf,
    name: String,
    folder_id: FileIdentity,
    level_stamp: FileStamp,
}

#[derive(Debug)]
pub struct WorldDetails {
    pub display_name: String,
    pub seed: Option<i64>,
    pub game_mode: Option<i32>,
    pub last_played: Option<DateTime<Utc>>,
}

#[derive(Debug)]
pub enum RenameOutcome {
    Renamed,
    NameOnly(io::Error),
}

pub(crate) fn plain_name(name: &str) -> bool {
    Path::new(name).file_name().and_then(|part| part.to_str()) == Some(name)
        && !name.contains(['/', '\\', ':'])
        && !name.chars().any(char::is_control)
}

pub(crate) fn require_directory(path: &Path) -> io::Result<()> {
    if fs::symlink_metadata(path)?.is_dir() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "Not a regular directory: {}",
            path.display()
        )))
    }
}

fn stamps(world: &Path) -> io::Result<(FileIdentity, FileStamp)> {
    let folder = fs::symlink_metadata(world)?;
    if !folder.is_dir() {
        return Err(io::Error::other("Not a regular world directory"));
    }
    let metadata = fs::symlink_metadata(world.join(LEVEL_DAT))?;
    if !metadata.is_file() {
        return Err(io::Error::other("Not a Java world (no regular level.dat)"));
    }
    Ok(((&folder).into(), (&metadata).into()))
}

pub(crate) fn nbt_child<'a>(root: &'a Tag, name: &str) -> Option<&'a Tag> {
    let Tag::Compound(entries) = root else {
        return None;
    };
    entries
        .iter()
        .find_map(|(key, value)| (key == name).then_some(value))
}

pub(crate) fn read_level(path: &Path) -> io::Result<(String, Tag)> {
    let mut bytes = Vec::new();
    GzDecoder::new(fs::File::open(path)?)
        .take(MAX_LEVEL_NBT + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_LEVEL_NBT {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "level.dat exceeds the NBT size limit",
        ));
    }
    nbt::read(&bytes)
}

impl World {
    pub fn open(game_root: &Path, name: &str) -> io::Result<Self> {
        if !plain_name(name) {
            return Err(io::Error::other("Invalid world name"));
        }
        let saves = game_root.join("saves");
        require_directory(&saves)?;
        let (folder_id, level_stamp) = stamps(&saves.join(name))?;
        Ok(Self {
            game_root: game_root.into(),
            name: name.into(),
            folder_id,
            level_stamp,
        })
    }

    pub fn list(game_root: &Path) -> io::Result<Vec<Self>> {
        let saves = game_root.join("saves");
        match require_directory(&saves) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            result => result?,
        }
        let mut worlds = Vec::new();
        for entry in fs::read_dir(saves)? {
            let entry = entry?;
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            if !plain_name(&name) {
                continue;
            }
            if let Ok((folder_id, level_stamp)) = stamps(&entry.path()) {
                worlds.push(Self {
                    game_root: game_root.into(),
                    name,
                    folder_id,
                    level_stamp,
                });
            }
        }
        worlds.sort_by_key(|world| (world.name.to_lowercase(), world.name.clone()));
        Ok(worlds)
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn game_root(&self) -> &Path {
        &self.game_root
    }

    pub fn world_path(&self) -> io::Result<PathBuf> {
        let saves = self.game_root.join("saves");
        require_directory(&saves)?;
        let world = saves.join(&self.name);
        let (folder_id, level_stamp) = stamps(&world)?;
        if folder_id != self.folder_id || level_stamp != self.level_stamp {
            return Err(io::Error::other(
                "The selected world changed. Refresh and try again.",
            ));
        }
        Ok(world)
    }

    pub fn details(&self) -> io::Result<WorldDetails> {
        let (_, root) = read_level(&self.world_path()?.join(LEVEL_DAT))?;
        let Some(data @ Tag::Compound(_)) = nbt_child(&root, "Data") else {
            return Err(invalid_data());
        };
        let long = |tag: Option<&Tag>| match tag {
            Some(Tag::Long(value)) => Some(*value),
            _ => None,
        };
        let seed = long(
            nbt_child(data, "WorldGenSettings").and_then(|settings| nbt_child(settings, "seed")),
        )
        .or_else(|| long(nbt_child(data, "RandomSeed")));
        let last_played = long(nbt_child(data, "LastPlayed"))
            .and_then(DateTime::from_timestamp_millis)
            .or_else(|| self.level_stamp.modified.map(DateTime::from));
        let game_mode = match nbt_child(data, "GameType") {
            Some(Tag::Int(mode)) => Some(*mode),
            _ => None,
        };
        let display_name = match nbt_child(data, "LevelName") {
            Some(Tag::String(name)) if !name.is_empty() => name.clone(),
            _ => self.name.clone(),
        };
        Ok(WorldDetails {
            display_name,
            seed,
            game_mode,
            last_played,
        })
    }

    pub fn rename(&self, name: &str) -> io::Result<RenameOutcome> {
        let path = self.world_path()?;
        let bytes = renamed_level(&path, name)?;
        let file = write_level(&path, &bytes)?;
        let level_stamp = FileStamp::from(&file.as_file().metadata()?);
        self.world_path()?;
        file.persist(path.join(LEVEL_DAT))
            .map_err(|error| error.error)?;
        let updated = Self {
            level_stamp,
            ..self.clone()
        };
        Ok(match updated.rename_folder(&path, name) {
            Ok(()) => RenameOutcome::Renamed,
            Err(error) => RenameOutcome::NameOnly(error),
        })
    }

    fn rename_folder(&self, path: &Path, name: &str) -> io::Result<()> {
        let saves = self.game_root.join("saves");
        let base = rename_folder_base(name);
        for index in 0..=9000 {
            let target = saves.join(if index == 0 {
                base.clone()
            } else {
                format!("{base}({index})")
            });
            if target == path {
                self.world_path()?;
                return Ok(());
            }
            match fs::symlink_metadata(&target) {
                Ok(_) => continue,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
            self.world_path()?;
            match fs::rename(path, &target) {
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                result => return result,
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "No unique save folder name available",
        ))
    }

    pub fn copy(&self, name: &str) -> io::Result<PathBuf> {
        let source = self.world_path()?;
        let bytes = renamed_level(&source, name)?;
        let saves = self.game_root.join("saves");
        let dest = reserve_copy_path(&saves, name)?;
        let result = (|| {
            copy_tree(&source, &dest, true)?;
            self.world_path()?;
            write_level(&dest, &bytes)?
                .persist_noclobber(dest.join(LEVEL_DAT))
                .map_err(|error| error.error)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_dir_all(&dest);
        }
        result.map(|()| dest)
    }

    pub fn delete(&self) -> io::Result<()> {
        fs::remove_dir_all(self.world_path()?)
    }
}

fn invalid_data() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "level.dat has no Data compound")
}

fn rename_folder_base(name: &str) -> String {
    let clean: String = name
        .chars()
        .filter(|ch| !ch.is_control() && !r#"<>:"/\|?*!"#.contains(*ch))
        .collect();
    let clean = clean
        .trim_start()
        .trim_end_matches(|ch: char| ch.is_whitespace() || ch == '.');
    if clean.is_empty() {
        return "World".into();
    }
    let device = clean
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end()
        .to_ascii_uppercase();
    if matches!(device.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ["COM", "LPT"].iter().any(|prefix| {
            device.strip_prefix(prefix).is_some_and(|suffix| {
                matches!(
                    suffix,
                    "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                )
            })
        })
    {
        return format!("_{clean}");
    }
    clean.into()
}

fn renamed_level(path: &Path, name: &str) -> io::Result<Vec<u8>> {
    let name = name.trim();
    if name.is_empty() || name.chars().any(char::is_control) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Enter a valid world name",
        ));
    }
    let (root_name, mut root) = read_level(&path.join(LEVEL_DAT))?;
    let Tag::Compound(root_tags) = &mut root else {
        return Err(invalid_data());
    };
    let Some((_, Tag::Compound(data))) = root_tags.iter_mut().find(|(key, _)| key == "Data") else {
        return Err(invalid_data());
    };
    set_level_name(data, name);
    let bytes = nbt::write(&root_name, &root)?;
    if bytes.len() as u64 > MAX_LEVEL_NBT {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "level.dat exceeds the NBT size limit",
        ));
    }
    Ok(bytes)
}

fn set_level_name(data: &mut Compound, name: &str) {
    let name = Tag::String(name.into());
    if let Some((_, old)) = data.iter_mut().find(|(key, _)| key == "LevelName") {
        *old = name;
    } else {
        data.push(("LevelName".into(), name));
    }
}

fn write_level(path: &Path, bytes: &[u8]) -> io::Result<tempfile::NamedTempFile> {
    let mut temp = tempfile::NamedTempFile::new_in(path)?;
    let mut encoder = GzEncoder::new(temp.as_file_mut(), Compression::default());
    encoder.write_all(bytes)?;
    encoder.finish()?.sync_all()?;
    Ok(temp)
}

fn reserve_copy_path(saves: &Path, name: &str) -> io::Result<PathBuf> {
    let base: String = name
        .trim()
        .chars()
        .take(45)
        .map(|ch| {
            if ch.is_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect();
    // A prefix also keeps paths portable for Windows device names such as CON.
    let base = format!("World-{base}");
    for index in 1.. {
        let path = saves.join(if index == 1 {
            base.clone()
        } else {
            format!("{base}-{index}")
        });
        match fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    unreachable!()
}

fn copy_tree(source: &Path, dest: &Path, root: bool) -> io::Result<()> {
    require_directory(source)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let name = entry.file_name();
        if root && name == LEVEL_DAT {
            continue;
        }
        let path = dest.join(&name);
        let kind = entry.file_type()?;
        if kind.is_dir() {
            fs::create_dir(&path)?;
            copy_tree(&entry.path(), &path, false)?;
        } else if kind.is_file() {
            fs::copy(entry.path(), path)?;
        } else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "World contains a symbolic link or special file",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(root: &Path, folder: &str, data: Compound) -> PathBuf {
        let path = root.join("saves").join(folder);
        fs::create_dir_all(&path).unwrap();
        let tags = Tag::Compound(vec![
            ("Data".into(), Tag::Compound(data)),
            ("Unknown".into(), Tag::IntArray(vec![42])),
        ]);
        write_level(&path, &nbt::write("Root", &tags).unwrap())
            .unwrap()
            .persist(path.join(LEVEL_DAT))
            .unwrap();
        path
    }

    #[test]
    fn reads_modern_and_legacy_world_details() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fixture(
            root,
            "modern",
            vec![
                ("LevelName".into(), Tag::String("雪 🌱".into())),
                ("LastPlayed".into(), Tag::Long(1_700_000_000_000)),
                ("GameType".into(), Tag::Int(1)),
                ("RandomSeed".into(), Tag::Long(-73)),
                (
                    "WorldGenSettings".into(),
                    Tag::Compound(vec![("seed".into(), Tag::Long(0))]),
                ),
            ],
        );
        fixture(root, "legacy", vec![("RandomSeed".into(), Tag::Long(-73))]);
        let worlds = World::list(root).unwrap();
        assert_eq!(
            worlds.iter().map(World::name).collect::<Vec<_>>(),
            ["legacy", "modern"]
        );
        let legacy = worlds[0].details().unwrap();
        assert_eq!(legacy.display_name, "legacy");
        assert_eq!(legacy.seed, Some(-73));
        let modern = worlds[1].details().unwrap();
        assert_eq!(modern.display_name, "雪 🌱");
        assert_eq!(modern.seed, Some(0));
        assert_eq!(modern.game_mode, Some(1));
        assert_eq!(
            modern.last_played.unwrap().timestamp_millis(),
            1_700_000_000_000
        );
    }

    #[test]
    fn rename_copy_and_delete_preserve_world_data_and_reject_stale_targets() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let source = fixture(
            root,
            "saved-folder",
            vec![
                ("LevelName".into(), Tag::String("Original".into())),
                ("GameType".into(), Tag::Int(0)),
            ],
        );
        let level = source.join(LEVEL_DAT);
        let (expected_name, expected_tags) = read_level(&level).unwrap();
        fs::create_dir(source.join("region")).unwrap();
        fs::write(source.join("region/r.0.0.mca"), b"region contents").unwrap();
        let original = World::open(root, "saved-folder").unwrap();
        assert!(matches!(
            original.rename("New 🌍 name").unwrap(),
            RenameOutcome::Renamed
        ));
        assert!(!source.exists());
        let source = root.join("saves/New 🌍 name");
        let level = source.join(LEVEL_DAT);
        let updated = World::open(root, "New 🌍 name").unwrap();
        assert_eq!(updated.details().unwrap().display_name, "New 🌍 name");
        assert!(original.rename("stale").is_err());
        assert!(original.copy("stale").is_err());
        assert!(original.delete().is_err());
        assert!(updated.rename(" ").is_err());
        let before_copy = fs::read(&level).unwrap();
        let dest = updated.copy("../Copy 🌍").unwrap();
        let another = updated.copy("../Copy 🌍").unwrap();
        assert_ne!(dest, another);
        assert_eq!(dest.parent(), source.parent());
        assert_eq!(
            fs::read(dest.join("region/r.0.0.mca")).unwrap(),
            b"region contents"
        );
        assert_eq!(fs::read(&level).unwrap(), before_copy);
        let (copy_name, mut copy_tags) = read_level(&dest.join(LEVEL_DAT)).unwrap();
        assert_eq!(copy_name, expected_name);
        let Tag::Compound(entries) = &mut copy_tags else {
            panic!("expected a compound")
        };
        let (_, Tag::Compound(data)) = &mut entries[0] else {
            panic!("expected Data")
        };
        assert_eq!(data[0].1, Tag::String("../Copy 🌍".into()));
        set_level_name(data, "Original");
        assert_eq!(copy_tags, expected_tags);
        let copied_world = World::open(root, dest.file_name().unwrap().to_str().unwrap()).unwrap();
        copied_world.delete().unwrap();
        assert!(!dest.exists());
        assert!(another.is_dir() && source.is_dir());
    }

    #[test]
    fn rename_strips_invalid_folder_characters_and_does_not_clobber_existing_saves() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let source = fixture(root, "original", vec![]);
        let collision = fixture(root, "Oak Ridge", vec![]);
        let before = fs::read(collision.join(LEVEL_DAT)).unwrap();
        fs::create_dir(root.join("saves/Oak Ridge(1)")).unwrap();
        let original = World::open(root, "original").unwrap();
        assert!(matches!(
            original.rename("Oak:/ Ridge?*.").unwrap(),
            RenameOutcome::Renamed
        ));
        let updated = World::open(root, "Oak Ridge(2)").unwrap();
        assert_eq!(updated.details().unwrap().display_name, "Oak:/ Ridge?*.");
        assert!(!source.exists());
        assert!(original.delete().is_err());
        assert_eq!(fs::read(collision.join(LEVEL_DAT)).unwrap(), before);
        assert!(root.join("saves/Oak Ridge(1)").is_dir());
        assert_eq!(rename_folder_base("../.."), "World");
        assert_eq!(rename_folder_base("CON.txt"), "_CON.txt");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn folder_rename_failure_keeps_new_level_name_and_invalidates_old_handle() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fixture(root, "original", vec![]);
        let original = World::open(root, "original").unwrap();
        let long_name = "x".repeat(512);
        assert!(matches!(
            original.rename(&long_name).unwrap(),
            RenameOutcome::NameOnly(_)
        ));
        assert!(original.delete().is_err());
        let updated = World::open(root, "original").unwrap();
        assert_eq!(updated.details().unwrap().display_name, long_name);
    }

    #[test]
    fn damaged_level_is_not_replaced_or_copied() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let path = fixture(root, "bad", vec![]);
        fs::write(path.join(LEVEL_DAT), b"not NBT").unwrap();
        let world = World::open(root, "bad").unwrap();
        assert!(world.details().is_err());
        assert!(world.rename("replacement").is_err());
        assert!(world.copy("replacement").is_err());
        assert_eq!(fs::read(path.join(LEVEL_DAT)).unwrap(), b"not NBT");
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
    }

    #[test]
    fn replacing_the_world_folder_invalidates_a_target_even_with_the_same_level() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let path = fixture(root, "world", vec![]);
        let snapshot = World::open(root, "world").unwrap();
        let old = path.with_extension("old");
        fs::rename(&path, &old).unwrap();
        fs::create_dir(&path).unwrap();
        fs::rename(old.join(LEVEL_DAT), path.join(LEVEL_DAT)).unwrap();
        assert!(snapshot.delete().is_err());
        assert!(path.join(LEVEL_DAT).is_file());
    }

    #[cfg(unix)]
    #[test]
    fn copy_rejects_symlinks_and_cleans_up_incomplete_destination() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let path = fixture(root, "linked", vec![]);
        std::os::unix::fs::symlink(root, path.join("outside")).unwrap();
        let world = World::open(root, "linked").unwrap();
        assert!(world.copy("copy").is_err());
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
    }
}
