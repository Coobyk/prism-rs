use crate::modplatform::ResourceType;
use std::{
    fs, io,
    path::{Path, PathBuf},
    time::SystemTime,
};

const DISABLED: &str = ".disabled";

#[derive(Clone, Debug, PartialEq, Eq)]
struct FileStamp {
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
    game_root: PathBuf,
    name: String,
    level_stamp: FileStamp,
}

#[derive(Clone, Debug)]
pub struct DataPack {
    folder: PathBuf,
    name: String,
    directory: bool,
    stamp: FileStamp,
}

impl DataPack {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn enabled(&self) -> bool {
        !self.name.ends_with(DISABLED)
    }
}

fn plain_name(name: &str) -> bool {
    Path::new(name).file_name().and_then(|part| part.to_str()) == Some(name)
        && !name.contains(['/', '\\', ':'])
        && !name.chars().any(char::is_control)
}

fn zip_name(name: &str) -> bool {
    plain_name(name) && name.ends_with(".zip")
}

fn require_directory(path: &Path) -> io::Result<()> {
    if fs::symlink_metadata(path)?.is_dir() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "Not a regular directory: {}",
            path.display()
        )))
    }
}

fn level_stamp(world: &Path) -> io::Result<FileStamp> {
    require_directory(world)?;
    let metadata = fs::symlink_metadata(world.join("level.dat"))?;
    if !metadata.is_file() {
        return Err(io::Error::other("Not a Java world (no regular level.dat)"));
    }
    Ok(FileStamp::from(&metadata))
}

fn ensure_absent(path: &Path) -> io::Result<()> {
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

    fn folder_path(&self) -> io::Result<PathBuf> {
        let saves = self.game_root.join("saves");
        require_directory(&saves)?;
        let world = saves.join(&self.name);
        if level_stamp(&world)? != self.level_stamp {
            return Err(io::Error::other(
                "The selected world changed. Refresh and try again.",
            ));
        }
        Ok(world.join(ResourceType::DataPack.folder()))
    }

    pub fn ensure_pack_folder(&self) -> io::Result<PathBuf> {
        let folder = self.folder_path()?;
        match fs::create_dir(&folder) {
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                require_directory(&folder)?
            }
            result => result?,
        }
        Ok(folder)
    }

    pub fn packs(&self) -> io::Result<Vec<DataPack>> {
        let folder = self.folder_path()?;
        match require_directory(&folder) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            result => result?,
        }
        let mut packs = Vec::new();
        for entry in fs::read_dir(&folder)? {
            let entry = entry?;
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            if !plain_name(&name) {
                continue;
            }
            let meta = fs::symlink_metadata(entry.path())?;
            let directory = meta.is_dir();
            if (meta.is_file() && zip_name(name.strip_suffix(DISABLED).unwrap_or(&name)))
                || (directory && entry.path().join("pack.mcmeta").is_file())
            {
                packs.push(DataPack {
                    folder: folder.clone(),
                    name,
                    directory,
                    stamp: FileStamp::from(&meta),
                });
            }
        }
        packs.sort_by_key(|pack| (pack.name.to_lowercase(), pack.name.clone()));
        Ok(packs)
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

    fn pack_path(&self, pack: &DataPack) -> io::Result<PathBuf> {
        let folder = self.folder_path()?;
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
        let path = self.pack_path(pack)?;
        let name = match pack.name.strip_suffix(DISABLED) {
            Some(enabled_name) if plain_name(enabled_name) => enabled_name.to_owned(),
            Some(_) => return Err(io::Error::other("Invalid pack name")),
            None => format!("{}{DISABLED}", pack.name),
        };
        let target = path.with_file_name(name);
        ensure_absent(&target)?;
        fs::rename(path, target)
    }

    pub fn remove(&self, pack: &DataPack) -> io::Result<()> {
        let path = self.pack_path(pack)?;
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
    }
}
