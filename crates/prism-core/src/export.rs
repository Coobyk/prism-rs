//! Prism instance archives and Modrinth modpacks.

use crate::modplatform::modrinth::{ModVersion, ModrinthApi, MrpackFile, MrpackIndex};
use crate::Instance;
use sha1::{Digest, Sha1};
use sha2::Sha512;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use zip::write::SimpleFileOptions;

#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Zip(#[from] zip::result::ZipError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Platform(#[from] crate::modplatform::PlatformError),
    #[error("Horizon instances cannot be exported from Prism. Use the Horizon launcher to export the game.")]
    HorizonInstance,
    #[error("{0}")]
    Invalid(String),
}

pub struct MrpackMetadata {
    pub name: String,
    pub version: String,
    pub summary: String,
}

fn collect(root: &Path, game: &Path, out: &mut Vec<PathBuf>) -> Result<(), ExportError> {
    collect_directory(root, game, out, &mut BTreeSet::new())?;
    out.sort();
    Ok(())
}

fn collect_directory(
    root: &Path,
    game: &Path,
    out: &mut Vec<PathBuf>,
    ancestors: &mut BTreeSet<PathBuf>,
) -> Result<(), ExportError> {
    let canonical = root.canonicalize()?;
    if !ancestors.insert(canonical.clone()) {
        return Ok(());
    }
    let mut entries = std::fs::read_dir(root)?.peekable();
    if entries.peek().is_none() {
        out.push(root.to_path_buf());
    }
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if matches!(name.as_ref(), ".DS_Store" | "thumbs.db" | "Thumbs.db")
            || (root == game
                && matches!(
                    name.as_ref(),
                    "logs" | "crash-reports" | ".cache" | ".fabric" | ".quilt"
                ))
        {
            continue;
        }
        let kind = match std::fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if std::fs::symlink_metadata(&path)?.is_symlink() {
                    continue;
                }
                return Err(error.into());
            }
            Err(error) => return Err(error.into()),
        };
        if kind.is_dir() {
            collect_directory(&path, game, out, ancestors)?;
        } else if kind.is_file() {
            out.push(path);
        }
    }
    ancestors.remove(&canonical);
    Ok(())
}

fn archive_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

struct IncompleteOutput {
    path: PathBuf,
    keep: bool,
}

impl Drop for IncompleteOutput {
    fn drop(&mut self) {
        if !self.keep {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

fn writer(
    instance: &Instance,
    output: &Path,
) -> Result<(IncompleteOutput, zip::ZipWriter<File>), ExportError> {
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    if parent
        .canonicalize()?
        .starts_with(instance.root.canonicalize()?)
    {
        return Err(ExportError::Invalid(
            "Choose an output outside the instance directory".into(),
        ));
    }
    let file = File::options().write(true).create_new(true).open(output)?;
    Ok((
        IncompleteOutput {
            path: output.into(),
            keep: false,
        },
        zip::ZipWriter::new(file),
    ))
}

pub fn export_zip(instance: &Instance, output: &Path) -> Result<(), ExportError> {
    if instance.is_horizon() {
        return Err(ExportError::HorizonInstance);
    }
    let mut files = Vec::new();
    collect(&instance.root, &instance.game_root(), &mut files)?;
    files.retain(|path| {
        path.parent() != Some(&instance.root)
            || !path.file_name().is_some_and(Instance::is_launch_state_file)
    });
    let (mut output, mut zip) = writer(instance, output)?;
    for path in files {
        if path.is_dir() {
            zip.add_directory(
                archive_path(path.strip_prefix(&instance.root).unwrap()),
                SimpleFileOptions::default(),
            )?;
            continue;
        }
        zip.start_file(
            archive_path(path.strip_prefix(&instance.root).unwrap()),
            SimpleFileOptions::default(),
        )?;
        std::io::copy(&mut File::open(path)?, &mut zip)?;
    }
    zip.finish()?;
    output.keep = true;
    Ok(())
}

pub async fn export_mrpack(
    instance: &Instance,
    output: &Path,
    metadata: MrpackMetadata,
) -> Result<(), ExportError> {
    export_mrpack_with_lookup(instance, output, metadata, |hashes| async move {
        Ok(ModrinthApi::new().version_files(&hashes).await?)
    })
    .await
}

/// Supply an empty lookup to export entirely as overrides, or inject an offline resolver.
pub async fn export_mrpack_with_lookup<F, Fut>(
    instance: &Instance,
    output: &Path,
    metadata: MrpackMetadata,
    lookup: F,
) -> Result<(), ExportError>
where
    F: FnOnce(Vec<String>) -> Fut,
    Fut: std::future::Future<Output = Result<BTreeMap<String, ModVersion>, ExportError>>,
{
    if instance.is_horizon() {
        return Err(ExportError::HorizonInstance);
    }
    if metadata.name.trim().is_empty() || metadata.version.trim().is_empty() {
        return Err(ExportError::Invalid(
            "Pack name and version are required".into(),
        ));
    }
    let pack: serde_json::Value =
        serde_json::from_slice(&std::fs::read(instance.root.join("mmc-pack.json"))?)?;
    let mut dependencies = BTreeMap::new();
    for component in pack["components"].as_array().into_iter().flatten() {
        let key = match component["uid"].as_str().unwrap_or_default() {
            "net.minecraft" => "minecraft",
            "net.fabricmc.fabric-loader" => "fabric-loader",
            "org.quiltmc.quilt-loader" => "quilt-loader",
            "net.minecraftforge" => "forge",
            "net.neoforged" => "neoforge",
            _ => continue,
        };
        if let Some(version) = component["version"].as_str() {
            dependencies.insert(key.into(), version.into());
        }
    }
    if !dependencies.contains_key("minecraft") {
        return Err(ExportError::Invalid("Missing Minecraft version".into()));
    }
    let game = instance.game_root();
    let mut paths = Vec::new();
    collect(&game, &game, &mut paths)?;
    paths.retain(|path| !path.is_dir());
    let mut hashes = BTreeMap::new();
    for path in &paths {
        let relative = path.strip_prefix(&game).unwrap();
        let category = relative.components().next().unwrap().as_os_str();
        let filename = path.file_name().unwrap().to_string_lossy();
        let filename = filename.strip_suffix(".disabled").unwrap_or(&filename);
        if [
            "mods",
            "coremods",
            "resourcepacks",
            "texturepacks",
            "shaderpacks",
        ]
        .iter()
        .any(|p| category == *p)
            && (filename.ends_with(".jar")
                || filename.ends_with(".zip")
                || filename.ends_with(".litemod"))
        {
            hashes.insert(
                path.clone(),
                hex::encode(Sha1::digest(std::fs::read(path)?)),
            );
        }
    }
    let resolved = if hashes.is_empty() {
        BTreeMap::new()
    } else {
        lookup(hashes.values().cloned().collect()).await?
    };
    let mut index = MrpackIndex {
        format_version: 1,
        game: "minecraft".into(),
        version_id: metadata.version,
        name: metadata.name,
        summary: Some(metadata.summary),
        files: Vec::new(),
        dependencies,
    };
    let (mut output, mut zip) = writer(instance, output)?;
    for path in paths {
        let relative = archive_path(path.strip_prefix(&game).unwrap());
        let data = std::fs::read(&path)?;
        let sha1 = hex::encode(Sha1::digest(&data));
        let found = hashes
            .get(&path)
            .filter(|hash| **hash == sha1)
            .and_then(|hash| {
                resolved.get(hash).and_then(|version| {
                    version
                        .files
                        .iter()
                        .find(|file| file.hashes.get("sha1") == Some(hash))
                })
            });
        if let Some(file) = found.filter(|f| !f.url.is_empty()) {
            index.files.push(MrpackFile {
                path: relative,
                hashes: BTreeMap::from([
                    ("sha1".into(), sha1),
                    ("sha512".into(), hex::encode(Sha512::digest(&data))),
                ]),
                env: Some(BTreeMap::from([
                    ("client".into(), "required".into()),
                    ("server".into(), "required".into()),
                ])),
                downloads: vec![file.url.clone()],
                file_size: data.len() as u64,
            });
        } else {
            zip.start_file(
                format!("overrides/{relative}"),
                SimpleFileOptions::default(),
            )?;
            zip.write_all(&data)?;
        }
    }
    zip.start_file("modrinth.index.json", SimpleFileOptions::default())?;
    zip.write_all(&serde_json::to_vec_pretty(&index)?)?;
    zip.finish()?;
    output.keep = true;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture(crate::Paths, Instance);
    impl Fixture {
        fn new() -> Self {
            let paths = crate::Paths {
                root: std::env::temp_dir().join(format!("prism-export-{}", uuid::Uuid::new_v4())),
            };
            paths.ensure().unwrap();
            let instance =
                crate::InstanceList::create(&paths, "Export test", "default", "1.20.1").unwrap();
            std::fs::create_dir_all(instance.game_root().join("mods")).unwrap();
            Self(paths, instance)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0.root);
        }
    }
    fn metadata() -> MrpackMetadata {
        MrpackMetadata {
            name: "Test".into(),
            version: "1.0".into(),
            summary: "Example".into(),
        }
    }

    #[tokio::test]
    async fn horizon_links_are_rejected_without_creating_archives() {
        let mut f = Fixture::new();
        f.1.kind = crate::InstanceKind::Horizon {
            id: "native".into(),
            home: f.0.root.join("horizon"),
            token: None,
        };
        let zip = f.0.root.join("horizon.zip");
        let mrpack = f.0.root.join("horizon.mrpack");

        assert!(matches!(export_zip(&f.1, &zip), Err(ExportError::HorizonInstance)));
        assert!(matches!(
            export_mrpack_with_lookup(&f.1, &mrpack, metadata(), |_| async {
                panic!("Horizon export should not resolve Modrinth files")
            }).await,
            Err(ExportError::HorizonInstance)
        ));
        assert!(!zip.exists());
        assert!(!mrpack.exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn dangling_mod_links_are_skipped_in_both_formats() {
        use std::os::unix::fs::symlink;

        let f = Fixture::new();
        symlink("missing.jar", f.1.mods_dir().join("dangling.jar")).unwrap();
        std::fs::write(f.1.mods_dir().join("local.jar"), b"local mod").unwrap();
        let output = f.0.root.join("dangling.zip");
        export_zip(&f.1, &output).unwrap();
        let mut zip = zip::ZipArchive::new(File::open(output).unwrap()).unwrap();
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(
            &mut zip.by_name("minecraft/mods/local.jar").unwrap(),
            &mut bytes,
        )
        .unwrap();
        assert_eq!(bytes, b"local mod");
        assert!(zip.by_name("minecraft/mods/dangling.jar").is_err());

        let output = f.0.root.join("dangling.mrpack");
        export_mrpack_with_lookup(&f.1, &output, metadata(), |hashes| async move {
            assert_eq!(hashes, vec![hex::encode(Sha1::digest(b"local mod"))]);
            Ok(BTreeMap::new())
        })
        .await
        .unwrap();
        let mut zip = zip::ZipArchive::new(File::open(output).unwrap()).unwrap();
        bytes.clear();
        std::io::Read::read_to_end(
            &mut zip.by_name("overrides/mods/local.jar").unwrap(),
            &mut bytes,
        )
        .unwrap();
        assert_eq!(bytes, b"local mod");
        assert!(zip.by_name("overrides/mods/dangling.jar").is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn linked_game_and_resources_are_exported_without_cycles() {
        use std::os::unix::fs::symlink;

        let f = Fixture::new();
        let game = f.1.root.join("minecraft");
        let shared = f.0.root.join("shared-game");
        std::fs::rename(f.1.game_root(), &shared).unwrap();
        symlink(&shared, &game).unwrap();
        let config = f.0.root.join("shared-config");
        std::fs::create_dir(&config).unwrap();
        std::fs::write(config.join("settings.txt"), b"config").unwrap();
        symlink(&config, game.join("config")).unwrap();
        let jar = f.0.root.join("shared.jar");
        std::fs::write(&jar, b"mod").unwrap();
        symlink(&jar, game.join("mods/linked.jar")).unwrap();
        symlink(&game, game.join("cycle")).unwrap();

        let output = f.0.root.join("linked.zip");
        export_zip(&f.1, &output).unwrap();
        let mut zip = zip::ZipArchive::new(File::open(output).unwrap()).unwrap();
        assert!(zip.by_name("minecraft/config/settings.txt").is_ok());
        assert!(zip.by_name("minecraft/mods/linked.jar").is_ok());
        assert!(!zip.file_names().any(|name| name.contains("cycle/")));

        let output = f.0.root.join("linked.mrpack");
        export_mrpack_with_lookup(&f.1, &output, metadata(), |hashes| async move {
            assert_eq!(hashes, vec![hex::encode(Sha1::digest(b"mod"))]);
            Ok(BTreeMap::new())
        })
        .await
        .unwrap();
        let mut zip = zip::ZipArchive::new(File::open(output).unwrap()).unwrap();
        assert!(zip.by_name("overrides/config/settings.txt").is_ok());
        assert!(zip.by_name("overrides/mods/linked.jar").is_ok());
        assert!(!zip.file_names().any(|name| name.contains("cycle/")));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn failed_exports_remove_partial_output() {
        use std::os::unix::fs::PermissionsExt;

        let f = Fixture::new();
        let unreadable = f.1.game_root().join("options.txt");
        std::fs::write(&unreadable, "options").unwrap();
        std::fs::set_permissions(&unreadable, std::fs::Permissions::from_mode(0)).unwrap();
        let zip = f.0.root.join("failed.zip");
        assert!(export_zip(&f.1, &zip).is_err());
        assert!(!zip.exists());
        let mrpack = f.0.root.join("failed.mrpack");
        assert!(
            export_mrpack_with_lookup(&f.1, &mrpack, metadata(), |_| async { Ok(BTreeMap::new()) })
                .await
                .is_err()
        );
        assert!(!mrpack.exists());
        std::fs::write(&zip, "existing").unwrap();
        assert!(export_zip(&f.1, &zip).is_err());
        assert_eq!(std::fs::read_to_string(zip).unwrap(), "existing");
    }

    #[test]
    fn prism_round_trip_and_defaults() {
        let f = Fixture::new();
        for name in [
            "config/test.txt",
            "logs/latest.log",
            ".fabric/cache",
            "crash-reports/crash.txt",
        ] {
            let path = f.1.game_root().join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "test").unwrap();
        }
        let output = f.0.root.join("test.zip");
        export_zip(&f.1, &output).unwrap();
        let imported = crate::import::import_sync(&f.0, &output, None).unwrap().instance;
        assert_eq!(imported.name, f.1.name);
        assert!(imported.game_root().join("config/test.txt").exists());
        assert!(!imported.game_root().join("logs").exists());
        assert!(!imported.game_root().join(".fabric").exists());
        assert!(!imported.game_root().join("crash-reports").exists());
        assert!(export_zip(&f.1, &output).is_err());
        assert!(export_zip(&f.1, &f.1.root.join("bad.zip")).is_err());
    }

    #[test]
    fn empty_game_and_nested_directories_survive_zip_round_trip() {
        let f = Fixture::new();
        std::fs::remove_dir(f.1.mods_dir()).unwrap();
        let output = f.0.root.join("vanilla.zip");
        export_zip(&f.1, &output).unwrap();
        let mut zip = zip::ZipArchive::new(File::open(&output).unwrap()).unwrap();
        assert!(zip.by_name("minecraft/").unwrap().is_dir());
        let imported = crate::import::import_sync(&f.0, &output, None).unwrap().instance;
        assert_eq!(imported.game_root(), imported.root.join("minecraft"));
        assert!(imported.game_root().is_dir());

        std::fs::create_dir_all(f.1.game_root().join("config/empty")).unwrap();
        let output = f.0.root.join("nested.zip");
        export_zip(&f.1, &output).unwrap();
        let imported = crate::import::import_sync(&f.0, &output, None).unwrap().instance;
        assert!(imported.game_root().join("config/empty").is_dir());
    }

    #[tokio::test]
    async fn mrpack_resolved_files_and_overrides() {
        let f = Fixture::new();
        std::fs::write(f.1.mods_dir().join("known.jar"), b"known").unwrap();
        std::fs::write(f.1.mods_dir().join("unknown.jar"), b"unknown").unwrap();
        std::fs::write(f.1.game_root().join("options.txt"), b"options").unwrap();
        crate::instance::set_loader(
            &f.1,
            &crate::loaders::Loader::Fabric {
                version: "0.16.0".into(),
            },
        )
        .unwrap();
        let hash = hex::encode(Sha1::digest(b"known"));
        let output = f.0.root.join("test.mrpack");
        export_mrpack_with_lookup(&f.1, &output, metadata(), |hashes| async move {
            assert_eq!(hashes.len(), 2);
            let version: ModVersion = serde_json::from_value(serde_json::json!({"id":"1", "project_id":"p", "name":"Known", "version_number":"1", "files":[{"url":"https://cdn.modrinth.com/known.jar", "filename":"known.jar", "primary":true, "size":5, "hashes":{"sha1": hash}}]})).unwrap();
            Ok(BTreeMap::from([(hash, version)]))
        }).await.unwrap();
        let mut zip = zip::ZipArchive::new(File::open(output).unwrap()).unwrap();
        let index: serde_json::Value =
            serde_json::from_reader(zip.by_name("modrinth.index.json").unwrap()).unwrap();
        assert_eq!(index["formatVersion"], 1);
        assert_eq!(index["versionId"], "1.0");
        assert_eq!(index["game"], "minecraft");
        assert_eq!(index["dependencies"]["minecraft"], "1.20.1");
        assert_eq!(index["dependencies"]["fabric-loader"], "0.16.0");
        assert_eq!(index["files"].as_array().unwrap().len(), 1);
        assert_eq!(index["files"][0]["path"], "mods/known.jar");
        assert_eq!(index["files"][0]["fileSize"], 5);
        assert_eq!(
            index["files"][0]["hashes"]["sha512"],
            hex::encode(Sha512::digest(b"known"))
        );
        assert_eq!(index["files"][0]["env"]["client"], "required");
        assert!(zip.by_name("overrides/mods/unknown.jar").is_ok());
        assert!(zip.by_name("overrides/options.txt").is_ok());
        assert!(zip.by_name("overrides/mods/known.jar").is_err());
    }

    #[tokio::test]
    async fn changed_resolved_file_is_exported_as_override() {
        let f = Fixture::new();
        let jar = f.1.mods_dir().join("changing.jar");
        std::fs::write(&jar, b"original").unwrap();
        let output = f.0.root.join("changed.mrpack");
        export_mrpack_with_lookup(&f.1, &output, metadata(), |hashes| async move {
            let hash = hex::encode(Sha1::digest(b"original"));
            assert_eq!(hashes, vec![hash.clone()]);
            std::fs::write(jar, b"changed").unwrap();
            let version: ModVersion = serde_json::from_value(serde_json::json!({
                "id": "1", "project_id": "p", "name": "Original", "version_number": "1",
                "files": [{"url": "https://cdn.modrinth.com/original.jar", "filename": "original.jar", "primary": true, "size": 8, "hashes": {"sha1": hash}}]
            })).unwrap();
            Ok(BTreeMap::from([(hash, version)]))
        }).await.unwrap();
        let mut zip = zip::ZipArchive::new(File::open(output).unwrap()).unwrap();
        let index: MrpackIndex =
            serde_json::from_reader(zip.by_name("modrinth.index.json").unwrap()).unwrap();
        assert!(index.files.is_empty());
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(
            &mut zip.by_name("overrides/mods/changing.jar").unwrap(),
            &mut bytes,
        )
        .unwrap();
        assert_eq!(bytes, b"changed");
    }

    #[tokio::test]
    async fn empty_pack_and_lookup_error() {
        let f = Fixture::new();
        export_mrpack_with_lookup(
            &f.1,
            &f.0.root.join("empty.mrpack"),
            metadata(),
            |_| async { panic!("empty pack needs no lookup") },
        )
        .await
        .unwrap();
        std::fs::write(f.1.mods_dir().join("mod.jar"), b"mod").unwrap();
        let output = f.0.root.join("failed.mrpack");
        let error = export_mrpack_with_lookup(&f.1, &output, metadata(), |_| async {
            Err(ExportError::Invalid("offline".into()))
        })
        .await
        .unwrap_err();
        assert_eq!(error.to_string(), "offline");
        assert!(!output.exists());
    }
}
