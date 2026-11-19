use super::PlatformError;
use crate::import::{extract_zip, ImportError};
use crate::loaders::Loader;
use crate::meta::{self, VersionData};
use crate::net::{DownloadRequest, Downloader};
use crate::profile::LaunchProfile;
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
const FORGE_PATCH: &str = "technic-forge.json";
const BUNDLED_LIBRARIES: &str = "technic-libraries";

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

fn forge_library_url(text: &str) -> Result<Url, ImportError> {
    let mut url = web_url(text)?;
    if url.scheme() == "http"
        && matches!(
            url.host_str(),
            Some(
                "files.minecraftforge.net"
                    | "maven.minecraftforge.net"
                    | "libraries.minecraft.net"
                    | "repo1.maven.org"
                    | "maven.apache.org"
            )
        )
    {
        url.set_scheme("https")
            .map_err(|_| problem("Invalid Technic Forge library URL."))?;
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
    fs::create_dir_all(&paths.root)?;
    let scratch = tempfile::Builder::new()
        .prefix(".technic-download-")
        .tempdir_in(&paths.root)?;
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
            fs::remove_file(&file.dest)?;
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
    forge_patch: Option<String>,
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

fn can_use_embedded_forge(
    jar: &mut zip::ZipArchive<fs::File>,
    minecraft: &str,
) -> Result<bool, ImportError> {
    let legacy = minecraft
        .strip_prefix("1.")
        .and_then(|version| version.split('.').next())
        .and_then(|minor| minor.parse::<u32>().ok())
        .is_some_and(|minor| minor < 13);
    if !legacy {
        return Ok(false);
    }
    let Some(text) = jar_text(jar, "install_profile.json")? else {
        return Ok(true);
    };
    let installer: serde_json::Value = serde_json::from_str(&text)?;
    Ok(match installer.get("processors") {
        None => true,
        Some(processors) => processors.as_array().is_some_and(Vec::is_empty),
    })
}

fn recipe(game: &Path, fallback: &str) -> Result<Recipe, ImportError> {
    let jar = game.join("bin/modpack.jar");
    let external = game.join("bin/version.json");
    let mut fml = String::new();
    let mut jar_mod = false;
    let mut forge_patch = None;
    let mut modpack_jar = None;
    let version = if jar.is_file() {
        let mut zip = zip::ZipArchive::new(fs::File::open(&jar)?)
            .map_err(|error| ImportError::Zip(error.to_string()))?;
        let version = jar_text(&mut zip, "version.json")?;
        if version.is_some() {
            fml = jar_text(&mut zip, "fmlversion.properties")?.unwrap_or_default();
            modpack_jar = Some(zip);
        } else {
            jar_mod = true;
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
        let loader = loader_from_version(&json, minecraft);
        if matches!(loader, Loader::Forge { .. })
            && json
                .libraries
                .iter()
                .any(|library| bundled_forge(&library.name))
        {
            if let Some(zip) = &mut modpack_jar {
                if can_use_embedded_forge(zip, minecraft)? {
                    forge_patch = Some(version);
                }
            }
        }
        (checked_minecraft(minecraft)?, loader)
    } else {
        (checked_minecraft(fallback)?, Loader::None)
    };
    Ok(Recipe {
        minecraft,
        loader,
        jar_mod,
        forge_patch,
    })
}

fn bundled_forge(spec: &str) -> bool {
    let (group, artifact, _, classifier) = meta::parse_gradle_specifier(spec);
    group == "net.minecraftforge"
        && matches!(artifact.as_str(), "forge" | "minecraftforge")
        && classifier
            .as_deref()
            .is_none_or(|classifier| classifier == "universal")
}

fn maven_part(part: &str) -> bool {
    !part.is_empty()
        && !matches!(part, "." | "..")
        && part
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._+-".contains(&b))
}

fn checked_maven_path(spec: &str) -> Result<PathBuf, ImportError> {
    let (group, artifact, version, classifier) = meta::parse_gradle_specifier(spec);
    if !group.split('.').all(maven_part)
        || !maven_part(&artifact)
        || !maven_part(&version)
        || classifier
            .as_deref()
            .is_some_and(|part| !maven_part(part.trim_start_matches('@')))
    {
        return Err(problem(
            "Invalid Maven coordinate in the Technic Forge profile.",
        ));
    }
    Ok(meta::maven_path(spec))
}

fn forge_patch(
    text: &str,
    minecraft: &str,
    loader: &Loader,
) -> Result<Option<VersionData>, ImportError> {
    let json: serde_json::Value = serde_json::from_str(text)?;
    let recipe: VersionJson = serde_json::from_value(json.clone())?;
    if (!recipe.inherits_from.is_empty() && recipe.inherits_from != minecraft)
        || &loader_from_version(&recipe, minecraft) != loader
    {
        return Ok(None);
    }
    let mut version = VersionData::from_json(&json)?;
    if version.main_class.is_empty()
        || !version.libraries.iter().any(|lib| bundled_forge(&lib.name))
    {
        return Err(problem(
            "The Technic Forge profile is missing its main class or bundled Forge library.",
        ));
    }
    for lib in &mut version.libraries {
        checked_maven_path(&lib.name)?;
        if let Some(natives) = &lib.natives {
            for classifier in natives.values() {
                checked_maven_path(&format!(
                    "{}:{}",
                    lib.name,
                    classifier.replace("${arch}", "64")
                ))?;
            }
        }
        if bundled_forge(&lib.name) {
            lib.url = None;
            lib.downloads = None;
            lib.natives = None;
            continue;
        }
        let repository = match lib
            .url
            .as_deref()
            .filter(|url| !url.is_empty())
            .map(|url| url.trim_end_matches('/'))
        {
            Some(
                "http://files.minecraftforge.net/maven" | "https://files.minecraftforge.net/maven",
            ) => "https://maven.minecraftforge.net",
            Some(url) => url,
            None => "https://libraries.minecraft.net",
        };
        lib.url = Some(forge_library_url(repository)?.to_string());
        if let Some(downloads) = &mut lib.downloads {
            if downloads
                .artifact
                .as_ref()
                .is_some_and(|artifact| artifact.url.is_empty())
            {
                downloads.artifact = None;
            }
            if let Some(artifact) = &mut downloads.artifact {
                artifact.path = None;
                artifact.url = forge_library_url(&artifact.url)?.to_string();
            }
            downloads
                .classifiers
                .retain(|_, artifact| !artifact.url.is_empty());
            for artifact in downloads.classifiers.values_mut() {
                artifact.url = forge_library_url(&artifact.url)?.to_string();
            }
        }
    }
    Ok(Some(version))
}

pub fn apply_forge_patch(
    profile: &mut LaunchProfile,
    instance: &Path,
    game: &Path,
    loader: &Loader,
) -> Result<bool, ImportError> {
    if !matches!(loader, Loader::Forge { .. }) {
        return Ok(false);
    }
    let patch = match fs::File::open(instance.join(FORGE_PATCH)) {
        Ok(file) => metadata_text(file)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    let jar = game.join("bin/modpack.jar");
    if !jar.is_file() {
        return Err(problem("The Technic Forge profile needs bin/modpack.jar."));
    }
    let mut zip = zip::ZipArchive::new(fs::File::open(&jar)?)
        .map_err(|error| ImportError::Zip(error.to_string()))?;
    if !can_use_embedded_forge(&mut zip, &profile.minecraft_version)? {
        return Ok(false);
    }
    let Some(patch) = forge_patch(&patch, &profile.minecraft_version, loader)? else {
        return Ok(false);
    };
    let game = game.canonicalize()?;
    let jar = game.join("bin/modpack.jar");
    let bundled_root = instance.canonicalize()?.join(BUNDLED_LIBRARIES);
    let library_root = game.join("libraries");
    let (libraries, natives) = (profile.libraries.len(), profile.natives.len());
    profile.apply_loader(&patch);
    for library in profile.libraries[libraries..]
        .iter_mut()
        .chain(&mut profile.natives[natives..])
    {
        if bundled_forge(&library.spec) {
            let embedded = bundled_root.join(checked_maven_path(&library.spec)?);
            library.path = if embedded.is_file() {
                embedded
            } else {
                jar.clone()
            };
        } else {
            library.path = library_root.join(checked_maven_path(&library.spec)?);
        }
    }
    Ok(true)
}

fn extract_bundled_forge(
    instance: &Path,
    game: &Path,
    patch: &VersionData,
) -> Result<(), ImportError> {
    let mut jar = zip::ZipArchive::new(fs::File::open(game.join("bin/modpack.jar"))?)
        .map_err(|error| ImportError::Zip(error.to_string()))?;
    for library in patch
        .libraries
        .iter()
        .filter(|lib| bundled_forge(&lib.name))
    {
        let path = checked_maven_path(&library.name)?;
        let entry = format!("maven/{}", path.to_string_lossy().replace('\\', "/"));
        let mut embedded = match jar.by_name(&entry) {
            Ok(embedded) => embedded,
            Err(zip::result::ZipError::FileNotFound) => continue,
            Err(error) => return Err(ImportError::Zip(error.to_string())),
        };
        let target = instance.join(BUNDLED_LIBRARIES).join(path);
        fs::create_dir_all(target.parent().unwrap())?;
        io::copy(&mut embedded, &mut fs::File::create(&target)?)?;
    }
    Ok(())
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
    let game_root = instance.game_root();
    fs::remove_dir(&game_root)?;
    match fs::rename(game, &game_root) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::CrossesDevices => {
            crate::import::copy_dir_recursive(game, &game_root)?;
        }
        Err(error) => return Err(error.into()),
    }
    let profile = Pack::load(&instance.root)?.set_loader(&recipe.loader)?;
    if let Some(patch) = recipe.forge_patch {
        if let Some(version) = forge_patch(&patch, &recipe.minecraft, &recipe.loader)? {
            extract_bundled_forge(&instance.root, &game_root, &version)?;
        }
        fs::write(instance.root.join(FORGE_PATCH), patch)?;
    }
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
                    version: "10.13.4.1614-1.7.10".into()
                },
                jar_mod: false,
                forge_patch: Some(r#"{"inheritsFrom":"1.7.10","libraries":[{"name":"net.minecraftforge:forge:1.7.10-10.13.4.1614-1.7.10"}]}"#.into()),
            }
        );
    }

    #[test]
    fn embedded_forge_patch_uses_pack_jar_and_downloads_other_maven_libraries() {
        let dir = tempfile::tempdir().unwrap();
        let game = dir.path().join("game");
        fixture(
            &game,
            &[(
                "version.json",
                r#"{
            "inheritsFrom":"1.12.2", "mainClass":"net.minecraft.launchwrapper.Launch",
            "minecraftArguments":"--tweakClass net.minecraftforge.fml.common.launcher.FMLTweaker",
            "libraries":[
                {"name":"net.minecraftforge:forge:1.12.2-14.23.5.2864","downloads":{"artifact":{"url":""}}},
                {"name":"net.minecraft:launchwrapper:1.12"},
                {"name":"org.ow2.asm:asm-debug-all:5.2","url":"http://files.minecraftforge.net/maven/"}
            ]
        }"#,
            )],
        );
        let paths = Paths {
            root: dir.path().join("launcher"),
        };
        let installed = install_staged(&paths, "Embedded Forge", &game, "1.12.2").unwrap();
        let loader = crate::instance::loader_of(&installed);
        let game = installed.game_root().canonicalize().unwrap();
        let mut profile = LaunchProfile {
            minecraft_version: "1.12.2".into(),
            ..Default::default()
        };
        assert!(apply_forge_patch(&mut profile, &installed.root, &game, &loader).unwrap());
        assert_eq!(profile.main_class, "net.minecraft.launchwrapper.Launch");
        assert!(profile
            .legacy_arguments
            .as_deref()
            .unwrap()
            .contains("FMLTweaker"));
        assert_eq!(profile.libraries[0].path, game.join("bin/modpack.jar"));
        assert_eq!(profile.libraries[0].url, None);
        assert_eq!(
            profile.libraries[1].path,
            game.join("libraries/net/minecraft/launchwrapper/1.12/launchwrapper-1.12.jar")
        );
        assert_eq!(profile.libraries[1].url.as_deref(), Some("https://libraries.minecraft.net/net/minecraft/launchwrapper/1.12/launchwrapper-1.12.jar"));
        assert_eq!(profile.libraries[2].url.as_deref(), Some("https://maven.minecraftforge.net/org/ow2/asm/asm-debug-all/5.2/asm-debug-all-5.2.jar"));
        assert!(!apply_forge_patch(
            &mut profile,
            &installed.root,
            &game,
            &Loader::Forge {
                version: "other".into()
            }
        )
        .unwrap());
        assert!(checked_maven_path("org.example:../outside:1").is_err());
    }

    #[test]
    fn forge_versions_that_need_processing_keep_the_installer_path() {
        for (minecraft, version, installer) in [
            ("1.16.5", "36.2.42", r#"{"processors":[]}"#),
            ("1.12.2", "14.23.5.2864", r#"{"processors":[{}]}"#),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let game = dir.path().join("game");
            let patch = serde_json::json!({
                "inheritsFrom": minecraft,
                "mainClass": "example.Main",
                "libraries": [{"name": format!("net.minecraftforge:forge:{minecraft}-{version}")}],
            })
            .to_string();
            fixture(
                &game,
                &[
                    ("version.json", &patch),
                    ("install_profile.json", installer),
                ],
            );
            let paths = Paths {
                root: dir.path().join("launcher"),
            };
            let installed = install_staged(&paths, "Installer Forge", &game, minecraft).unwrap();
            let loader = crate::instance::loader_of(&installed);
            assert_eq!(
                loader,
                Loader::Forge {
                    version: version.into()
                }
            );
            let patch_path = installed.root.join(FORGE_PATCH);
            assert!(!patch_path.exists());
            fs::write(patch_path, patch).unwrap();
            let mut profile = LaunchProfile {
                minecraft_version: minecraft.into(),
                ..Default::default()
            };
            assert!(!apply_forge_patch(
                &mut profile,
                &installed.root,
                &installed.game_root(),
                &loader
            )
            .unwrap());
            assert!(profile.libraries.is_empty());
        }
    }

    #[test]
    fn native_only_forge_library_does_not_resolve_a_plain_jar() {
        let minecraft = "1.12.2";
        let loader = Loader::Forge {
            version: "14.23.5.2864".into(),
        };
        let classifier = format!("natives-{}", meta::os_name());
        let patch = serde_json::json!({
            "inheritsFrom": minecraft,
            "mainClass": "example.Main",
            "libraries": [
                {"name": "net.minecraftforge:forge:1.12.2-14.23.5.2864"},
                {"name": "org.example:native-only:1",
                    "url":"http://repo1.maven.org/maven2",
                    "natives": {(meta::os_name()): &classifier}},
                {"name": "org.example:with-plain:1",
                    "natives": {(meta::os_name()): &classifier},
                    "downloads": {
                        "artifact": {"url": "http://libraries.minecraft.net/with-plain.jar"},
                        "classifiers": {(&classifier): {"url": "http://maven.apache.org/native.jar"}}
                    }
                }
            ]
        }).to_string();
        let patch = forge_patch(&patch, minecraft, &loader).unwrap().unwrap();
        let mut profile = LaunchProfile::default();
        profile.apply_loader(&patch);
        assert_eq!(profile.libraries.len(), 2);
        assert_eq!(profile.libraries[1].spec, "org.example:with-plain:1");
        assert_eq!(
            profile.libraries[1].url.as_deref(),
            Some("https://libraries.minecraft.net/with-plain.jar")
        );
        assert_eq!(profile.natives.len(), 2);
        assert_eq!(profile.natives[0].url.as_deref(), Some(format!(
            "https://repo1.maven.org/maven2/org/example/native-only/1/native-only-1-{classifier}.jar"
        ).as_str()));
        assert_eq!(
            profile.natives[1].url.as_deref(),
            Some("https://maven.apache.org/native.jar")
        );
    }

    #[test]
    fn known_forge_library_hosts_use_https_but_custom_hosts_are_unchanged() {
        for host in [
            "files.minecraftforge.net",
            "maven.minecraftforge.net",
            "libraries.minecraft.net",
            "repo1.maven.org",
            "maven.apache.org",
        ] {
            let url = forge_library_url(&format!("http://{host}/repo/")).unwrap();
            assert_eq!(url.as_str(), format!("https://{host}/repo/"));
        }
        for host in ["repo.example", "repo1.maven.org.example"] {
            let url = format!("http://{host}/repo/");
            assert_eq!(forge_library_url(&url).unwrap().as_str(), url);
        }
    }

    #[test]
    fn embedded_forge_maven_jar_is_extracted_without_an_installer() {
        let dir = tempfile::tempdir().unwrap();
        let game = dir.path().join("game");
        let artifact = "net/minecraftforge/forge/1.12.2-14.23.5.2864/forge-1.12.2-14.23.5.2864.jar";
        fixture(
            &game,
            &[
                (
                    "version.json",
                    r#"{"inheritsFrom":"1.12.2","mainClass":"net.minecraft.launchwrapper.Launch","libraries":[{"name":"net.minecraftforge:forge:1.12.2-14.23.5.2864"}]}"#,
                ),
                (&format!("maven/{artifact}"), "bundled Forge jar"),
            ],
        );
        let paths = Paths {
            root: dir.path().join("launcher"),
        };
        let installed = install_staged(&paths, "Embedded Forge", &game, "1.12.2").unwrap();
        let loader = crate::instance::loader_of(&installed);
        let mut profile = LaunchProfile {
            minecraft_version: "1.12.2".into(),
            ..Default::default()
        };
        assert!(apply_forge_patch(
            &mut profile,
            &installed.root,
            &installed.game_root(),
            &loader
        )
        .unwrap());
        let library = &profile.libraries[0];
        assert_eq!(
            library.path,
            installed
                .root
                .canonicalize()
                .unwrap()
                .join("technic-libraries")
                .join(artifact)
        );
        assert_eq!(fs::read(&library.path).unwrap(), b"bundled Forge jar");
        assert_eq!(library.url, None);
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
        assert!(!game.exists());
        assert!(instance.game_root().join("bin/version.json").is_file());
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
    fn legacy_jar_with_bundled_forge_is_installed_without_a_second_loader() {
        let dir = tempfile::tempdir().unwrap();
        let game = dir.path().join("game");
        fixture(&game, &[
            ("net/minecraftforge/common/MinecraftForge.class", "fixture"),
            ("forgeversion.properties", "forge.major.number=7\nforge.minor.number=8\nforge.revision.number=1\nforge.build.number=738"),
        ]);
        let paths = Paths {
            root: dir.path().join("launcher"),
        };
        let instance = install_staged(&paths, "Legacy", &game, "1.4.7").unwrap();
        let profile = Pack::load(&instance.root).unwrap();
        assert_eq!(profile.version("net.minecraftforge"), None);
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
