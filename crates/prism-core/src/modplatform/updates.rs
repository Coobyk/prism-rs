//! Compatible updates for installed Java mods, with Prism's packwiz index format.

use super::flame::{curseforge_loader, FlameApi, FlameFile, API_KEY_ENV};
use super::modrinth::{modrinth_loader, ModFile, ModVersion, ModrinthApi};
use super::{PlatformError, ResourceType};
use crate::loaders::Loader;
use crate::net::{DownloadRequest, Downloader};
use futures::{stream, StreamExt};
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

#[derive(Clone, Deserialize, Serialize)]
struct Index {
    name: String,
    filename: String,
    side: Option<String>,
    download: IndexDownload,
    #[serde(default)]
    update: Providers,
    #[serde(
        rename = "x-prismlauncher-lock-update",
        default,
        skip_serializing_if = "std::ops::Not::not"
    )]
    locked: bool,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
struct IndexDownload {
    hash_format: String,
    hash: String,
    mode: Option<String>,
    url: Option<String>,
}

#[derive(Clone, Default, Deserialize, Serialize)]
struct Providers {
    modrinth: Option<ModrinthId>,
    curseforge: Option<CurseForgeId>,
}

impl Providers {
    fn keep_alternates(&mut self, previous: &Self) {
        if self.modrinth.is_none() {
            self.modrinth = previous.modrinth.clone();
        }
        if self.curseforge.is_none() {
            self.curseforge = previous.curseforge.clone();
        }
    }
}

#[derive(Clone, Deserialize, Serialize)]
struct ModrinthId {
    #[serde(rename = "mod-id")]
    project: String,
    version: String,
}

#[derive(Clone, Deserialize, Serialize)]
struct CurseForgeId {
    #[serde(rename = "project-id")]
    project: i64,
    #[serde(rename = "file-id")]
    file: i64,
}

#[derive(Clone)]
struct IndexedMod {
    path: PathBuf,
    index: Index,
    original: String,
}

struct Installed {
    filename: String,
    sha1: String,
    index: Option<IndexedMod>,
}

#[derive(Clone)]
pub struct ModUpdate {
    pub title: String,
    pub current_version: String,
    pub new_version: String,
    pub provider: &'static str,
    old_name: String,
    old_sha1: String,
    new_index: Index,
    index_path: PathBuf,
    old_index: Option<String>,
    url: String,
}

#[derive(Default)]
pub struct UpdateReport {
    pub updates: Vec<ModUpdate>,
    pub unmatched: usize,
    pub locked: usize,
    pub warnings: Vec<String>,
}

fn problem(message: impl Into<String>) -> PlatformError {
    PlatformError::Request(message.into())
}

pub(super) fn is_jar(name: &str) -> bool {
    Path::new(name).file_name().and_then(|name| name.to_str()) == Some(name)
        && !name.contains(['/', '\\', ':'])
        && !name.chars().any(char::is_control)
        && name.to_ascii_lowercase().ends_with(".jar")
}

fn sha1(path: &Path) -> io::Result<String> {
    if !path.symlink_metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "not a regular file",
        ));
    }
    let mut file = fs::File::open(path)?;
    let mut hash = Sha1::new();
    let mut buffer = [0; 32 * 1024];
    loop {
        let len = file.read(&mut buffer)?;
        if len == 0 {
            break;
        }
        hash.update(&buffer[..len]);
    }
    Ok(hex::encode(hash.finalize()))
}

fn index_path(mods: &Path, index: &Index) -> PathBuf {
    let mut hash = Sha1::new();
    hash.update(index.filename.as_bytes());
    mods.join(".index")
        .join(format!("{}.pw.toml", hex::encode(hash.finalize())))
}

fn read_index(mods: &Path) -> BTreeMap<String, Vec<IndexedMod>> {
    let mut result: BTreeMap<String, Vec<IndexedMod>> = BTreeMap::new();
    let Ok(entries) = fs::read_dir(mods.join(".index")) else {
        return result;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.to_string_lossy().ends_with(".pw.toml")
            || !entry.file_type().is_ok_and(|t| t.is_file())
        {
            continue;
        }
        if let Ok(original) = fs::read_to_string(&path) {
            if let Ok(index) = toml::from_str::<Index>(&original) {
                result
                    .entry(index.filename.clone())
                    .or_default()
                    .push(IndexedMod {
                        path,
                        index,
                        original,
                    });
            }
        }
    }
    result
}

pub(crate) struct ModListMetadata {
    pub name: String,
    pub homepage: Option<String>,
    pub version: Option<String>,
}

pub(crate) fn mod_list_metadata(mods: &Path) -> BTreeMap<String, ModListMetadata> {
    read_index(mods).into_iter().filter_map(|(filename, entries)| {
        let [entry] = entries.as_slice() else { return None };
        let index = &entry.index;
        let (homepage, version) = if let Some(provider) = &index.update.modrinth {
            let mut url = reqwest::Url::parse("https://modrinth.com/mod/").unwrap();
            url.path_segments_mut().unwrap().pop_if_empty().push(&provider.project);
            (Some(url.to_string()), Some(provider.version.clone()))
        } else if let Some(provider) = &index.update.curseforge {
            (
                Some(format!("https://www.curseforge.com/projects/{}", provider.project)),
                Some(provider.file.to_string()),
            )
        } else {
            (None, None)
        };
        Some((filename, ModListMetadata { name: index.name.clone(), homepage, version }))
    }).collect()
}

fn save_index(path: &Path, index: &Index, previous: Option<&str>) -> Result<(), PlatformError> {
    let dir = path
        .parent()
        .ok_or_else(|| problem("invalid mod index path"))?;
    fs::create_dir_all(dir)?;
    let mut content = match previous {
        Some(text) => toml::from_str::<toml::Table>(text).map_err(|e| problem(e.to_string()))?,
        None => toml::Table::new(),
    };
    let replacement = toml::Table::try_from(index).map_err(|e| problem(e.to_string()))?;
    content.extend(replacement);
    for attribute in [
        "version-number",
        "release-type",
        "dependencies",
        "loaders",
        "mc-versions",
    ] {
        content.remove(&format!("x-prismlauncher-{attribute}"));
    }
    let text = toml::to_string(&content).map_err(|e| problem(e.to_string()))?;
    let mut temp = tempfile::NamedTempFile::new_in(dir)?;
    temp.write_all(text.as_bytes())?;
    if previous.is_some() {
        temp.persist(path).map_err(|e| e.error)?;
    } else {
        temp.persist_noclobber(path).map_err(|e| e.error)?;
    }
    Ok(())
}

fn modrinth_index(title: &str, version: &ModVersion, file: &ModFile) -> Option<Index> {
    Some(Index {
        name: title.to_owned(),
        filename: file.filename.clone(),
        side: Some("both".into()),
        download: IndexDownload {
            hash_format: "sha1".into(),
            hash: file.hashes.get("sha1")?.clone(),
            mode: Some("url".into()),
            url: Some(file.url.clone()),
        },
        update: Providers {
            modrinth: Some(ModrinthId {
                project: version.project_id.clone(),
                version: version.id.clone(),
            }),
            curseforge: None,
        },
        locked: false,
    })
}

fn curseforge_index(project: i64, file: &FlameFile) -> Option<Index> {
    Some(Index {
        name: file.displayName.clone(),
        filename: file.fileName.clone(),
        side: Some("both".into()),
        download: IndexDownload {
            hash_format: "sha1".into(),
            hash: file.sha1()?.to_owned(),
            mode: Some("metadata:curseforge".into()),
            url: None,
        },
        update: Providers {
            modrinth: None,
            curseforge: Some(CurseForgeId {
                project,
                file: file.id,
            }),
        },
        locked: false,
    })
}

fn record(game_root: &Path, index: Option<Index>) -> Result<(), PlatformError> {
    let Some(mut index) = index.filter(|index| is_jar(&index.filename)) else {
        return Ok(());
    };
    let mods = game_root.join("mods");
    let indexes = read_index(&mods);
    match indexes.get(&index.filename).map(Vec::as_slice) {
        Some([previous]) => {
            index.side = previous.index.side.clone();
            index.locked = previous.index.locked;
            let new = &index.update;
            let old = &previous.index.update;
            let same_project = new
                .modrinth
                .as_ref()
                .zip(old.modrinth.as_ref())
                .is_some_and(|(new, old)| new.project == old.project)
                || new
                    .curseforge
                    .as_ref()
                    .zip(old.curseforge.as_ref())
                    .is_some_and(|(new, old)| new.project == old.project);
            if same_project {
                index.update.keep_alternates(old);
            }
            save_index(&previous.path, &index, Some(&previous.original))
        }
        Some(_) => Err(problem(format!(
            "{}: conflicting .index entries",
            index.filename
        ))),
        None => save_index(&index_path(&mods, &index), &index, None),
    }
}

fn warn_on_record_failure(result: Result<(), PlatformError>) {
    if let Err(error) = result {
        tracing::warn!("could not save mod index: {error}");
    }
}

pub(super) fn record_modrinth(game_root: &Path, title: &str, version: &ModVersion, file: &ModFile) {
    warn_on_record_failure(record(game_root, modrinth_index(title, version, file)));
}

pub(crate) fn record_curseforge(game_root: &Path, project: i64, file: &FlameFile) {
    warn_on_record_failure(write_curseforge_index(game_root, project, file));
}

pub(super) fn write_curseforge_index(
    game_root: &Path,
    project: i64,
    file: &FlameFile,
) -> Result<(), PlatformError> {
    record(game_root, curseforge_index(project, file))
}

fn timestamp(date: &str) -> Option<chrono::DateTime<chrono::FixedOffset>> {
    chrono::DateTime::parse_from_rfc3339(date).ok()
}

fn modrinth_channel(channel: &str) -> u8 {
    match channel {
        "beta" => 2,
        "alpha" => 3,
        _ => 1,
    }
}

fn newer(candidate: &str, current: &str) -> bool {
    timestamp(candidate)
        .zip(timestamp(current))
        .is_some_and(|(candidate, current)| candidate > current)
}

async fn check_modrinth(
    api: &ModrinthApi,
    current: &ModVersion,
    mc: &str,
    loader: &str,
) -> Result<Option<(String, String, Index, String)>, PlatformError> {
    let versions = api.versions(&current.project_id, &[loader], &[mc]).await?;
    Ok(versions
        .into_iter()
        .filter(|version| {
            version.id != current.id
                && version.game_versions.iter().any(|game| game == mc)
                && version.loaders.iter().any(|kind| kind == loader)
                && modrinth_channel(&version.version_type)
                    <= modrinth_channel(&current.version_type)
                && newer(&version.date_published, &current.date_published)
        })
        .filter_map(|version| {
            let file = api
                .primary_file(&version)
                .filter(|file| is_jar(&file.filename))?;
            let index = modrinth_index(&version.name, &version, file)?;
            Some((
                timestamp(&version.date_published)?,
                version.version_number.clone(),
                index,
                file.url.clone(),
            ))
        })
        .max_by_key(|(date, _, _, _)| *date)
        .map(|(_, version, index, url)| (current.version_number.clone(), version, index, url)))
}

fn matches_modrinth_install(version: &ModVersion, project: &str, hash: &str) -> bool {
    version.project_id == project
        && version.files.iter().any(|file| {
            file.hashes
                .get("sha1")
                .is_some_and(|sha1| hash.eq_ignore_ascii_case(sha1))
        })
}

async fn check_curseforge(
    api: &FlameApi,
    id: &CurseForgeId,
    mc: &str,
    loader: u8,
    hash: &str,
) -> Result<Option<(String, String, Index, String)>, PlatformError> {
    let current = api.file(id.project, id.file).await?;
    if !current
        .sha1()
        .is_some_and(|known| hash.eq_ignore_ascii_case(known))
    {
        return Err(problem(
            "installed file does not match its CurseForge index",
        ));
    }
    let files = api.compatible_files(id.project, mc, loader).await?;
    Ok(files
        .into_iter()
        .filter(|file| {
            file.id != current.id
                && (1..=current.release_type).contains(&file.release_type)
                && file.gameVersions.iter().any(|game| game == mc)
                && newer(&file.file_date, &current.file_date)
        })
        .filter_map(|file| {
            let url = file
                .downloadUrl
                .as_ref()
                .filter(|url| !url.is_empty() && is_jar(&file.fileName))?
                .clone();
            Some((
                timestamp(&file.file_date)?,
                file.id,
                file.displayName.clone(),
                curseforge_index(id.project, &file)?,
                url,
            ))
        })
        .max_by_key(|(date, id, _, _, _)| (*date, *id))
        .map(|(_, _, version, index, url)| (current.displayName, version, index, url)))
}

pub async fn check(
    game_root: &Path,
    mc: &str,
    loader: &Loader,
    downloader: &Downloader,
) -> Result<UpdateReport, PlatformError> {
    let mr_loader = modrinth_loader(ResourceType::Mod, loader)
        .ok_or_else(|| problem("Install a mod loader before checking for updates."))?;
    let cf_loader = curseforge_loader(loader).expect("supported mod loader");
    let mods = game_root.join("mods");
    let mut report = UpdateReport::default();
    let entries = match fs::read_dir(&mods) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(report),
        Err(e) => return Err(e.into()),
    };
    let indexes = read_index(&mods);
    let mut installed = Vec::new();
    for entry in entries {
        let entry = entry?;
        let filename = entry.file_name().to_string_lossy().into_owned();
        let active_name = filename.strip_suffix(".disabled").unwrap_or(&filename);
        if !is_jar(active_name) || !entry.file_type()?.is_file() {
            continue;
        }
        let index = match indexes.get(active_name).map(Vec::as_slice) {
            Some([index]) if index.index.locked => {
                report.locked += 1;
                continue;
            }
            Some([index]) => Some(index.clone()),
            Some(_) => {
                report
                    .warnings
                    .push(format!("{filename}: conflicting .index entries"));
                continue;
            }
            None => None,
        };
        match sha1(&entry.path()) {
            Ok(sha1) => installed.push(Installed {
                filename,
                sha1,
                index,
            }),
            Err(error) => report.warnings.push(format!("{filename}: {error}")),
        }
    }
    let mr = ModrinthApi::with_client(downloader.client());
    let cf = FlameApi::new();
    let use_curseforge = |index: &Index| {
        index.update.curseforge.is_some()
            && (index.update.modrinth.is_none()
                || (cf.has_api_key()
                    && index.download.mode.as_deref() == Some("metadata:curseforge")))
    };
    let hashes = installed
        .iter()
        .filter(|file| {
            file.index.as_ref().is_none_or(|meta| {
                !use_curseforge(&meta.index)
                    && (meta.index.update.modrinth.is_none()
                        || meta.index.update.curseforge.is_some())
            })
        })
        .map(|file| file.sha1.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let mut current_versions = BTreeMap::new();
    for batch in hashes.chunks(100) {
        match mr.version_files(batch).await {
            Ok(versions) => current_versions.extend(versions),
            Err(error) => report
                .warnings
                .push(format!("Modrinth file lookup failed: {error}")),
        }
    }
    if !cf.has_api_key()
        && installed.iter().any(|file| {
            file.index
                .as_ref()
                .is_some_and(|meta| use_curseforge(&meta.index))
        })
    {
        report
            .warnings
            .push(format!("Set {API_KEY_ENV} to check CurseForge mods."));
    }
    let results = stream::iter(installed.into_iter().map(|file| {
        let (mr, cf, current_versions, use_curseforge) =
            (&mr, &cf, &current_versions, &use_curseforge);
        async move {
            let providers = file.index.as_ref().map(|meta| &meta.index.update);
            let (provider, result) = if let Some(id) = file
                .index
                .as_ref()
                .filter(|meta| use_curseforge(&meta.index))
                .and_then(|meta| meta.index.update.curseforge.as_ref())
            {
                if !cf.has_api_key() {
                    return (file, None);
                }
                (
                    "CurseForge",
                    check_curseforge(cf, id, mc, cf_loader, &file.sha1).await,
                )
            } else if let Some(id) = providers.and_then(|p| p.modrinth.as_ref()) {
                let result = async {
                    // An update from the other provider may leave this version ID stale.
                    if let Some(current) = current_versions.get(&file.sha1).filter(|current| {
                        matches_modrinth_install(current, &id.project, &file.sha1)
                    }) {
                        return check_modrinth(mr, current, mc, mr_loader).await;
                    }
                    let current = mr.version(&id.version).await?;
                    if !matches_modrinth_install(&current, &id.project, &file.sha1) {
                        return Err(problem("installed file does not match its Modrinth index"));
                    }
                    check_modrinth(mr, &current, mc, mr_loader).await
                }
                .await;
                ("Modrinth", result)
            } else if let Some(current) = current_versions.get(&file.sha1) {
                ("Modrinth", check_modrinth(mr, current, mc, mr_loader).await)
            } else {
                return (file, None);
            };
            (file, Some((provider, result)))
        }
    }))
    .buffer_unordered(6)
    .collect::<Vec<_>>()
    .await;
    for (file, result) in results {
        match result {
            None => report.unmatched += 1,
            Some((_, Err(error))) => report.warnings.push(format!("{}: {error}", file.filename)),
            Some((_, Ok(None))) => {}
            Some((provider, Ok(Some((current_version, new_version, mut index, url))))) => {
                if !is_jar(&index.filename) || index.download.hash.eq_ignore_ascii_case(&file.sha1)
                {
                    continue;
                }
                let (title, path, old_index) = if let Some(previous) = &file.index {
                    index.side = previous.index.side.clone();
                    index.update.keep_alternates(&previous.index.update);
                    (
                        previous.index.name.clone(),
                        previous.path.clone(),
                        Some(previous.original.clone()),
                    )
                } else {
                    (index.name.clone(), index_path(&mods, &index), None)
                };
                index.name = title.clone();
                report.updates.push(ModUpdate {
                    title,
                    current_version,
                    new_version,
                    provider,
                    old_name: file.filename.clone(),
                    old_sha1: file.sha1.clone(),
                    new_index: index,
                    index_path: path,
                    old_index,
                    url,
                });
            }
        }
    }
    report
        .updates
        .sort_by(|a, b| a.title.cmp(&b.title).then(a.old_name.cmp(&b.old_name)));
    report.warnings.sort();
    Ok(report)
}

impl ModUpdate {
    pub fn installed_filename(&self) -> &str {
        &self.old_name
    }

    pub async fn apply(&self, downloader: &Downloader) -> Result<(), PlatformError> {
        let mods = self
            .index_path
            .parent()
            .and_then(Path::parent)
            .ok_or_else(|| problem("invalid mod path"))?;
        let old = mods.join(&self.old_name);
        let new_name = &self.new_index.filename;
        let disabled = format!("{new_name}.disabled");
        let (new, other) = if self.old_name.ends_with(".disabled") {
            (mods.join(disabled), mods.join(new_name))
        } else {
            (mods.join(new_name), mods.join(disabled))
        };
        let ensure_fresh = || -> Result<(), PlatformError> {
            if sha1(&old)? != self.old_sha1 {
                return Err(problem("the mod changed since the check; check again"));
            }
            match &self.old_index {
                Some(expected) if fs::read_to_string(&self.index_path)? != *expected => {
                    return Err(problem(
                        "the mod metadata changed since the check; check again",
                    ));
                }
                None if self.index_path.symlink_metadata().is_ok() => {
                    return Err(problem(
                        "mod metadata already exists at the update destination",
                    ));
                }
                _ => {}
            }
            if other.symlink_metadata().is_ok() || (old != new && new.symlink_metadata().is_ok()) {
                return Err(problem("the update destination already exists"));
            }
            Ok(())
        };
        ensure_fresh()?;
        let stage = tempfile::tempdir_in(mods)?;
        let req = DownloadRequest::new(&self.url, stage.path().join("mod.jar"))
            .with_sha1(&self.new_index.download.hash);
        downloader.download(&req).await?;
        ensure_fresh()?;
        let backup = stage.path().join("original.jar");
        let replacement = tempfile::TempPath::try_from_path(req.dest)?;
        fs::rename(&old, &backup)?;

        let roll_back = |error: PlatformError, installed: bool| {
            let restore = (|| -> io::Result<()> {
                if installed && old != new {
                    fs::remove_file(&new)?;
                }
                if (!installed || old != new) && old.symlink_metadata().is_ok() {
                    return Err(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        "original path is occupied",
                    ));
                }
                fs::rename(&backup, &old)
            })();
            return match restore {
                Ok(()) => Err(error),
                Err(restore_error) => {
                    let backup = stage.keep().join("original.jar");
                    Err(problem(format!(
                        "{error}; could not roll back ({restore_error}); original backed up at {}",
                        backup.display()
                    )))
                }
            };
        };
        if let Err(error) = replacement.persist_noclobber(&new) {
            return roll_back(error.error.into(), false);
        }
        if let Err(error) = save_index(&self.index_path, &self.new_index, self.old_index.as_deref())
        {
            return roll_back(
                problem(format!("could not save mod metadata: {error}")),
                true,
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CURSEFORGE: &str = r#"
name = "Example Mod"
filename = "example.jar"
side = "client"
x-prismlauncher-lock-update = true

[download]
hash-format = "murmur2"
hash = "1781245820"
mode = "metadata:curseforge"

[update.curseforge]
project-id = 327154
file-id = 3509043
"#;

    fn serve(
        listener: std::net::TcpListener,
        responses: Vec<(&'static [&'static str], Vec<u8>)>,
    ) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            use std::io::BufRead;
            for (parts, body) in responses {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = std::io::BufReader::new(&mut stream);
                let mut line = String::new();
                request.read_line(&mut line).unwrap();
                assert!(parts.iter().all(|part| line.contains(part)), "{line}");
                loop {
                    line.clear();
                    if request.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                        break;
                    }
                }
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .unwrap();
                stream.write_all(&body).unwrap();
            }
        })
    }

    #[test]
    fn reads_prism_packwiz_metadata_and_preserves_unrelated_properties() {
        let temp = tempfile::tempdir().unwrap();
        let mods = temp.path();
        let path = mods.join(".index").join("prism.pw.toml");
        let original = format!("custom-setting = 42\n{CURSEFORGE}");
        let mut index: Index = toml::from_str(&original).unwrap();
        assert!(index.locked);
        assert_eq!(index.update.curseforge.as_ref().unwrap().project, 327154);
        assert_eq!(index.update.curseforge.as_ref().unwrap().file, 3509043);
        save_index(&path, &index, None).unwrap();
        assert!(read_index(mods)["example.jar"][0].index.locked);
        index.filename = "updated.jar".into();
        save_index(&path, &index, Some(&original)).unwrap();
        let result = fs::read_to_string(path).unwrap();
        assert!(result.contains("custom-setting = 42"));
        assert_eq!(
            read_index(mods)["updated.jar"][0].index.side.as_deref(),
            Some("client")
        );
    }

    #[tokio::test]
    async fn locked_metadata_does_not_need_an_update_provider() {
        let temp = tempfile::tempdir().unwrap();
        let mods = temp.path().join("mods");
        let dir = mods.join(".index");
        fs::create_dir_all(&dir).unwrap();
        fs::write(mods.join("example.jar"), b"locked mod").unwrap();
        fs::write(
            dir.join("example.pw.toml"),
            CURSEFORGE.split("[update.curseforge]").next().unwrap(),
        )
        .unwrap();

        let report = check(
            temp.path(),
            "1.20.1",
            &Loader::Fabric {
                version: "0.16.14".into(),
            },
            &Downloader::new(),
        )
        .await
        .unwrap();
        assert_eq!(report.locked, 1);
        assert!(report.updates.is_empty());
    }

    #[test]
    fn recording_a_same_name_install_refreshes_the_file_id_and_preserves_the_lock() {
        let temp = tempfile::tempdir().unwrap();
        let mods = temp.path().join("mods");
        let mut index: Index = toml::from_str(CURSEFORGE).unwrap();
        record(temp.path(), Some(index.clone())).unwrap();
        index.update.curseforge.as_mut().unwrap().file = 5;
        index.download.hash = "replacement".into();
        index.locked = false;
        index.side = Some("both".into());
        record(temp.path(), Some(index)).unwrap();
        let records = read_index(&mods);
        let [recorded] = records["example.jar"].as_slice() else {
            panic!("duplicate sidecars")
        };
        assert_eq!(recorded.index.update.curseforge.as_ref().unwrap().file, 5);
        assert_eq!(recorded.index.download.hash, "replacement");
        assert!(recorded.index.locked);
        assert_eq!(recorded.index.side.as_deref(), Some("client"));
    }

    #[test]
    fn refreshing_a_sidecar_keeps_the_alternate_provider() {
        let temp = tempfile::tempdir().unwrap();
        let mods = temp.path().join("mods");
        let mut index: Index = toml::from_str(CURSEFORGE).unwrap();
        index.update.modrinth = Some(ModrinthId {
            project: "modrinth-project".into(),
            version: "old".into(),
        });
        record(temp.path(), Some(index.clone())).unwrap();
        index.update.modrinth = None;
        index.update.curseforge.as_mut().unwrap().file = 5;
        record(temp.path(), Some(index)).unwrap();
        let records = read_index(&mods);
        let recorded = &records["example.jar"][0].index.update;
        assert_eq!(
            recorded.modrinth.as_ref().unwrap().project,
            "modrinth-project"
        );
        assert_eq!(recorded.curseforge.as_ref().unwrap().file, 5);
    }

    #[tokio::test]
    async fn applies_a_verified_update_and_preserves_disabled_state() {
        let temp = tempfile::tempdir().unwrap();
        let mods = temp.path();
        let old = mods.join("example.jar.disabled");
        let new = mods.join("new.jar.disabled");
        let bytes = b"replacement file";
        fs::write(&old, b"original file").unwrap();
        let mut index: Index = toml::from_str(CURSEFORGE).unwrap();
        index.filename = "new.jar".into();
        index.download.hash_format = "sha1".into();
        index.download.hash = hex::encode(Sha1::digest(bytes));
        index.locked = false;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let update = ModUpdate {
            title: index.name.clone(),
            current_version: "Old".into(),
            new_version: "New".into(),
            provider: "CurseForge",
            old_name: "example.jar.disabled".into(),
            old_sha1: sha1(&old).unwrap(),
            index_path: index_path(mods, &index),
            new_index: index,
            old_index: None,
            url: format!("http://{}/file", listener.local_addr().unwrap()),
        };
        let downloader = Downloader::new();
        fs::write(&new, b"user file").unwrap();
        assert!(update.apply(&downloader).await.is_err());
        assert_eq!(fs::read(&new).unwrap(), b"user file");
        fs::remove_file(&new).unwrap();
        let done = serve(listener, vec![(&["GET /file "], bytes.to_vec())]);
        update.apply(&downloader).await.unwrap();
        done.join().unwrap();
        assert!(!old.exists());
        assert_eq!(fs::read(new).unwrap(), bytes);
        assert!(read_index(mods).contains_key("new.jar"));
    }

    #[tokio::test]
    async fn restores_the_old_jar_if_the_metadata_cannot_be_saved() {
        for new_name in ["example.jar", "new.jar"] {
            let temp = tempfile::tempdir().unwrap();
            let mods = temp.path();
            let old = mods.join("example.jar");
            fs::write(&old, b"original file").unwrap();
            fs::write(mods.join(".index"), b"not a directory").unwrap();
            let bytes = b"replacement file";
            let mut index: Index = toml::from_str(CURSEFORGE).unwrap();
            index.filename = new_name.into();
            index.download.hash = hex::encode(Sha1::digest(bytes));
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let update = ModUpdate {
                title: index.name.clone(),
                current_version: "Old".into(),
                new_version: "New".into(),
                provider: "CurseForge",
                old_name: "example.jar".into(),
                old_sha1: sha1(&old).unwrap(),
                index_path: index_path(mods, &index),
                new_index: index,
                old_index: None,
                url: format!("http://{}/file", listener.local_addr().unwrap()),
            };
            let done = serve(listener, vec![(&["GET /file "], bytes.to_vec())]);
            let error = update.apply(&Downloader::new()).await.unwrap_err();
            done.join().unwrap();
            assert!(error.to_string().contains("could not save mod metadata"));
            assert_eq!(fs::read(old).unwrap(), b"original file");
            assert!(!mods.join("new.jar").exists());
        }
    }

    #[tokio::test]
    async fn curseforge_finds_a_compatible_release_on_a_later_page() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let api =
            FlameApi::with_test_endpoint(format!("http://{}", listener.local_addr().unwrap()));
        let version = |id, name, date, channel, game| {
            serde_json::json!({
                "id": id, "displayName": name, "fileName": format!("example-{id}.jar"),
                "fileDate": date, "releaseType": channel, "gameVersions": [game],
                "downloadUrl": "https://example.invalid/example.jar",
                "hashes": [{"algo": 1, "value": "c4293c1483f3b39c1c6c4c69143e8bb2c6e53daf"}]
            })
        };
        let current = version(1, "Installed", "2026-01-01T00:00:00Z", 1, "1.20.1");
        let beta = version(4, "New Beta", "2026-01-04T00:00:00Z", 2, "1.20.1");
        let wrong_game = version(3, "Wrong Game", "2026-01-03T00:00:00Z", 1, "1.21");
        let newer = version(2, "New Stable", "2026-01-02T00:00:00Z", 1, "1.20.1");
        let service = serve(
            listener,
            vec![
                (
                    &["GET /mods/42/files/1 "],
                    serde_json::to_vec(&serde_json::json!({"data": current})).unwrap(),
                ),
                (
                    &[
                        "GET /mods/42/files?",
                        "gameVersion=1.20.1",
                        "modLoaderType=4",
                        "index=0",
                    ],
                    serde_json::to_vec(&serde_json::json!({
                        "data": [beta, wrong_game],
                        "pagination": {"index": 0, "totalCount": 3}
                    }))
                    .unwrap(),
                ),
                (
                    &["GET /mods/42/files?", "index=2"],
                    serde_json::to_vec(&serde_json::json!({
                        "data": [newer],
                        "pagination": {"index": 2, "totalCount": 3}
                    }))
                    .unwrap(),
                ),
            ],
        );
        let (old, new, index, _) = check_curseforge(
            &api,
            &CurseForgeId {
                project: 42,
                file: 1,
            },
            "1.20.1",
            4,
            "c4293c1483f3b39c1c6c4c69143e8bb2c6e53daf",
        )
        .await
        .unwrap()
        .unwrap();
        service.join().unwrap();
        assert_eq!(old, "Installed");
        assert_eq!(new, "New Stable");
        assert_eq!(index.update.curseforge.unwrap().file, 2);
    }

    #[test]
    fn release_dates_use_instants_not_string_order() {
        assert!(newer("2026-01-01T00:00:00.100Z", "2026-01-01T00:00:00Z"));
        assert!(!newer("2026-01-01T01:00:00+01:00", "2026-01-01T00:00:00Z"));
        assert!(!newer("malformed", "2026-01-01T00:00:00Z"));
        assert_eq!(
            curseforge_loader(&Loader::Fabric {
                version: "0.16.14".into()
            }),
            Some(4)
        );
    }
}
