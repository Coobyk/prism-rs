use super::PlatformError;
use crate::import::{extract_zip, ImportError};
use crate::loaders::Loader;
use crate::net::{DownloadRequest, Downloader};
use crate::version_components::{version_id, ArchiveKind, Pack};
use crate::{Instance, InstanceList, Paths};
use md5::{Digest, Md5};
use reqwest::Url;
use serde::{de::DeserializeOwned, Deserialize};
use std::{
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
    sync::Arc,
};

const PLATFORM: &str = "https://api.technicpack.net/";

#[derive(Debug, Deserialize)]
pub struct Listing {
    pub name: String,
    pub slug: String,
}

#[derive(Deserialize)]
struct SearchResults {
    modpacks: Vec<Listing>,
}

#[derive(Debug, Deserialize)]
struct PlatformPack {
    name: String,
    #[serde(rename = "displayName")]
    title: String,
    #[serde(default)]
    user: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    minecraft: String,
    version: String,
    url: Option<String>,
    solder: Option<String>,
}

#[derive(Debug)]
pub struct Modpack {
    pub title: String,
    pub author: String,
    pub description: String,
    pub minecraft: String,
    pub builds: Vec<String>,
    pub recommended: String,
    slug: String,
    source: Source,
}

#[derive(Debug)]
enum Source {
    Zip(Url),
    Solder(Url),
}

#[derive(Deserialize)]
struct SolderPack {
    recommended: String,
    builds: Vec<String>,
}

#[derive(Deserialize)]
struct SolderBuild {
    #[serde(default)]
    minecraft: String,
    mods: Vec<SolderMod>,
}

#[derive(Deserialize)]
struct SolderMod {
    url: String,
    #[serde(default)]
    md5: String,
}

fn endpoint(mut base: Url, segments: &[&str]) -> Url {
    base.path_segments_mut()
        .expect("HTTP URL")
        .pop_if_empty()
        .extend(segments);
    base
}

fn platform(segments: &[&str]) -> Url {
    let mut url = endpoint(Url::parse(PLATFORM).unwrap(), segments);
    url.query_pairs_mut().append_pair("build", "multimc");
    url
}

async fn get<T: DeserializeOwned>(dl: &Downloader, url: Url) -> Result<T, PlatformError> {
    Ok(dl
        .client()
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?)
}

fn slug_valid(slug: &str) -> bool {
    !slug.is_empty()
        && slug
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

pub async fn search(dl: &Downloader, term: &str) -> Result<Vec<Listing>, PlatformError> {
    let term = term.trim();
    if let Some(slug) = term.strip_prefix('@').filter(|slug| slug_valid(slug)) {
        let pack: PlatformPack = get(dl, platform(&["modpack", slug])).await?;
        return Ok(vec![Listing {
            slug: pack.name,
            name: pack.title,
        }]);
    }
    let url = if term.is_empty() {
        platform(&["trending"])
    } else {
        let mut url = platform(&["search"]);
        url.query_pairs_mut().append_pair("q", term);
        url
    };
    let list: SearchResults = get(dl, url).await?;
    Ok(list
        .modpacks
        .into_iter()
        .filter(|pack| pack.slug != "vanilla" && slug_valid(&pack.slug))
        .collect())
}

fn problem(message: impl Into<String>) -> ImportError {
    ImportError::Other(message.into())
}

fn web_url(text: &str) -> Result<Url, ImportError> {
    let url = Url::parse(text).map_err(|_| problem("Invalid Technic download URL."))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(problem("Invalid Technic download URL."));
    }
    Ok(url)
}

pub async fn details(dl: &Downloader, slug: &str) -> Result<Modpack, ImportError> {
    if !slug_valid(slug) {
        return Err(problem("Invalid Technic pack slug."));
    }
    let info: PlatformPack = get(dl, platform(&["modpack", slug])).await?;
    let (source, mut builds, recommended) =
        if let Some(url) = info.url.as_deref().filter(|url| !url.is_empty()) {
            (
                Source::Zip(web_url(url)?),
                vec![info.version.clone()],
                info.version,
            )
        } else {
            let url = web_url(
                info.solder
                    .as_deref()
                    .ok_or_else(|| problem("This pack has no download source."))?,
            )?;
            let data: SolderPack = get(dl, endpoint(url.clone(), &["modpack", slug])).await?;
            (
                Source::Solder(url),
                data.builds.into_iter().rev().collect(),
                data.recommended,
            )
        };
    builds.retain(|build| !build.trim().is_empty() && !matches!(build.as_str(), "." | ".."));
    if builds.is_empty() {
        return Err(problem("This pack has no available builds."));
    }
    Ok(Modpack {
        title: info.title,
        author: info.user,
        description: info.description,
        minecraft: info.minecraft,
        builds,
        recommended,
        slug: slug.into(),
        source,
    })
}

pub async fn install(
    paths: &Paths,
    dl: &Arc<Downloader>,
    pack: &Modpack,
    build: &str,
) -> Result<Instance, ImportError> {
    if !pack.builds.iter().any(|value| value == build) {
        return Err(problem("Select an available Technic build."));
    }
    let scratch = tempfile::tempdir()?;
    let (minecraft, archives) = match &pack.source {
        Source::Zip(url) => (pack.minecraft.clone(), vec![(url.clone(), String::new())]),
        Source::Solder(url) => {
            let data: SolderBuild =
                get(dl, endpoint(url.clone(), &["modpack", &pack.slug, build])).await?;
            let archives = data
                .mods
                .into_iter()
                .map(|file| Ok((web_url(&file.url)?, file.md5)))
                .collect::<Result<Vec<_>, ImportError>>()?;
            (
                if data.minecraft.trim().is_empty() {
                    pack.minecraft.clone()
                } else {
                    data.minecraft
                },
                archives,
            )
        }
    };
    let files: Vec<_> = archives
        .into_iter()
        .enumerate()
        .map(|(i, (url, hash))| {
            (
                DownloadRequest::new(url, scratch.path().join(format!("{i}.zip"))),
                hash,
            )
        })
        .collect();
    dl.download_all(
        files.iter().map(|(request, _)| request.clone()).collect(),
        4,
        |_| {},
    )
    .await?;
    let paths = paths.clone();
    let title = pack.title.clone();
    tokio::task::spawn_blocking(move || {
        let game = scratch.path().join("game");
        fs::create_dir(&game)?;
        for (file, hash) in files {
            verify_md5(&file.dest, &hash)?;
            extract_zip(&file.dest, &game)?;
        }
        install_staged(&paths, &title, &game, &minecraft)
    })
    .await
    .map_err(io::Error::other)?
}

fn verify_md5(path: &Path, expected: &str) -> Result<(), ImportError> {
    if !expected.is_empty() {
        let mut file = fs::File::open(path)?;
        let mut digest = Md5::new();
        io::copy(&mut file, &mut digest)?;
        if !format!("{:x}", digest.finalize()).eq_ignore_ascii_case(expected) {
            return Err(problem("A Technic Solder download failed its MD5 check."));
        }
    }
    Ok(())
}

#[derive(Debug, PartialEq)]
struct Recipe {
    minecraft: String,
    loader: Loader,
    jar_mod: bool,
}

#[derive(Deserialize)]
struct VersionJson {
    #[serde(default, rename = "inheritsFrom")]
    inherits_from: String,
    #[serde(default)]
    libraries: Vec<Library>,
    #[serde(default)]
    arguments: Arguments,
}

#[derive(Deserialize)]
struct Library {
    name: String,
}

#[derive(Default, Deserialize)]
struct Arguments {
    #[serde(default)]
    game: Vec<serde_json::Value>,
}

fn jar_text(
    jar: &mut zip::ZipArchive<fs::File>,
    name: &str,
) -> Result<Option<String>, ImportError> {
    match jar.by_name(name) {
        Ok(file) => Ok(Some(metadata_text(file)?)),
        Err(zip::result::ZipError::FileNotFound) => Ok(None),
        Err(error) => Err(ImportError::Zip(error.to_string())),
    }
}

fn metadata_text(reader: impl Read) -> Result<String, ImportError> {
    let limit = 4 * 1024 * 1024;
    let mut text = String::new();
    reader.take(limit + 1).read_to_string(&mut text)?;
    if text.len() as u64 > limit {
        return Err(problem("Technic pack version metadata is too large."));
    }
    Ok(text)
}

fn property<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    text.lines()
        .filter_map(|line| line.split_once('='))
        .find_map(|(name, value)| (name.trim() == key).then(|| value.trim()))
        .filter(|value| !value.is_empty())
}

fn recipe(game: &Path, fallback: &str) -> Result<Recipe, ImportError> {
    let jar = game.join("bin/modpack.jar");
    let external = game.join("bin/version.json");
    let mut fml = String::new();
    let mut legacy = Loader::None;
    let mut jar_mod = false;
    let version = if jar.is_file() {
        let mut zip = zip::ZipArchive::new(fs::File::open(&jar)?)
            .map_err(|error| ImportError::Zip(error.to_string()))?;
        let version = jar_text(&mut zip, "version.json")?;
        if version.is_some() {
            fml = jar_text(&mut zip, "fmlversion.properties")?.unwrap_or_default();
        } else {
            jar_mod = true;
            if let Some(data) = jar_text(&mut zip, "forgeversion.properties")? {
                let version = ["major", "minor", "revision", "build"]
                    .iter()
                    .map(|part| property(&data, &format!("forge.{part}.number")))
                    .collect::<Option<Vec<_>>>()
                    .ok_or_else(|| problem("Invalid Technic forgeversion.properties."))?
                    .join(".");
                legacy = Loader::Forge { version };
            }
        }
        version
    } else if external.is_file() {
        Some(metadata_text(fs::File::open(external)?)?)
    } else {
        None
    };
    let (minecraft, loader) = if let Some(version) = version {
        let json: VersionJson = serde_json::from_str(&version)?;
        let minecraft = if json.inherits_from.trim().is_empty() {
            property(&fml, "fmlbuild.mcversion").unwrap_or(fallback)
        } else {
            &json.inherits_from
        };
        (
            checked_minecraft(minecraft)?,
            loader_from_version(&json, minecraft),
        )
    } else {
        (checked_minecraft(fallback)?, legacy)
    };
    Ok(Recipe {
        minecraft,
        loader,
        jar_mod,
    })
}

fn checked_minecraft(value: &str) -> Result<String, ImportError> {
    version_id(value)
        .map(str::to_owned)
        .map_err(|_| problem("The Technic pack does not specify a valid Minecraft version."))
}

fn loader_from_version(json: &VersionJson, minecraft: &str) -> Loader {
    for library in &json.libraries {
        let mut coordinate = library.name.split(':');
        let (Some(group), Some(artifact), Some(version)) =
            (coordinate.next(), coordinate.next(), coordinate.next())
        else {
            continue;
        };
        let version = version.split('@').next().unwrap_or_default();
        if version.is_empty() {
            continue;
        }
        match (group, artifact) {
            ("net.minecraftforge", "forge" | "fmlloader" | "minecraftforge") => {
                let version = version
                    .strip_prefix(minecraft)
                    .and_then(|v| v.strip_prefix('-'))
                    .unwrap_or(version);
                let version = if minecraft == "1.7.10" {
                    version.split('-').next().unwrap_or(version)
                } else {
                    version
                };
                return Loader::Forge {
                    version: version.into(),
                };
            }
            ("net.fabricmc", "fabric-loader") => {
                return Loader::Fabric {
                    version: version.into(),
                }
            }
            ("org.quiltmc", "quilt-loader") => {
                return Loader::Quilt {
                    version: version.into(),
                }
            }
            ("net.neoforged", "neoforge") => {
                return Loader::NeoForge {
                    version: version.into(),
                }
            }
            ("net.neoforged.fancymodloader", _) => {
                if let Some(version) = json.arguments.game.windows(2).find_map(|pair| {
                    matches!(
                        pair[0].as_str(),
                        Some("--fml.neoForgeVersion" | "--fml.forgeVersion")
                    )
                    .then(|| pair[1].as_str())
                    .flatten()
                }) {
                    return Loader::NeoForge {
                        version: version.into(),
                    };
                }
            }
            _ => {}
        }
    }
    Loader::None
}

fn install_staged(
    paths: &Paths,
    title: &str,
    game: &Path,
    fallback: &str,
) -> Result<Instance, ImportError> {
    let recipe = recipe(game, fallback)?;
    let instances = paths.instances_dir();
    fs::create_dir_all(&instances)?;
    let staging = tempfile::Builder::new()
        .prefix(".technic-")
        .tempdir_in(&instances)?;
    let staging_paths = Paths {
        root: staging.path().to_owned(),
    };
    let mut instance =
        InstanceList::create(&staging_paths, "Technic", "default", &recipe.minecraft)?;
    instance.name = title.to_owned();
    instance.save()?;
    crate::import::copy_dir_recursive(game, &instance.game_root())?;
    let profile = Pack::load(&instance.root)?.set_loader(&recipe.loader)?;
    if recipe.jar_mod {
        profile.add_archive(
            &instance.game_root().join("bin/modpack.jar"),
            ArchiveKind::JarMod,
        )?;
    }
    publish(&instances, &instance.root, title)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn fixture(game: &Path, entries: &[(&str, &str)]) -> PathBuf {
        let bin = game.join("bin");
        fs::create_dir_all(&bin).unwrap();
        let jar = bin.join("modpack.jar");
        let mut zip = zip::ZipWriter::new(fs::File::create(&jar).unwrap());
        for (name, contents) in entries {
            zip.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(contents.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
        jar
    }

    #[test]
    fn embedded_metadata_prefers_inherited_minecraft_and_extracts_legacy_forge_coordinate() {
        let game = tempfile::tempdir().unwrap();
        fixture(
            game.path(),
            &[(
                "version.json",
                r#"{"inheritsFrom":"1.7.10","libraries":[{"name":"net.minecraftforge:forge:1.7.10-10.13.4.1614-1.7.10"}]}"#,
            )],
        );
        assert_eq!(
            recipe(game.path(), "1.12.2").unwrap(),
            Recipe {
                minecraft: "1.7.10".into(),
                loader: Loader::Forge {
                    version: "10.13.4.1614".into()
                },
                jar_mod: false,
            }
        );
    }

    #[test]
    fn external_version_json_installs_fabric_component() {
        let dir = tempfile::tempdir().unwrap();
        let game = dir.path().join("game");
        fs::create_dir_all(game.join("bin")).unwrap();
        fs::write(game.join("bin/version.json"), r#"{"inheritsFrom":"1.21.1","libraries":[{"name":"net.fabricmc:fabric-loader:0.16.10"}]}"#).unwrap();
        let paths = Paths {
            root: dir.path().join("launcher"),
        };
        let instance = install_staged(&paths, "Technic fixture", &game, "1.20.1").unwrap();
        assert_eq!(
            crate::instance::loader_of(&instance),
            Loader::Fabric {
                version: "0.16.10".into()
            }
        );
        assert_eq!(
            Pack::load(&instance.root).unwrap().version("net.minecraft"),
            Some("1.21.1")
        );
    }

    #[test]
    fn legacy_jar_is_installed_as_a_jar_mod_with_forge_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let game = dir.path().join("game");
        fixture(&game, &[
            ("old.class", "fixture"),
            ("forgeversion.properties", "forge.major.number=7\nforge.minor.number=8\nforge.revision.number=1\nforge.build.number=738"),
        ]);
        let paths = Paths {
            root: dir.path().join("launcher"),
        };
        let instance = install_staged(&paths, "Legacy", &game, "1.5.2").unwrap();
        let profile = Pack::load(&instance.root).unwrap();
        assert_eq!(profile.version("net.minecraftforge"), Some("7.8.1.738"));
        assert_eq!(profile.archive_paths(ArchiveKind::JarMod).unwrap().len(), 1);
    }

    #[test]
    fn rejects_a_bad_solder_hash_and_encodes_build_segments() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file");
        fs::write(&path, b"abc").unwrap();
        assert!(verify_md5(&path, "900150983CD24FB0D6963F7D28E17F72").is_ok());
        assert!(verify_md5(&path, "00000000000000000000000000000000").is_err());
        assert_eq!(
            endpoint(
                Url::parse("https://repo.example/api/").unwrap(),
                &["modpack", "example", "version / 2"]
            )
            .path(),
            "/api/modpack/example/version%20%2F%202"
        );
    }
}

fn publish(instances: &Path, source: &Path, title: &str) -> Result<Instance, ImportError> {
    let base: String = title
        .chars()
        .take(100)
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let base = if base.trim_matches('_').is_empty() {
        "Technic"
    } else {
        &base
    };
    let destination: PathBuf = (1..)
        .find_map(|number| {
            let name = if number == 1 {
                base.to_owned()
            } else {
                format!("{base}{number}")
            };
            let candidate = instances.join(name);
            match fs::create_dir(&candidate) {
                Ok(()) => Some(Ok(candidate)),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => None,
                Err(error) => Some(Err(error)),
            }
        })
        .unwrap()?;
    let result = (|| {
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            if entry.file_name() != "instance.cfg" {
                fs::rename(entry.path(), destination.join(entry.file_name()))?;
            }
        }
        fs::rename(
            source.join("instance.cfg"),
            destination.join("instance.cfg"),
        )?;
        Instance::load(&destination)
            .ok_or_else(|| problem("Could not open the imported Technic instance."))
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(destination);
    }
    result
}
