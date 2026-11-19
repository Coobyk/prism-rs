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
    saves: Directory,
    target: WorldTarget,
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

#[derive(Clone, Debug, PartialEq, Eq)]
struct Directory {
    path: PathBuf,
    id: FileIdentity,
}

impl Directory {
    fn resolve(path: &Path) -> io::Result<Self> {
        let path = fs::canonicalize(path)?;
        let meta = fs::symlink_metadata(&path)?;
        if !meta.is_dir() {
            return Err(io::Error::other("Not a directory"));
        }
        Ok(Self {
            path,
            id: (&meta).into(),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct WorldTarget {
    folder: Directory,
    link: Option<FileStamp>,
}

impl WorldTarget {
    fn read(entry: PathBuf) -> io::Result<(Self, FileStamp)> {
        let meta = fs::symlink_metadata(&entry)?;
        let (folder, link) = if meta.file_type().is_symlink() {
            (Directory::resolve(&entry)?, Some((&meta).into()))
        } else if meta.is_dir() {
            (
                Directory {
                    path: entry,
                    id: (&meta).into(),
                },
                None,
            )
        } else {
            return Err(io::Error::other("Not a world directory"));
        };
        let level = fs::symlink_metadata(folder.path.join(LEVEL_DAT))?;
        if !level.is_file() {
            return Err(io::Error::other("Not a Java world (no regular level.dat)"));
        }
        Ok((Self { folder, link }, (&level).into()))
    }
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
        Self::open_in(
            game_root,
            name,
            &Directory::resolve(&game_root.join("saves"))?,
        )
    }

    pub fn open_regular(game_root: &Path, name: &str) -> io::Result<Self> {
        let world = Self::open(game_root, name)?;
        world.regular_world_path()?;
        Ok(world)
    }

    fn open_in(game_root: &Path, name: &str, saves: &Directory) -> io::Result<Self> {
        if !plain_name(name) {
            return Err(io::Error::other("Invalid world name"));
        }
        let (target, level_stamp) = WorldTarget::read(saves.path.join(name))?;
        Ok(Self {
            game_root: game_root.into(),
            name: name.into(),
            saves: saves.clone(),
            target,
            level_stamp,
        })
    }

    pub fn list(game_root: &Path) -> io::Result<Vec<Self>> {
        Self::list_with_links(game_root, true)
    }

    pub fn list_regular(game_root: &Path) -> io::Result<Vec<Self>> {
        Self::list_with_links(game_root, false)
    }

    fn list_with_links(game_root: &Path, follow_links: bool) -> io::Result<Vec<Self>> {
        let saves = game_root.join("saves");
        let directory = (|| {
            if !follow_links {
                require_directory(&saves)?;
            }
            Directory::resolve(&saves)
        })();
        let saves = match directory {
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            result => result?,
        };
        let mut worlds = Vec::new();
        for entry in fs::read_dir(&saves.path)? {
            let entry = entry?;
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            if let Ok(world) = Self::open_in(game_root, &name, &saves) {
                if follow_links || world.target.link.is_none() {
                    worlds.push(world);
                }
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
        self.check()?;
        Ok(self.game_root.join("saves").join(&self.name))
    }

    pub(crate) fn regular_world_path(&self) -> io::Result<PathBuf> {
        let world = self.world_path()?;
        require_directory(&self.game_root.join("saves"))?;
        require_directory(&world)?;
        Ok(world)
    }

    fn entry_path(&self) -> PathBuf {
        self.saves.path.join(&self.name)
    }

    fn check(&self) -> io::Result<()> {
        let saves = self.game_root.join("saves");
        if Directory::resolve(&saves)? == self.saves {
            let (target, level_stamp) = WorldTarget::read(self.entry_path())?;
            if target == self.target && level_stamp == self.level_stamp {
                return Ok(());
            }
        }
        Err(io::Error::other(
            "The selected world changed. Refresh and try again.",
        ))
    }

    pub fn details(&self) -> io::Result<WorldDetails> {
        self.check()?;
        let (_, root) = read_level(&self.target.folder.path.join(LEVEL_DAT))?;
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
        self.check()?;
        let path = &self.target.folder.path;
        let bytes = renamed_level(path, name)?;
        let file = write_level(path, &bytes)?;
        let level_stamp = FileStamp::from(&file.as_file().metadata()?);
        self.check()?;
        file.persist(path.join(LEVEL_DAT))
            .map_err(|error| error.error)?;
        let updated = Self {
            level_stamp,
            ..self.clone()
        };
        Ok(match updated.rename_folder(name) {
            Ok(()) => RenameOutcome::Renamed,
            Err(error) => RenameOutcome::NameOnly(error),
        })
    }

    fn rename_folder(&self, name: &str) -> io::Result<()> {
        let path = self.entry_path();
        let base = rename_folder_base(name);
        for index in 0..=9000 {
            let target = self.saves.path.join(if index == 0 {
                base.clone()
            } else {
                format!("{base}({index})")
            });
            if target == path {
                return self.check();
            }
            match fs::symlink_metadata(&target) {
                Ok(_) => continue,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
            self.check()?;
            match fs::rename(&path, &target) {
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
        self.check()?;
        let source = &self.target.folder.path;
        if self.saves.path.starts_with(source) {
            return Err(io::Error::other("Cannot copy a world into itself"));
        }
        let bytes = renamed_level(source, name)?;
        let dest = reserve_copy_path(&self.saves.path, name)?;
        let result = (|| {
            copy_tree(source, &dest, true)?;
            self.check()?;
            write_level(&dest, &bytes)?
                .persist_noclobber(dest.join(LEVEL_DAT))
                .map_err(|error| error.error)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_dir_all(&dest);
        }
        result.map(|()| self.game_root.join("saves").join(dest.file_name().unwrap()))
    }

    pub fn delete(&self) -> io::Result<()> {
        self.check()?;
        let entry = self.entry_path();
        if self.target.link.is_some() {
            #[cfg(windows)]
            return fs::remove_dir(entry);
            #[cfg(not(windows))]
            return fs::remove_file(entry);
        }
        fs::remove_dir_all(entry)
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
    fn lists_worlds_through_linked_saves_and_world_folders() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let world = fixture(root, "World", vec![]);
        symlink("World", world.with_file_name("Alias")).unwrap();
        for linked_saves in [false, true] {
            if linked_saves {
                fs::rename(root.join("saves"), root.join("moved-saves")).unwrap();
                symlink("moved-saves", root.join("saves")).unwrap();
            }
            let worlds = World::list(root).unwrap();
            assert_eq!(
                worlds.iter().map(World::name).collect::<Vec<_>>(),
                ["Alias", "World"]
            );
            assert_eq!(worlds[0], World::open(root, "Alias").unwrap());
            assert_eq!(worlds[0].world_path().unwrap(), root.join("saves/Alias"));
            assert_eq!(worlds[0].details().unwrap().display_name, "Alias");
        }
    }

    #[cfg(unix)]
    #[test]
    fn linked_world_operations_leave_the_real_folder_in_place() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let world = fixture(root, "Original", vec![]);
        fs::create_dir(world.join("region")).unwrap();
        fs::write(world.join("region/r.0.0.mca"), "chunks").unwrap();
        fs::rename(root.join("saves"), root.join("stored")).unwrap();
        let world = root.join("stored/Original");
        fs::create_dir(root.join("linked-saves")).unwrap();
        symlink("linked-saves", root.join("saves")).unwrap();
        symlink("../stored/Original", root.join("linked-saves/Alias")).unwrap();

        let alias = World::open(root, "Alias").unwrap();
        assert!(matches!(
            alias.rename("New World").unwrap(),
            RenameOutcome::Renamed
        ));
        assert!(!root.join("linked-saves/Alias").exists());
        assert_eq!(
            fs::read_link(root.join("linked-saves/New World")).unwrap(),
            Path::new("../stored/Original")
        );
        let (_, tag) = read_level(&world.join(LEVEL_DAT)).unwrap();
        assert_eq!(
            nbt_child(nbt_child(&tag, "Data").unwrap(), "LevelName"),
            Some(&Tag::String("New World".into()))
        );

        let renamed = World::open(root, "New World").unwrap();
        let copy = renamed.copy("Independent").unwrap();
        assert!(fs::symlink_metadata(&copy).unwrap().is_dir());
        assert_eq!(fs::read(copy.join("region/r.0.0.mca")).unwrap(), b"chunks");
        assert_eq!(
            World::open(root, "World-Independent")
                .unwrap()
                .details()
                .unwrap()
                .display_name,
            "Independent"
        );
        assert_eq!(renamed.details().unwrap().display_name, "New World");

        renamed.delete().unwrap();
        assert!(fs::symlink_metadata(root.join("linked-saves/New World")).is_err());
        assert!(world.join(LEVEL_DAT).is_file());
        assert!(world.join("region/r.0.0.mca").is_file());
    }

    #[cfg(unix)]
    #[test]
    fn retargeting_a_world_link_chain_or_saves_link_invalidates_the_handle() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let alpha = fixture(root, "Alpha", vec![]);
        let beta = fixture(root, "Beta", vec![]);
        let level = fs::read(beta.join(LEVEL_DAT)).unwrap();
        let bridge = root.join("bridge");
        let alias = root.join("saves/Alias");
        symlink(&alpha, &bridge).unwrap();
        symlink(&bridge, &alias).unwrap();
        let stale = World::open(root, "Alias").unwrap();
        fs::remove_file(&bridge).unwrap();
        symlink(&beta, &bridge).unwrap();
        assert!(stale.rename("Stale").is_err());
        assert!(stale.copy("Stale").is_err());
        assert!(stale.delete().is_err());
        assert_eq!(fs::read(beta.join(LEVEL_DAT)).unwrap(), level);
        assert!(alias.is_symlink());

        fs::rename(root.join("saves"), root.join("real-saves")).unwrap();
        symlink("real-saves", root.join("saves")).unwrap();
        let original = root.join("real-saves/Beta");
        let stale = World::open(root, "Beta").unwrap();
        fs::create_dir(root.join("replacement-saves")).unwrap();
        let moved = root.join("replacement-saves/Beta");
        fs::rename(original, &moved).unwrap();
        fs::remove_file(root.join("saves")).unwrap();
        symlink("replacement-saves", root.join("saves")).unwrap();
        assert!(stale.delete().is_err());
        assert_eq!(fs::read(moved.join(LEVEL_DAT)).unwrap(), level);
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
