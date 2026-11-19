use crate::{
    import::{extract_zip, ImportError},
    loaders::Loader,
    modplatform::modrinth::{ModFile, MrpackIndex, API_BASE},
    net::{DownloadRequest, Downloader},
    Instance, InstanceList, Paths,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::{Component, Path, PathBuf},
    sync::Arc,
};

pub(crate) const RECORD: &str = ".prism-rs-managed-pack.json";
const COMPONENTS: &str = "mmc-pack.json";

#[derive(Clone, Debug, Deserialize)]
pub struct Project {
    pub id: String,
    pub title: String,
    project_type: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Version {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub version_number: String,
    #[serde(default)]
    pub changelog: Option<String>,
    files: Vec<ModFile>,
}

impl Version {
    fn archive(&self) -> Option<&ModFile> {
        let files = self
            .files
            .iter()
            .filter(|file| file.filename.ends_with(".mrpack"));
        files
            .clone()
            .find(|file| file.primary)
            .or_else(|| files.into_iter().next())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ManagedPack {
    pub project_id: String,
    pub name: String,
    pub version_id: String,
    pub version: String,
    files: BTreeMap<PathBuf, OwnedFile>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
enum OwnedFile {
    Download(String),
    Override,
}

fn problem(message: impl Into<String>) -> ImportError {
    ImportError::Other(message.into())
}

fn provider_url(segments: &[&str]) -> reqwest::Url {
    let mut url = reqwest::Url::parse(API_BASE).unwrap();
    url.path_segments_mut().unwrap().extend(segments);
    url
}

async fn get<T: serde::de::DeserializeOwned>(
    dl: &Downloader,
    segments: &[&str],
) -> Result<T, ImportError> {
    async {
        dl.client()
            .get(provider_url(segments))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await
    }
    .await
    .map_err(crate::modplatform::PlatformError::from)
    .map_err(Into::into)
}

pub fn import_source(value: &str) -> Option<(String, Option<String>)> {
    let url = reqwest::Url::parse(value).ok()?;
    if url.scheme() != "https"
        || !matches!(url.host_str(), Some("modrinth.com" | "www.modrinth.com"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
    {
        return None;
    }
    let segments: Vec<_> = url.path_segments()?.filter(|s| !s.is_empty()).collect();
    let (project, version) = match segments.as_slice() {
        ["modpack", project] => (*project, None),
        ["modpack", project, "version", version] => (*project, Some(*version)),
        _ => return None,
    };
    let valid = |value: &str| {
        value
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, b'-' | b'_' | b'.'))
            && value != "."
            && value != ".."
    };
    (valid(project) && version.is_none_or(valid))
        .then(|| (project.to_owned(), version.map(str::to_owned)))
}

pub async fn versions(dl: &Downloader, project: &str) -> Result<Vec<Version>, ImportError> {
    let mut versions: Vec<Version> = get(dl, &["project", project, "version"]).await?;
    versions.retain(|version| version.project_id == project && version.archive().is_some());
    Ok(versions)
}

pub async fn install_modrinth(
    paths: &Paths,
    dl: &Arc<Downloader>,
    project: &str,
    requested: Option<&str>,
) -> Result<Instance, ImportError> {
    let project: Project = get(dl, &["project", project]).await?;
    if project.project_type != "modpack" {
        return Err(problem("The Modrinth project is not a modpack."));
    }
    let versions = versions(dl, &project.id).await?;
    let version = versions
        .iter()
        .find(|version| requested.is_none_or(|id| id == version.id || id == version.version_number))
        .ok_or_else(|| problem("No matching Modrinth modpack release."))?;
    let stage = Staged::download(dl, &project.id, &project.title, version).await?;
    let title = match project.title.trim() {
        "" | "." | ".." => "Imported Modpack",
        title => title,
    };
    let instance = InstanceList::create(paths, title, "default", &stage.minecraft)?;
    let result = (|| {
        let game = instance.game_root();
        for path in stage.pack.files.keys() {
            let target = game.join(path);
            fs::create_dir_all(target.parent().unwrap())?;
            fs::copy(stage.payload().join(path), target)?;
        }
        fs::write(
            instance.root.join(COMPONENTS),
            stage.components(&fs::read(instance.root.join(COMPONENTS))?)?,
        )?;
        fs::write(
            instance.root.join(RECORD),
            serde_json::to_vec_pretty(&stage.pack)?,
        )?;
        Ok(instance.clone())
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&instance.root);
    }
    result
}

impl ManagedPack {
    pub fn present(instance: &Instance) -> bool {
        !instance.is_horizon() && instance.root.join(RECORD).is_file()
    }
}

pub struct Snapshot {
    pub pack: ManagedPack,
    instance: Instance,
    game: PathBuf,
    token: String,
    record: Vec<u8>,
    components: Vec<u8>,
}

impl Snapshot {
    pub fn load(instance: Instance) -> Result<Self, ImportError> {
        if instance.is_horizon()
            || !Instance::load(&instance.root).is_some_and(|instance| !instance.is_horizon())
        {
            return Err(problem("The Java instance no longer exists."));
        }
        require_directory(&instance.root)?;
        let game = instance.game_root();
        require_directory(&game)?;
        let record_path = instance.root.join(RECORD);
        let components_path = instance.root.join(COMPONENTS);
        require_file(&record_path)?;
        require_file(&components_path)?;
        let record = fs::read(record_path)?;
        let pack: ManagedPack = serde_json::from_slice(&record)?;
        for path in pack.files.keys() {
            normalize_relative(path)?;
        }
        let components = fs::read(components_path)?;
        let token = instance.ensure_shortcut_token()?;
        Ok(Self {
            pack,
            instance,
            game,
            token,
            record,
            components,
        })
    }

    pub async fn update(
        &self,
        dl: &Arc<Downloader>,
        version: &Version,
    ) -> Result<ManagedPack, ImportError> {
        self.check_current()?;
        if version.id == self.pack.version_id {
            return Err(problem("That pack release is already installed."));
        }
        let stage = Staged::download(dl, &self.pack.project_id, &self.pack.name, version).await?;
        self.apply(stage)
    }

    fn check_current(&self) -> Result<(), ImportError> {
        let stale = || problem("The managed pack or instance changed. Refresh before updating.");
        require_directory(&self.instance.root)?;
        require_directory(&self.game)?;
        require_file(&self.instance.root.join(RECORD))?;
        require_file(&self.instance.root.join(COMPONENTS))?;
        let live = Instance::load(&self.instance.root).ok_or_else(stale)?;
        if live.is_horizon()
            || live.game_root() != self.game
            || live.shortcut_token()? != self.token
            || fs::read(live.root.join(RECORD))? != self.record
            || fs::read(live.root.join(COMPONENTS))? != self.components
        {
            return Err(stale());
        }
        Ok(())
    }

    fn apply(&self, mut stage: Staged) -> Result<ManagedPack, ImportError> {
        self.check_current()?;
        let mut owned = BTreeSet::new();
        let mut changes = BTreeMap::new();
        for (path, old) in &self.pack.files {
            if path.starts_with("saves") {
                continue;
            }
            let moved = if let OwnedFile::Download(hash) = old {
                alternate(path).filter(|relative| {
                    let target = self.game.join(relative);
                    !self.game.join(path).exists()
                        && existing_file(&self.game, &target).is_ok_and(|exists| exists)
                        && Downloader::verify_sha1(&target, hash).is_ok()
                })
            } else {
                None
            };
            let variants = std::iter::once(path.clone()).chain(moved);
            let new = stage.pack.files.get(path);
            for relative in variants {
                let target = self.game.join(&relative);
                owned.insert(relative);
                if existing_file(&self.game, &target)? {
                    if let (OwnedFile::Download(old_hash), Some(OwnedFile::Download(new_hash))) =
                        (old, new)
                    {
                        if old_hash == new_hash
                            && Downloader::verify_sha1(&target, new_hash).is_ok()
                        {
                            continue;
                        }
                    }
                    changes.insert(target, None);
                }
            }
        }
        let mut kept_saves = Vec::new();
        for (path, new) in &stage.pack.files {
            let target = self.game.join(path);
            if existing_file(&self.game, &target)? {
                if path.starts_with("saves") {
                    kept_saves.push(path.clone());
                    continue;
                }
                if !owned.contains(path) {
                    return Err(problem(format!(
                        "A user file conflicts with the new pack: {}",
                        path.display()
                    )));
                }
            }
            if let Some(OwnedFile::Download(old_hash)) = self.pack.files.get(path) {
                if let OwnedFile::Download(new_hash) = new {
                    if old_hash == new_hash {
                        let variants = std::iter::once(target.clone()).chain(
                            alternate(path)
                                .filter(|p| owned.contains(p))
                                .map(|p| self.game.join(p)),
                        );
                        if variants.into_iter().any(|p| {
                            existing_file(&self.game, &p).is_ok_and(|exists| exists)
                                && Downloader::verify_sha1(&p, new_hash).is_ok()
                        }) {
                            continue;
                        }
                    }
                }
            }
            changes.insert(target, Some(stage.payload().join(path)));
        }
        for path in kept_saves {
            stage.pack.files.remove(&path);
        }
        let components = stage.directory.path().join("components.json");
        let record = stage.directory.path().join("record.json");
        fs::write(&components, stage.components(&self.components)?)?;
        fs::write(&record, serde_json::to_vec_pretty(&stage.pack)?)?;
        changes.insert(self.instance.root.join(COMPONENTS), Some(components));
        changes.insert(self.instance.root.join(RECORD), Some(record));
        self.check_current()?;
        apply_changes(&self.instance.root, changes)?;
        Ok(stage.pack)
    }
}

struct Staged {
    directory: tempfile::TempDir,
    pack: ManagedPack,
    minecraft: String,
    loader: Loader,
}

impl Staged {
    fn payload(&self) -> PathBuf {
        self.directory.path().join("payload")
    }

    async fn download(
        dl: &Arc<Downloader>,
        project: &str,
        name: &str,
        version: &Version,
    ) -> Result<Self, ImportError> {
        if project != version.project_id {
            return Err(problem("The release belongs to a different modpack."));
        }
        let archive = version
            .archive()
            .ok_or_else(|| problem("The release has no .mrpack file."))?;
        let directory = tempfile::tempdir()?;
        let zip = directory.path().join("pack.mrpack");
        let unpacked = directory.path().join("archive");
        dl.download(&download_request(&archive.url, &zip, &archive.hashes)?)
            .await?;
        extract_zip(&zip, &unpacked)?;
        let index = MrpackIndex::from_dir(&unpacked)?;
        if index.game != "minecraft" || index.format_version != 1 {
            return Err(problem("Unsupported Modrinth modpack format."));
        }
        let minecraft = index
            .minecraft_version()
            .filter(|v| !v.trim().is_empty())
            .ok_or_else(|| problem("The modpack has no Minecraft version."))?
            .to_owned();
        let loader = match index.loader() {
            Some((loader, version)) => match loader.as_str() {
                "fabric-loader" => Loader::Fabric { version },
                "quilt-loader" => Loader::Quilt { version },
                "forge" => Loader::Forge { version },
                "neoforge" => Loader::NeoForge { version },
                _ => return Err(problem("Unsupported modpack loader.")),
            },
            None => Loader::None,
        };
        let mut stage = Self {
            directory,
            minecraft,
            loader,
            pack: ManagedPack {
                project_id: project.to_owned(),
                name: name.to_owned(),
                version_id: version.id.clone(),
                version: version.version_number.clone(),
                files: BTreeMap::new(),
            },
        };
        let mut requests = Vec::new();
        for file in index.files {
            if file
                .env
                .as_ref()
                .and_then(|env| env.get("client"))
                .is_some_and(|env| env == "unsupported")
            {
                continue;
            }
            let path = safe_relative(&file.path)?;
            let url = file
                .downloads
                .first()
                .ok_or_else(|| problem(format!("No download for {}", file.path)))?;
            let request = download_request(url, &stage.payload().join(&path), &file.hashes)?;
            let hash = file.hashes["sha1"].to_ascii_lowercase();
            if stage
                .pack
                .files
                .insert(path, OwnedFile::Download(hash))
                .is_some()
            {
                return Err(problem("The modpack lists the same download path twice."));
            }
            requests.push(request);
        }
        dl.download_all(requests, 8, |_| {}).await?;
        for dir in ["overrides", "client-overrides"] {
            let dir = unpacked.join(dir);
            if dir.is_dir() {
                let payload = stage.payload();
                apply_overrides(&dir, &dir, &payload, &mut stage.pack.files)?;
            }
        }
        Ok(stage)
    }

    fn components(&self, current: &[u8]) -> Result<Vec<u8>, ImportError> {
        let mut pack: Value = serde_json::from_slice(current)?;
        let components = pack["components"]
            .as_array_mut()
            .ok_or_else(|| problem("Invalid instance components."))?;
        let replaced = [
            "net.minecraft",
            "net.fabricmc.fabric-loader",
            "org.quiltmc.quilt-loader",
            "net.minecraftforge",
            "net.neoforged",
        ];
        components
            .retain(|component| !replaced.contains(&component["uid"].as_str().unwrap_or_default()));
        components.insert(
            0,
            json!({ "uid": "net.minecraft", "version": self.minecraft,
            "cachedVersion": self.minecraft, "important": true }),
        );
        if let Some(version) = self.loader.version() {
            components
                .push(json!({ "uid": self.loader.uid(), "version": version, "important": true }));
        }
        Ok(serde_json::to_vec_pretty(&pack)?)
    }
}

fn download_request(
    url: &str,
    target: &Path,
    hashes: &BTreeMap<String, String>,
) -> Result<DownloadRequest, ImportError> {
    let parsed = reqwest::Url::parse(url).map_err(|_| problem("Invalid download URL."))?;
    if parsed.scheme() != "https" || !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(problem("Expected an HTTPS modpack download URL."));
    }
    let hash = hashes
        .get("sha1")
        .filter(|hash| hash.len() == 40 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or_else(|| problem("A modpack download has no valid SHA-1 checksum."))?;
    Ok(DownloadRequest::new(url, target).with_sha1(hash))
}

fn apply_overrides(
    root: &Path,
    dir: &Path,
    payload: &Path,
    owned: &mut BTreeMap<PathBuf, OwnedFile>,
) -> Result<(), ImportError> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            apply_overrides(root, &path, payload, owned)?;
        } else {
            let relative = normalize_relative(path.strip_prefix(root).unwrap())?;
            let target = payload.join(&relative);
            fs::create_dir_all(target.parent().unwrap())?;
            fs::copy(root.join(&relative), &target)?;
            owned.insert(relative, OwnedFile::Override);
        }
    }
    Ok(())
}

fn normalize_relative(path: &Path) -> Result<PathBuf, ImportError> {
    let segments: Vec<_> = path
        .components()
        .map(|part| match part {
            Component::Normal(part) => part.to_str().unwrap_or_default(),
            _ => "",
        })
        .collect();
    safe_relative(&segments.join("/"))
}

fn safe_relative(path: &str) -> Result<PathBuf, ImportError> {
    if path.split('/').any(|part| {
        let stem = part
            .split('.')
            .next()
            .unwrap_or_default()
            .to_ascii_uppercase();
        let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || ["COM", "LPT"].iter().any(|prefix| {
                stem.strip_prefix(prefix).is_some_and(|suffix| {
                    matches!(suffix, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
                })
            });
        reserved
            || part.is_empty()
            || part == "."
            || part == ".."
            || part.ends_with(['.', ' '])
            || part.contains(['\\', ':', '<', '>', '"', '|', '?', '*'])
            || part.chars().any(char::is_control)
    }) {
        return Err(problem(format!("Invalid pack path: {path:?}")));
    }
    Ok(path.split('/').collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_urls_require_the_modrinth_modpack_path() {
        assert_eq!(
            import_source("https://modrinth.com/modpack/example/version/1.2"),
            Some(("example".into(), Some("1.2".into())))
        );
        for url in [
            "https://modrinth.com/mod/example",
            "https://modrinth.com.example.org/modpack/example",
            "http://modrinth.com/modpack/example",
            "https://user@modrinth.com/modpack/example",
            "https://modrinth.com/modpack/a%2fb",
        ] {
            assert_eq!(import_source(url), None, "{url}");
        }
    }

    fn write(root: &Path, path: &str, content: &[u8]) {
        let path = root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    fn fixture() -> (tempfile::TempDir, Instance, ManagedPack) {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            root: dir.path().to_path_buf(),
        };
        let instance = InstanceList::create(&paths, "Managed", "default", "1.20.1").unwrap();
        let pack = ManagedPack {
            name: "Example".into(),
            project_id: "pack".into(),
            version_id: "v1".into(),
            version: "1.0".into(),
            files: [
                (
                    "mods/old.jar",
                    OwnedFile::Download("d73ef92426f2b11dfc4aed4d4bfc41c49ee1087c".into()),
                ),
                ("config/old.cfg", OwnedFile::Override),
                ("config/replace.cfg", OwnedFile::Override),
                ("saves/World/level.dat", OwnedFile::Override),
            ]
            .into_iter()
            .map(|(path, kind)| (path.into(), kind))
            .collect(),
        };
        write(&instance.root, RECORD, &serde_json::to_vec(&pack).unwrap());
        (dir, instance, pack)
    }

    fn next(mut pack: ManagedPack) -> Staged {
        pack.version_id = "v2".into();
        pack.version = "2.0".into();
        pack.files.clear();
        let stage = Staged {
            directory: tempfile::tempdir().unwrap(),
            pack,
            minecraft: "1.21.4".into(),
            loader: Loader::Fabric {
                version: "0.16.10".into(),
            },
        };
        stage
    }

    #[test]
    fn replaces_owned_files_and_components_but_keeps_local_files_and_saves() {
        let (_dir, instance, pack) = fixture();
        let game = instance.game_root();
        for path in [
            "mods/old.jar.disabled",
            "config/old.cfg",
            "config/replace.cfg",
            "saves/World/level.dat",
            "mods/personal.jar",
        ] {
            write(&game, path, b"original");
        }
        let snapshot = Snapshot::load(instance.clone()).unwrap();
        let mut stage = next(pack);
        for path in [
            "mods/latest.jar",
            "config/replace.cfg",
            "saves/World/level.dat",
        ] {
            write(&stage.payload(), path, b"next");
            stage.pack.files.insert(path.into(), OwnedFile::Override);
        }
        assert_eq!(snapshot.apply(stage).unwrap().version_id, "v2");
        assert!(!game.join("mods/old.jar.disabled").exists());
        assert!(!game.join("config/old.cfg").exists());
        for path in ["config/replace.cfg", "mods/latest.jar"] {
            assert_eq!(fs::read(game.join(path)).unwrap(), b"next");
        }
        for path in ["mods/personal.jar", "saves/World/level.dat"] {
            assert_eq!(fs::read(game.join(path)).unwrap(), b"original");
        }
        let live = Snapshot::load(instance).unwrap();
        assert!(!live
            .pack
            .files
            .contains_key(Path::new("saves/World/level.dat")));
        assert!(String::from_utf8(live.components)
            .unwrap()
            .contains("1.21.4"));
    }

    #[test]
    fn keeps_an_untracked_file_with_a_disabled_pack_mods_name() {
        let (_dir, instance, pack) = fixture();
        let game = instance.game_root();
        write(&game, "mods/old.jar.disabled", b"personal");
        Snapshot::load(instance).unwrap().apply(next(pack)).unwrap();
        assert_eq!(
            fs::read(game.join("mods/old.jar.disabled")).unwrap(),
            b"personal"
        );
    }

    #[test]
    fn refuses_stale_metadata_and_unowned_collisions_before_modifying_files() {
        let (_dir, instance, pack) = fixture();
        let game = instance.game_root();
        write(&game, "mods/old.jar", b"old");
        write(&game, "mods/personal.jar", b"personal");
        let snapshot = Snapshot::load(instance.clone()).unwrap();
        let mut stage = next(pack.clone());
        stage
            .pack
            .files
            .insert("mods/personal.jar".into(), OwnedFile::Override);
        write(&stage.payload(), "mods/personal.jar", b"new");
        assert!(snapshot
            .apply(stage)
            .unwrap_err()
            .to_string()
            .contains("conflicts"));
        assert_eq!(
            fs::read(game.join("mods/personal.jar")).unwrap(),
            b"personal"
        );
        let mut changed = pack.clone();
        changed.version_id = "changed".into();
        write(
            &instance.root,
            RECORD,
            &serde_json::to_vec(&changed).unwrap(),
        );
        assert!(snapshot
            .apply(next(pack))
            .unwrap_err()
            .to_string()
            .contains("changed"));
        assert_eq!(fs::read(game.join("mods/old.jar")).unwrap(), b"old");
    }

    #[test]
    fn restores_original_file_when_apply_fails() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("a");
        write(dir.path(), "a", b"original");
        let missing = dir.path().join("missing");
        assert!(apply_changes(dir.path(), [(target.clone(), Some(missing))].into()).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"original");
    }

    #[test]
    fn rejects_traversal_and_nonportable_paths() {
        for path in [
            "../outside",
            "config/../../outside",
            "config//file",
            "/absolute",
            "mods\\linked",
            "C:/absolute",
            "mods/CON.jar",
        ] {
            assert!(safe_relative(path).is_err(), "{path}");
        }
        assert_eq!(
            safe_relative("config/example.json").unwrap(),
            Path::new("config").join("example.json")
        );
    }
}

fn alternate(path: &Path) -> Option<PathBuf> {
    if !path.starts_with("mods") {
        return None;
    }
    let filename = path.file_name()?.to_str()?;
    Some(
        path.with_file_name(
            filename
                .strip_suffix(".disabled")
                .map(str::to_owned)
                .unwrap_or_else(|| format!("{filename}.disabled")),
        ),
    )
}

fn require_file(path: &Path) -> io::Result<()> {
    if fs::symlink_metadata(path)?.is_file() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "Not a regular file: {}",
            path.display()
        )))
    }
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

fn existing_file(root: &Path, path: &Path) -> io::Result<bool> {
    let mut current = path.parent().unwrap();
    while current != root {
        match require_directory(current) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        current = current.parent().unwrap();
    }
    match require_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn apply_changes(
    root: &Path,
    changes: BTreeMap<PathBuf, Option<PathBuf>>,
) -> Result<(), ImportError> {
    let backup = tempfile::Builder::new()
        .prefix(".managed-pack-backup-")
        .tempdir_in(root)?;
    let mut applied = Vec::new();
    let mut dirs = Vec::new();
    let result = (|| -> io::Result<()> {
        for (number, (target, source)) in changes.into_iter().enumerate() {
            let previous = backup.path().join(number.to_string());
            let saved = if existing_file(root, &target)? {
                fs::rename(&target, &previous)?;
                Some(previous)
            } else {
                None
            };
            applied.push((target, saved, false));
            let (target, _, installed) = applied.last_mut().unwrap();
            if let Some(source) = source {
                create_parents(target.parent().unwrap(), &mut dirs)?;
                let mut file = tempfile::NamedTempFile::new_in(target.parent().unwrap())?;
                io::copy(&mut fs::File::open(source)?, file.as_file_mut())?;
                file.persist_noclobber(target)
                    .map_err(|error| error.error)?;
                *installed = true;
            }
        }
        Ok(())
    })();
    if let Err(error) = result {
        let mut failed = false;
        for (target, saved, installed) in applied.into_iter().rev() {
            if installed && fs::remove_file(&target).is_err() {
                failed = true;
            }
            if let Some(saved) = saved {
                if target.exists() || fs::rename(saved, &target).is_err() {
                    failed = true;
                }
            }
        }
        for dir in dirs.into_iter().rev() {
            let _ = fs::remove_dir(dir);
        }
        if failed {
            return Err(problem(format!("Update failed: {error}. Some original files could not be restored; backups are in {}.", backup.keep().display())));
        }
        return Err(error.into());
    }
    Ok(())
}

fn create_parents(path: &Path, created: &mut Vec<PathBuf>) -> io::Result<()> {
    match require_directory(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            create_parents(path.parent().unwrap(), created)?;
            fs::create_dir(path)?;
            created.push(path.to_owned());
            Ok(())
        }
        Err(error) => Err(error),
    }
}
