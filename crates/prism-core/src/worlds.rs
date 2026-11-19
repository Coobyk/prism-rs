//! Java world management for the instance Worlds page (upstream `World` /
//! `WorldList`). Operations go through [`World`] so a target that another
//! window or task changed since it was listed is rejected.

use crate::datapacks::{ensure_absent, nbt_child, read_gzip_nbt, World};
use crate::nbt::{self, Tag};
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GameMode {
    Survival,
    Creative,
    Adventure,
    Spectator,
}

impl GameMode {
    fn from_id(id: i32) -> Option<Self> {
        Some(match id {
            0 => Self::Survival,
            1 => Self::Creative,
            2 => Self::Adventure,
            3 => Self::Spectator,
            _ => return None,
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Survival => "Survival",
            Self::Creative => "Creative",
            Self::Adventure => "Adventure",
            Self::Spectator => "Spectator",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorldDetails {
    /// `Data.LevelName`, or the folder name when the world has none.
    pub display_name: String,
    pub game_mode: Option<GameMode>,
    pub hardcore: bool,
    pub seed: Option<i64>,
    /// Milliseconds since the Unix epoch.
    pub last_played: Option<i64>,
}

impl World {
    pub fn path(&self) -> io::Result<PathBuf> {
        self.world_path()
    }

    pub fn details(&self) -> io::Result<WorldDetails> {
        let (_, root) = read_gzip_nbt(&self.world_path()?.join("level.dat"))?;
        let data = nbt_child(&root, "Data")
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "level.dat has no Data"))?;
        let long = |tag: Option<&Tag>| match tag {
            Some(Tag::Long(value)) => Some(*value),
            _ => None,
        };
        let display_name = match nbt_child(data, "LevelName") {
            Some(Tag::String(name)) if !name.trim().is_empty() => name.clone(),
            _ => self.name.clone(),
        };
        let seed = nbt_child(data, "WorldGenSettings")
            .and_then(|settings| long(nbt_child(settings, "seed")))
            .or_else(|| long(nbt_child(data, "RandomSeed")));
        Ok(WorldDetails {
            display_name,
            game_mode: match nbt_child(data, "GameType") {
                Some(Tag::Int(id)) => GameMode::from_id(*id),
                _ => None,
            },
            hardcore: matches!(nbt_child(data, "hardcore"), Some(Tag::Byte(flag)) if *flag != 0),
            seed,
            last_played: long(nbt_child(data, "LastPlayed")),
        })
    }

    /// Sets the display name and moves the world to a matching free folder.
    pub fn rename(&self, display_name: &str) -> io::Result<World> {
        let display_name = checked_display_name(display_name)?;
        let world = self.world_path()?;
        let saves = self.game_root.join("saves");
        let level = world.join("level.dat");
        let original = fs::read(&level)?;
        set_level_name(&world, display_name)?;
        let mut folders = folder_candidates(display_name);
        let folder = if folders.clone().any(|folder| folder == self.name) {
            self.name.clone()
        } else {
            match folders.find(|folder| ensure_absent(&saves.join(folder)).is_ok()) {
                Some(folder) => folder,
                None => {
                    let _ = replace_file(&world, &level, &original);
                    return Err(io::Error::other("No free folder name for this world"));
                }
            }
        };
        if folder != self.name {
            if let Err(error) = fs::rename(&world, saves.join(&folder)) {
                let _ = replace_file(&world, &level, &original);
                return Err(error);
            }
        }
        World::open(&self.game_root, &folder)
    }

    /// Copies the world into a new folder without replacing anything or
    /// following symbolic links, then gives the copy `display_name`.
    pub fn copy(&self, display_name: &str) -> io::Result<World> {
        let display_name = checked_display_name(display_name)?;
        let world = self.world_path()?;
        let saves = self.game_root.join("saves");
        let mut reserved = None;
        for folder in folder_candidates(display_name) {
            match fs::create_dir(saves.join(&folder)) {
                Ok(()) => {
                    reserved = Some(folder);
                    break;
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        let folder =
            reserved.ok_or_else(|| io::Error::other("No free folder name for the copy"))?;
        let target = saves.join(&folder);
        let result = copy_tree(&world, &target)
            .and_then(|()| set_level_name(&target, display_name))
            .and_then(|()| World::open(&self.game_root, &folder));
        if result.is_err() {
            let _ = fs::remove_dir_all(&target);
        }
        result
    }

    /// Removes the world folder. It is first moved aside so a failed removal
    /// never leaves a half-deleted world in the list.
    pub fn delete(&self) -> io::Result<()> {
        let world = self.world_path()?;
        let doomed = tempfile::Builder::new()
            .prefix(".prism-rs-deleting-")
            .tempdir_in(self.game_root.join("saves"))?;
        fs::rename(&world, doomed.path().join(&self.name))?;
        doomed.close()
    }
}

fn checked_display_name(name: &str) -> io::Result<&str> {
    let name = name.trim();
    if name.is_empty() {
        return Err(io::Error::other("The world name cannot be empty"));
    }
    if name.chars().any(char::is_control) {
        return Err(io::Error::other(
            "The world name cannot contain control characters",
        ));
    }
    Ok(name)
}

/// Folder names for a display name, like upstream `DirNameFromString`: unsafe
/// characters are replaced and a numeric suffix avoids collisions.
fn folder_candidates(display_name: &str) -> impl Iterator<Item = String> + Clone {
    const RESERVED: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    let cleaned: String = display_name
        .chars()
        .map(|c| {
            if "<>:\"/\\|?*".contains(c) || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    let mut base = cleaned.trim().trim_end_matches(['.', ' ']).to_owned();
    if base.trim_start_matches('.').is_empty() {
        base = "World".into();
    } else if RESERVED.contains(&base.split('.').next().unwrap_or("").to_uppercase().as_str()) {
        base.insert(0, '_');
    }
    std::iter::once(base.clone()).chain((2..1000).map(move |n| format!("{base} ({n})")))
}

fn set_level_name(world: &Path, display_name: &str) -> io::Result<()> {
    let level = world.join("level.dat");
    let (root_name, mut root) = read_gzip_nbt(&level)?;
    let Tag::Compound(entries) = &mut root else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "level.dat is not a compound",
        ));
    };
    let Some(Tag::Compound(data)) = entries
        .iter_mut()
        .find_map(|(key, value)| (key == "Data").then_some(value))
    else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "level.dat has no Data",
        ));
    };
    let name = Tag::String(display_name.into());
    match data.iter_mut().find(|(key, _)| key == "LevelName") {
        Some((_, value)) => *value = name,
        None => data.push(("LevelName".into(), name)),
    }
    let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gzip.write_all(&nbt::write(&root_name, &root)?)?;
    replace_file(world, &level, &gzip.finish()?)
}

fn replace_file(dir: &Path, path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut temp = tempfile::NamedTempFile::new_in(dir)?;
    temp.write_all(bytes)?;
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|error| error.error)?;
    Ok(())
}

fn copy_tree(from: &Path, to: &Path) -> io::Result<()> {
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let target = to.join(entry.file_name());
        if kind.is_dir() {
            fs::create_dir(&target)?;
            copy_tree(&entry.path(), &target)?;
        } else if kind.is_file() {
            fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::read::GzDecoder;
    use std::io::Read;

    fn level_dat(data: Vec<(&str, Tag)>) -> Vec<u8> {
        let data = data
            .into_iter()
            .map(|(key, tag)| (key.into(), tag))
            .collect();
        let root = Tag::Compound(vec![("Data".into(), Tag::Compound(data))]);
        let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gzip.write_all(&nbt::write("", &root).unwrap()).unwrap();
        gzip.finish().unwrap()
    }

    fn save(root: &Path, folder: &str, data: Vec<(&str, Tag)>) -> World {
        let world = root.join("saves").join(folder);
        fs::create_dir_all(world.join("region")).unwrap();
        fs::write(world.join("level.dat"), level_dat(data)).unwrap();
        fs::write(world.join("region/r.0.0.mca"), folder.as_bytes()).unwrap();
        World::open(root, folder).unwrap()
    }

    fn named(name: &str) -> Vec<(&'static str, Tag)> {
        vec![
            ("LevelName", Tag::String(name.into())),
            ("GameType", Tag::Int(0)),
        ]
    }

    fn snapshot(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
        let mut files = Vec::new();
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                files.extend(snapshot(&path));
            } else {
                files.push((path.clone(), fs::read(&path).unwrap()));
            }
        }
        files.sort();
        files
    }

    fn raw_level_name(world: &World) -> String {
        let mut bytes = Vec::new();
        GzDecoder::new(fs::File::open(world.path().unwrap().join("level.dat")).unwrap())
            .read_to_end(&mut bytes)
            .unwrap();
        let (_, root) = nbt::read(&bytes).unwrap();
        match nbt_child(nbt_child(&root, "Data").unwrap(), "LevelName") {
            Some(Tag::String(name)) => name.clone(),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn details_read_modern_and_legacy_level_dat() {
        let root = tempfile::tempdir().unwrap();
        let modern = save(
            root.path(),
            "New World",
            vec![
                ("LevelName", Tag::String("Hardcore Island".into())),
                ("GameType", Tag::Int(0)),
                ("hardcore", Tag::Byte(1)),
                ("LastPlayed", Tag::Long(1_700_000_000_000)),
                (
                    "WorldGenSettings",
                    Tag::Compound(vec![("seed".into(), Tag::Long(-4_172_144_997_902_289_642))]),
                ),
            ],
        );
        assert_eq!(
            modern.details().unwrap(),
            WorldDetails {
                display_name: "Hardcore Island".into(),
                game_mode: Some(GameMode::Survival),
                hardcore: true,
                seed: Some(-4_172_144_997_902_289_642),
                last_played: Some(1_700_000_000_000),
            }
        );
        let legacy = save(
            root.path(),
            "Old",
            vec![("RandomSeed", Tag::Long(42)), ("GameType", Tag::Int(1))],
        );
        let details = legacy.details().unwrap();
        assert_eq!(details.display_name, "Old");
        assert_eq!(details.game_mode, Some(GameMode::Creative));
        assert_eq!(details.seed, Some(42));
        assert!(!details.hardcore);
    }

    #[test]
    fn rename_sets_level_name_and_moves_to_a_free_folder() {
        let root = tempfile::tempdir().unwrap();
        let world = save(root.path(), "New World", named("New World"));
        let other = save(root.path(), "Base", named("Other Base"));
        let before = snapshot(&root.path().join("saves/Base"));

        let renamed = world.rename("  Base  ").unwrap();
        assert_eq!(renamed.name(), "Base (2)");
        assert_eq!(renamed.details().unwrap().display_name, "Base");
        assert!(!root.path().join("saves/New World").exists());
        assert_eq!(snapshot(&root.path().join("saves/Base")), before);
        assert_eq!(other.details().unwrap().display_name, "Other Base");

        let same_folder = renamed.rename("Base").unwrap();
        assert_eq!(same_folder.name(), "Base (2)");
        let unsafe_name = same_folder.rename("a/b: c?").unwrap();
        assert_eq!(unsafe_name.name(), "a_b_ c_");
        assert_eq!(raw_level_name(&unsafe_name), "a/b: c?");
        assert!(unsafe_name.rename(" ").is_err());
    }

    #[test]
    fn copy_never_clobbers_and_leaves_the_source_unchanged() {
        let root = tempfile::tempdir().unwrap();
        let world = save(root.path(), "Survival", named("Survival"));
        let taken = root.path().join("saves/Survival Copy");
        fs::create_dir(&taken).unwrap();
        fs::write(taken.join("keep.txt"), "mine").unwrap();
        let before = snapshot(&root.path().join("saves/Survival"));

        let copy = world.copy("Survival Copy").unwrap();
        assert_eq!(copy.name(), "Survival Copy (2)");
        assert_eq!(copy.details().unwrap().display_name, "Survival Copy");
        assert_eq!(
            fs::read(copy.path().unwrap().join("region/r.0.0.mca")).unwrap(),
            b"Survival"
        );
        assert_eq!(fs::read_to_string(taken.join("keep.txt")).unwrap(), "mine");
        assert_eq!(snapshot(&root.path().join("saves/Survival")), before);
        assert_eq!(world.details().unwrap().display_name, "Survival");
    }

    #[cfg(unix)]
    #[test]
    fn copy_does_not_follow_symlinks() {
        let root = tempfile::tempdir().unwrap();
        let outside = root.path().join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("secret"), "x").unwrap();
        let world = save(root.path(), "Linked", named("Linked"));
        let path = world.path().unwrap();
        std::os::unix::fs::symlink(&outside, path.join("linked-dir")).unwrap();
        std::os::unix::fs::symlink(outside.join("secret"), path.join("linked-file")).unwrap();

        let copy = world.copy("Linked").unwrap();
        let copied = copy.path().unwrap();
        assert!(fs::symlink_metadata(copied.join("linked-dir")).is_err());
        assert!(fs::symlink_metadata(copied.join("linked-file")).is_err());
        assert!(copied.join("region/r.0.0.mca").is_file());
    }

    #[test]
    fn delete_removes_only_the_selected_world() {
        let root = tempfile::tempdir().unwrap();
        let world = save(root.path(), "Doomed", named("Doomed"));
        save(root.path(), "Keep", named("Keep"));
        let before = snapshot(&root.path().join("saves/Keep"));
        world.delete().unwrap();
        let left: Vec<_> = fs::read_dir(root.path().join("saves"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(left, ["Keep"]);
        assert_eq!(snapshot(&root.path().join("saves/Keep")), before);
    }

    #[test]
    fn stale_targets_are_rejected() {
        let root = tempfile::tempdir().unwrap();
        let world = save(root.path(), "Shared", named("Shared"));
        let other_window = World::open(root.path(), "Shared").unwrap();
        other_window.rename("Moved").unwrap();
        assert!(world.rename("Again").is_err());
        assert!(world.copy("Again").is_err());
        assert!(world.delete().is_err());
        assert!(root.path().join("saves/Moved/level.dat").is_file());

        let world = World::open(root.path(), "Moved").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(
            world.path().unwrap().join("level.dat"),
            level_dat(named("Changed")),
        )
        .unwrap();
        assert!(world.delete().is_err());
        assert!(root.path().join("saves/Moved").is_dir());
    }
}
