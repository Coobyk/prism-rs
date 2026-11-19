use crate::modplatform::ResourceType;
use crate::nbt::{self, Tag};
use flate2::read::GzDecoder;
use std::{
    collections::HashSet,
    fs, io,
    io::Read,
    path::{Path, PathBuf},
    time::SystemTime,
};

const DISABLED: &str = ".disabled";
const DISABLED_FOLDERS: &str = "datapacks.disabled";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FileStamp {
    created: Option<SystemTime>,
    modified: Option<SystemTime>,
    len: u64,
    #[cfg(unix)]
    id: (u64, u64),
}

impl From<&fs::Metadata> for FileStamp {
    fn from(meta: &fs::Metadata) -> Self {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        Self {
            created: meta.created().ok(),
            modified: meta.modified().ok(),
            len: meta.len(),
            #[cfg(unix)]
            id: (meta.dev(), meta.ino()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct World {
    pub(crate) game_root: PathBuf,
    pub(crate) name: String,
    pub(crate) level_stamp: FileStamp,
}

#[derive(Clone, Debug)]
pub struct DataPack {
    folder: PathBuf,
    name: String,
    directory: bool,
    enabled: bool,
    disabled_in_world: bool,
    stamp: FileStamp,
}

impl DataPack {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn enabled(&self) -> bool {
        self.enabled && !self.disabled_in_world
    }

    pub fn disabled_in_world(&self) -> bool {
        self.disabled_in_world
    }
}

pub(crate) fn plain_name(name: &str) -> bool {
    Path::new(name).file_name().and_then(|part| part.to_str()) == Some(name)
        && !name.contains(['/', '\\', ':'])
        && !name.chars().any(char::is_control)
}

fn zip_name(name: &str) -> bool {
    plain_name(name) && name.ends_with(".zip")
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

pub(crate) fn level_stamp(world: &Path) -> io::Result<FileStamp> {
    require_directory(world)?;
    let metadata = fs::symlink_metadata(world.join("level.dat"))?;
    if !metadata.is_file() {
        return Err(io::Error::other("Not a Java world (no regular level.dat)"));
    }
    Ok(FileStamp::from(&metadata))
}

pub(crate) fn nbt_child<'a>(root: &'a Tag, name: &str) -> Option<&'a Tag> {
    let Tag::Compound(entries) = root else {
        return None;
    };
    entries
        .iter()
        .find_map(|(key, value)| (key == name).then_some(value))
}

fn pack_names<'a>(packs: &'a Tag, key: &str) -> impl Iterator<Item = &'a str> {
    let values = match nbt_child(packs, key) {
        Some(Tag::List(8, names)) => names.as_slice(),
        _ => &[],
    };
    values.iter().filter_map(|tag| match tag {
        Tag::String(name) => name.strip_prefix("file/"),
        _ => None,
    })
}

pub(crate) fn read_gzip_nbt(path: &Path) -> io::Result<(String, Tag)> {
    const MAX_LEVEL_NBT: u64 = 16 * 1024 * 1024;
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

fn disabled_world_packs(path: &Path) -> io::Result<HashSet<String>> {
    let (_, root) = read_gzip_nbt(path)?;
    let Some(packs) = nbt_child(&root, "Data").and_then(|data| nbt_child(data, "DataPacks")) else {
        return Ok(HashSet::new());
    };
    let enabled: HashSet<_> = pack_names(packs, "Enabled").collect();
    Ok(pack_names(packs, "Disabled")
        .filter(|name| !enabled.contains(name))
        .map(str::to_owned)
        .collect())
}

pub(crate) fn ensure_absent(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!(
                "{} already exists",
                path.file_name().unwrap_or_default().to_string_lossy()
            ),
        )),
    }
}

impl World {
    pub fn open(game_root: &Path, name: &str) -> io::Result<Self> {
        if !plain_name(name) {
            return Err(io::Error::other("Invalid world name"));
        }
        let saves = game_root.join("saves");
        require_directory(&saves)?;
        let level_stamp = level_stamp(&saves.join(name))?;
        Ok(Self {
            game_root: game_root.into(),
            name: name.into(),
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
            if let Ok(level_stamp) = level_stamp(&entry.path()) {
                worlds.push(Self {
                    game_root: game_root.into(),
                    name,
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

    pub(crate) fn world_path(&self) -> io::Result<PathBuf> {
        let saves = self.game_root.join("saves");
        require_directory(&saves)?;
        let world = saves.join(&self.name);
        let stamp = match level_stamp(&world) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            stamp => Some(stamp?),
        };
        if stamp.as_ref() != Some(&self.level_stamp) {
            return Err(io::Error::other(
                "The selected world changed. Refresh and try again.",
            ));
        }
        Ok(world)
    }

    pub fn ensure_pack_folder(&self) -> io::Result<PathBuf> {
        Self::ensure_folder(self.world_path()?.join(ResourceType::DataPack.folder()))
    }

    fn ensure_folder(folder: PathBuf) -> io::Result<PathBuf> {
        match fs::create_dir(&folder) {
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                require_directory(&folder)?
            }
            result => result?,
        }
        Ok(folder)
    }

    pub fn packs(&self) -> io::Result<Vec<DataPack>> {
        let world = self.world_path()?;
        let mut packs = Vec::new();
        Self::scan(
            &world.join(ResourceType::DataPack.folder()),
            true,
            &mut packs,
        )?;
        Self::scan(&world.join(DISABLED_FOLDERS), false, &mut packs)?;
        if packs.iter().any(|pack| pack.enabled) {
            if let Ok(disabled) = disabled_world_packs(&world.join("level.dat")) {
                for pack in &mut packs {
                    pack.disabled_in_world = pack.enabled && disabled.contains(&pack.name);
                }
            }
        }
        packs.sort_by_key(|pack| (pack.name.to_lowercase(), pack.name.clone(), !pack.enabled));
        Ok(packs)
    }

    fn scan(folder: &Path, active_folder: bool, packs: &mut Vec<DataPack>) -> io::Result<()> {
        match require_directory(folder) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            result => result?,
        }
        for entry in fs::read_dir(folder)? {
            let entry = entry?;
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            if !plain_name(&name) {
                continue;
            }
            let meta = fs::symlink_metadata(entry.path())?;
            let directory = meta.is_dir();
            if (active_folder
                && meta.is_file()
                && zip_name(name.strip_suffix(DISABLED).unwrap_or(&name)))
                || (directory && entry.path().join("pack.mcmeta").is_file())
            {
                packs.push(DataPack {
                    folder: folder.into(),
                    enabled: active_folder && (directory || !name.ends_with(DISABLED)),
                    disabled_in_world: false,
                    name,
                    directory,
                    stamp: FileStamp::from(&meta),
                });
            }
        }
        Ok(())
    }

    pub fn add_zip(&self, source: &Path) -> io::Result<()> {
        let name = source
            .file_name()
            .and_then(|part| part.to_str())
            .filter(|name| zip_name(name))
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "Choose a data pack .zip file")
            })?;
        let mut file = fs::File::open(source)?;
        let mut zip = zip::ZipArchive::new(&mut file)?;
        if !zip
            .by_name("pack.mcmeta")
            .is_ok_and(|entry| entry.is_file())
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "The ZIP must contain pack.mcmeta at its root",
            ));
        }
        drop(zip);
        use io::Seek;
        file.rewind()?;
        let folder = self.ensure_pack_folder()?;
        let target = folder.join(name);
        ensure_absent(&target)?;
        ensure_absent(&folder.join(format!("{name}{DISABLED}")))?;
        let mut temporary = tempfile::NamedTempFile::new_in(folder)?;
        io::copy(&mut file, &mut temporary)?;
        temporary
            .persist_noclobber(target)
            .map_err(|error| error.error)?;
        Ok(())
    }

    fn pack_path(world: &Path, pack: &DataPack) -> io::Result<PathBuf> {
        let folder = world.join(if pack.directory && !pack.enabled {
            DISABLED_FOLDERS
        } else {
            ResourceType::DataPack.folder()
        });
        if folder != pack.folder {
            return Err(io::Error::other("The pack belongs to another world"));
        }
        require_directory(&folder)?;
        let path = folder.join(&pack.name);
        let meta = fs::symlink_metadata(&path)?;
        if (if pack.directory {
            !meta.is_dir()
        } else {
            !meta.is_file()
        }) || FileStamp::from(&meta) != pack.stamp
        {
            return Err(io::Error::other(
                "The pack changed on disk. Refresh and try again.",
            ));
        }
        Ok(path)
    }

    pub fn toggle(&self, pack: &DataPack) -> io::Result<()> {
        if pack.disabled_in_world() {
            return Err(io::Error::other("This pack is disabled in Minecraft"));
        }
        let world = self.world_path()?;
        let path = Self::pack_path(&world, pack)?;
        let target = if pack.directory {
            let folder = if pack.enabled {
                DISABLED_FOLDERS
            } else {
                ResourceType::DataPack.folder()
            };
            Self::ensure_folder(world.join(folder))?.join(&pack.name)
        } else if let Some(name) = pack.name.strip_suffix(DISABLED) {
            path.with_file_name(name)
        } else {
            path.with_file_name(format!("{}{DISABLED}", pack.name))
        };
        ensure_absent(&target)?;
        fs::rename(path, target)
    }

    pub fn remove(&self, pack: &DataPack) -> io::Result<()> {
        let path = Self::pack_path(&self.world_path()?, pack)?;
        if pack.directory {
            fs::remove_dir_all(path)
        } else {
            fs::remove_file(path)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn world(root: &Path, name: &str) -> PathBuf {
        let world = root.join("saves").join(name);
        fs::create_dir_all(&world).unwrap();
        fs::write(world.join("level.dat"), name).unwrap();
        world
    }

    fn zip(root: &Path, name: &str, entry: &str) -> PathBuf {
        let source = root.join(name);
        let mut zip = zip::ZipWriter::new(fs::File::create(&source).unwrap());
        zip.start_file(entry, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(br#"{"pack":{"pack_format":15,"description":"Test"}}"#)
            .unwrap();
        zip.finish().unwrap();
        source
    }

    #[test]
    fn pack_edits_are_isolated_to_the_selected_world() {
        let root = tempfile::tempdir().unwrap();
        let a = world(root.path(), "Alpha");
        let b = world(root.path(), "Beta");
        fs::create_dir(a.parent().unwrap().join("Not a world")).unwrap();
        let worlds = World::list(root.path()).unwrap();
        assert_eq!(
            worlds.iter().map(World::name).collect::<Vec<_>>(),
            ["Alpha", "Beta"]
        );
        let (alpha, beta) = (&worlds[0], &worlds[1]);
        let pack = zip(root.path(), "alpha.zip", "pack.mcmeta");
        let beta_pack = beta.ensure_pack_folder().unwrap().join("b-folder");
        fs::create_dir(&beta_pack).unwrap();
        fs::write(beta_pack.join("pack.mcmeta"), "{}").unwrap();
        assert!(alpha.packs().unwrap().is_empty());
        assert!(!a.join("datapacks").exists());
        alpha.add_zip(&pack).unwrap();
        let installed = &alpha.packs().unwrap()[0];
        assert!(installed.enabled());
        assert!(beta.remove(installed).is_err());
        alpha.toggle(installed).unwrap();
        let installed = &alpha.packs().unwrap()[0];
        assert!(!installed.enabled());
        assert_eq!(
            alpha.add_zip(&pack).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        alpha.toggle(installed).unwrap();
        alpha.remove(&alpha.packs().unwrap()[0]).unwrap();
        assert!(alpha.packs().unwrap().is_empty());
        assert!(b.join("datapacks/b-folder/pack.mcmeta").is_file());
        beta.toggle(&beta.packs().unwrap()[0]).unwrap();
        assert!(!beta.packs().unwrap()[0].enabled());
        beta.remove(&beta.packs().unwrap()[0]).unwrap();
        assert!(beta.packs().unwrap().is_empty());
        assert!(!root.path().join("datapacks").exists());
    }

    #[test]
    fn rejects_invalid_archives_collisions_and_stale_actions() {
        let root = tempfile::tempdir().unwrap();
        let save = world(root.path(), "Save");
        let selected = &World::list(root.path()).unwrap()[0];
        let bad = zip(root.path(), "bad.zip", "nested/pack.mcmeta");
        assert!(selected.add_zip(&bad).is_err());
        assert!(!save.join("datapacks").exists());
        let source = zip(root.path(), "pack.zip", "pack.mcmeta");
        selected.add_zip(&source).unwrap();
        let installed = &selected.packs().unwrap()[0];
        let folder = save.join("datapacks");
        fs::write(folder.join("pack.zip.disabled"), "existing pack").unwrap();
        assert_eq!(
            selected.toggle(installed).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        fs::write(folder.join("pack.zip"), "changed since listing").unwrap();
        assert!(selected.remove(installed).is_err());
        assert_eq!(
            fs::read_to_string(folder.join("pack.zip")).unwrap(),
            "changed since listing"
        );
        fs::rename(&save, save.with_file_name("Old Save")).unwrap();
        world(root.path(), "Save");
        assert!(selected.add_zip(&source).is_err());
        assert!(!folder.exists());
    }

    #[test]
    fn disabled_zip_and_folder_packs_are_outside_minecraft_discovery() {
        let root = tempfile::tempdir().unwrap();
        let save = world(root.path(), "Save");
        let selected = World::open(root.path(), "Save").unwrap();
        let active = selected.ensure_pack_folder().unwrap();
        selected
            .add_zip(&zip(root.path(), "pack.zip", "pack.mcmeta"))
            .unwrap();
        let unpacked = active.join("unpacked.disabled");
        fs::create_dir(&unpacked).unwrap();
        fs::write(unpacked.join("pack.mcmeta"), "{}").unwrap();
        let packs = selected.packs().unwrap();
        assert_eq!(packs.len(), 2);
        assert!(packs.iter().all(DataPack::enabled));
        for pack in packs {
            selected.toggle(&pack).unwrap();
        }

        // Minecraft accepts directories with pack.mcmeta regardless of their name,
        // and regular files only when their filename ends in .zip.
        assert!(fs::read_dir(&active).unwrap().all(|entry| {
            let path = entry.unwrap().path();
            !(path.is_dir() && path.join("pack.mcmeta").is_file())
                && !(path.is_file() && path.extension().is_some_and(|ext| ext == "zip"))
        }));
        assert!(active.join("pack.zip.disabled").is_file());
        assert!(save
            .join("datapacks.disabled/unpacked.disabled/pack.mcmeta")
            .is_file());
        let packs = selected.packs().unwrap();
        assert_eq!(packs.len(), 2);
        assert!(packs.iter().all(|pack| !pack.enabled()));
        for pack in packs {
            selected.toggle(&pack).unwrap();
        }
        assert!(active.join("pack.zip").is_file());
        assert!(unpacked.join("pack.mcmeta").is_file());
    }

    #[test]
    fn reads_in_game_disabled_packs_without_editing_level_dat() {
        let root = tempfile::tempdir().unwrap();
        let save = world(root.path(), "Save");
        let level_path = save.join("level.dat");
        let list = |packs: &[&str]| {
            Tag::List(
                8,
                packs
                    .iter()
                    .map(|name| Tag::String((*name).into()))
                    .collect(),
            )
        };
        let nbt = Tag::Compound(vec![(
            "Data".into(),
            Tag::Compound(vec![(
                "DataPacks".into(),
                Tag::Compound(vec![
                    ("Enabled".into(), list(&["vanilla", "file/active.zip"])),
                    (
                        "Disabled".into(),
                        list(&["file/disabled.zip", "file/active.zip"]),
                    ),
                ]),
            )]),
        )]);
        let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gzip.write_all(&nbt::write("", &nbt).unwrap()).unwrap();
        let level = gzip.finish().unwrap();
        fs::write(&level_path, &level).unwrap();
        let world = World::open(root.path(), "Save").unwrap();
        for name in ["active.zip", "disabled.zip", "new.zip"] {
            world
                .add_zip(&zip(root.path(), name, "pack.mcmeta"))
                .unwrap();
        }
        let packs = world.packs().unwrap();
        let disabled = packs
            .iter()
            .find(|pack| pack.name() == "disabled.zip")
            .unwrap();
        assert!(disabled.disabled_in_world());
        assert!(!disabled.enabled());
        assert!(world.toggle(disabled).is_err());
        assert!(save.join("datapacks/disabled.zip").is_file());
        assert!(packs
            .iter()
            .filter(|pack| pack.name() != "disabled.zip")
            .all(DataPack::enabled));
        assert_eq!(fs::read(level_path).unwrap(), level);
    }

    #[test]
    fn unpacked_packs_are_not_overwritten_or_removed_by_a_stale_action() {
        let root = tempfile::tempdir().unwrap();
        let save = world(root.path(), "Save");
        let selected = World::open(root.path(), "Save").unwrap();
        let active = selected.ensure_pack_folder().unwrap().join("unpacked");
        let disabled = save.join("datapacks.disabled/unpacked");
        fs::create_dir(&active).unwrap();
        fs::write(active.join("pack.mcmeta"), "{}").unwrap();
        let pack = selected.packs().unwrap().pop().unwrap();
        fs::create_dir_all(&disabled).unwrap();
        assert_eq!(
            selected.toggle(&pack).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        fs::remove_dir(&disabled).unwrap();
        selected.toggle(&pack).unwrap();
        let disabled_pack = selected.packs().unwrap().pop().unwrap();
        fs::create_dir(&active).unwrap();
        assert_eq!(
            selected.toggle(&disabled_pack).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        fs::rename(&disabled, disabled.with_file_name("old-unpacked")).unwrap();
        fs::create_dir(&disabled).unwrap();
        fs::write(disabled.join("pack.mcmeta"), "replacement").unwrap();
        assert!(selected.remove(&disabled_pack).is_err());
        assert!(disabled.join("pack.mcmeta").is_file());
    }

    #[cfg(unix)]
    #[test]
    fn does_not_follow_world_or_pack_folder_symlinks() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let a = world(root.path(), "Alpha");
        let b = world(root.path(), "Beta");
        symlink(&b, a.with_file_name("Alias")).unwrap();
        let worlds = World::list(root.path()).unwrap();
        assert_eq!(worlds.len(), 2);
        let target = worlds[1].ensure_pack_folder().unwrap();
        symlink(&target, a.join("datapacks")).unwrap();
        assert!(worlds[0]
            .add_zip(&zip(root.path(), "a.zip", "pack.mcmeta"))
            .is_err());
        assert!(worlds[1].packs().unwrap().is_empty());
        fs::remove_file(a.join("datapacks")).unwrap();
        let folder = worlds[0].ensure_pack_folder().unwrap().join("folder");
        fs::create_dir(&folder).unwrap();
        fs::write(folder.join("pack.mcmeta"), "{}").unwrap();
        let pack = worlds[0].packs().unwrap().pop().unwrap();
        symlink(&target, a.join("datapacks.disabled")).unwrap();
        assert!(worlds[0].toggle(&pack).is_err());
        assert!(folder.join("pack.mcmeta").is_file());
        assert!(worlds[1].packs().unwrap().is_empty());
    }
}
