use crate::{
    import::ImportError,
    ini::IniFile,
    net::{DownloadRequest, Downloader},
    profile::LaunchProfile,
    version_components::{ArchiveKind, Pack},
    Instance, InstanceList, Paths,
};
use md5::{Digest, Md5};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
};

pub const CDN: &str = "https://download.nodecdn.net/containers/atl/";
const OVERLAY: &str = ".prism-rs-atlauncher.json";
type Result<T> = std::result::Result<T, ImportError>;
fn error(s: impl Into<String>) -> ImportError {
    ImportError::Other(s.into())
}

#[derive(Clone, Debug, Deserialize)]
pub struct Release {
    pub version: String,
    pub minecraft: String,
}
#[derive(Clone, Debug, Deserialize)]
pub struct IndexedPack {
    pub name: String,
    #[serde(default, deserialize_with = "nullable_string")]
    pub description: String,
    pub versions: Vec<Release>,
    #[serde(default)]
    pub system: bool,
    #[serde(rename = "type")]
    pub kind: String,
}
pub fn safe_name(name: &str) -> String {
    name.chars().filter(char::is_ascii_alphanumeric).collect()
}
fn nullable_string<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<String, D::Error> {
    Ok(Option::<String>::deserialize(d)?.unwrap_or_default())
}
pub async fn list(dl: &Downloader) -> Result<Vec<IndexedPack>> {
    let mut packs: Vec<IndexedPack> = get(dl, &format!("{CDN}launcher/json/packsnew.json")).await?;
    packs.retain(|p| p.kind == "public" && !p.system && !p.versions.is_empty());
    Ok(packs)
}
async fn get<T: serde::de::DeserializeOwned>(dl: &Downloader, url: &str) -> Result<T> {
    dl.client()
        .get(url)
        .send()
        .await
        .map_err(crate::modplatform::PlatformError::from)?
        .error_for_status()
        .map_err(crate::modplatform::PlatformError::from)?
        .json()
        .await
        .map_err(crate::modplatform::PlatformError::from)
        .map_err(Into::into)
}
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Manifest {
    pub version: String,
    pub minecraft: String,
    pub no_configs: bool,
    pub loader: LoaderSpec,
    pub configs: Configs,
    pub mods: Vec<Mod>,
    pub libraries: Vec<Library>,
    pub main_class: MainClass,
    pub extra_arguments: ExtraArguments,
    pub messages: BTreeMap<String, String>,
    pub colours: BTreeMap<String, String>,
    pub warnings: BTreeMap<String, String>,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct LoaderSpec {
    #[serde(rename = "type")]
    pub kind: String,
    pub choose: bool,
    pub metadata: Value,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Configs {
    pub sha1: String,
    pub filesize: u64,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct MainClass {
    pub main_class: String,
    pub depends: String,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct ExtraArguments {
    pub arguments: String,
    pub depends: String,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Library {
    pub file: String,
    pub url: String,
    pub md5: String,
    pub download: String,
    pub server: String,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Mod {
    pub name: String,
    pub version: String,
    pub file: String,
    pub url: String,
    pub md5: String,
    pub download: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub client: bool,
    pub optional: bool,
    pub selected: bool,
    pub recommended: bool,
    pub hidden: bool,
    pub library: bool,
    pub group: String,
    pub depends: Vec<String>,
    pub description: String,
    pub colour: String,
    pub warning: String,
    pub extract_to: String,
    pub extract_folder: String,
    pub decomp_type: String,
    pub decomp_file: String,
}
pub async fn manifest(dl: &Downloader, name: &str, version: &str) -> Result<Manifest> {
    let base = pack_url(name, version)?;
    let m: Manifest = get(dl, &format!("{base}Configs.json")).await?;
    if m.minecraft.is_empty() || m.version.is_empty() {
        return Err(error("Pack manifest has no Minecraft/version"));
    }
    Ok(m)
}
fn pack_url(name: &str, version: &str) -> Result<String> {
    safe_path(version)?;
    if version.contains('/') || safe_name(name).is_empty() {
        return Err(error("Invalid pack/version"));
    }
    Ok(format!(
        "{CDN}packs/{}/versions/{version}/",
        safe_name(name)
    ))
}
pub fn safe_path(raw: &str) -> Result<PathBuf> {
    let path = Path::new(raw);
    if raw.is_empty()
        || raw.contains(['\\', ':', '\0'])
        || raw.split('/').any(|s| s == ".." || s == ".")
        || !path.components().all(|c| matches!(c, Component::Normal(_)))
    {
        return Err(error(format!("Unsafe pack path: {raw}")));
    }
    Ok(path.into())
}

#[derive(Clone, Debug, Default)]
pub struct Selection {
    pub enabled: BTreeSet<String>,
}
impl Selection {
    pub fn defaults(m: &Manifest) -> Result<Self> {
        let mut s = Self::default();
        for entry in m
            .mods
            .iter()
            .filter(|e| e.client && (!e.optional || e.selected))
        {
            s.set(m, &entry.name, true)?;
        }
        Ok(s)
    }
    pub fn recommended(m: &Manifest) -> Result<Self> {
        let mut s = Self::default();
        for entry in m
            .mods
            .iter()
            .filter(|e| e.client && (!e.optional || e.recommended))
        {
            s.set(m, &entry.name, true)?;
        }
        Ok(s)
    }
    pub fn clear(m: &Manifest) -> Result<Self> {
        let mut s = Self::default();
        for entry in m.mods.iter().filter(|e| e.client && !e.optional) {
            s.set(m, &entry.name, true)?;
        }
        Ok(s)
    }
    pub fn set(&mut self, m: &Manifest, name: &str, enable: bool) -> Result<()> {
        let mut next = self.clone();
        next.change(m, name, enable, &mut BTreeSet::new())?;
        if !enable
            && m.mods
                .iter()
                .any(|e| e.client && !e.optional && !next.enabled.contains(&e.name))
        {
            return Err(error("Cannot disable a required mod"));
        }
        *self = next;
        Ok(())
    }
    fn change(
        &mut self,
        m: &Manifest,
        name: &str,
        enable: bool,
        visiting: &mut BTreeSet<String>,
    ) -> Result<()> {
        let entry = m
            .mods
            .iter()
            .find(|e| e.name == name && e.client)
            .ok_or_else(|| error(format!("Unknown client dependency: {name}")))?;
        if !visiting.insert(name.into()) {
            return Ok(());
        }
        if enable {
            if !entry.group.is_empty() {
                for other in m
                    .mods
                    .iter()
                    .filter(|e| {
                        e.name != name && e.group == entry.group && self.enabled.contains(&e.name)
                    })
                    .cloned()
                    .collect::<Vec<_>>()
                {
                    self.change(m, &other.name, false, visiting)?;
                }
            }
            self.enabled.insert(name.into());
            for dep in &entry.depends {
                self.change(m, dep, true, visiting)?;
            }
        } else {
            self.enabled.remove(name);
            for other in m
                .mods
                .iter()
                .filter(|e| e.depends.iter().any(|d| d == name) && self.enabled.contains(&e.name))
                .cloned()
                .collect::<Vec<_>>()
            {
                self.change(m, &other.name, false, visiting)?;
            }
            for dep in &entry.depends {
                if m.mods
                    .iter()
                    .any(|e| &e.name == dep && (e.hidden || e.library))
                    && !m
                        .mods
                        .iter()
                        .any(|e| self.enabled.contains(&e.name) && e.depends.contains(dep))
                {
                    self.change(m, dep, false, visiting)?;
                }
            }
        }
        visiting.remove(name);
        Ok(())
    }
    pub fn validate(&self, m: &Manifest) -> Result<()> {
        let mut groups = BTreeSet::new();
        for e in m.mods.iter().filter(|e| e.client) {
            if !e.optional && !self.enabled.contains(&e.name) {
                return Err(error(format!("Missing required mod: {}", e.name)));
            }
            if self.enabled.contains(&e.name) {
                if !e.depends.iter().all(|d| self.enabled.contains(d)) {
                    return Err(error(format!("Missing dependency for {}", e.name)));
                }
                if !e.group.is_empty() && !groups.insert(&e.group) {
                    return Err(error(format!("Conflicting mod group: {}", e.group)));
                }
            }
        }
        Ok(())
    }
}

pub fn destination(kind: &str, mc: &str) -> Result<Option<PathBuf>> {
    Ok(Some(match kind {
        "root" | "mcpc" | "millenaire" => return Ok(None),
        "forge" | "jar" => "jarmods".into(),
        "mods" => "mods".into(),
        "flan" => "Flan".into(),
        "dependency" | "depandency" => Path::new("mods").join(safe_path(mc)?),
        "ic2lib" => "mods/ic2".into(),
        "denlib" => "mods/denlib".into(),
        "coremods" => "coremods".into(),
        "plugins" => "plugins".into(),
        "texturepack" => "texturepacks".into(),
        "resourcepack" => "resourcepacks".into(),
        "shaderpack" => "shaderpacks".into(),
        _ => return Err(error(format!("Unknown mod type: {kind}"))),
    }))
}
fn download_url(kind: &str, url: &str) -> Result<String> {
    match kind {
        "server" => {
            safe_path(url)?;
            Ok(format!("{CDN}{url}"))
        }
        "direct" => {
            let parsed = reqwest::Url::parse(url).map_err(|e| error(e.to_string()))?;
            if !matches!(parsed.scheme(), "http" | "https") {
                return Err(error("Unsupported download URL"));
            }
            Ok(url.into())
        }
        _ => Err(error(format!("Unsupported download type {kind}: {url}"))),
    }
}
async fn fetch_file(
    dl: &Downloader,
    url: String,
    path: &Path,
    md5: &str,
    sha1: &str,
) -> Result<()> {
    let mut req = DownloadRequest::new(url, path);
    req.skip_if_exists = false;
    if !sha1.is_empty() {
        req.sha1 = Some(sha1.into());
    }
    dl.download(&req).await?;
    if !md5.is_empty() && !format!("{:x}", Md5::digest(fs::read(path)?)).eq_ignore_ascii_case(md5) {
        return Err(error(format!("MD5 mismatch: {}", path.display())));
    }
    Ok(())
}
fn extract(archive: &Path, dest: &Path, folder: Option<&str>, member: Option<&str>) -> Result<()> {
    let mut zip =
        zip::ZipArchive::new(fs::File::open(archive)?).map_err(|e| error(e.to_string()))?;
    let folder = folder
        .filter(|s| !s.is_empty())
        .map(safe_path)
        .transpose()?;
    let member = member.map(safe_path).transpose()?;
    let mut found = member.is_none();
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| error(e.to_string()))?;
        let path = safe_path(entry.name().trim_end_matches('/'))?;
        if entry
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            return Err(error("Symlink in pack archive"));
        }
        if member.as_ref().is_some_and(|m| m != &path) {
            continue;
        }
        let relative = if let Some(folder) = &folder {
            match path.strip_prefix(folder) {
                Ok(p) if !p.as_os_str().is_empty() => p,
                _ => continue,
            }
        } else {
            &path
        };
        let out = dest.join(relative);
        if entry.is_dir() {
            fs::create_dir_all(out)?;
        } else {
            fs::create_dir_all(out.parent().unwrap())?;
            std::io::copy(&mut entry, &mut fs::File::create(out)?)?;
            found = true;
        }
    }
    if !found {
        return Err(error("Requested archive member is missing"));
    }
    Ok(())
}

async fn resolve_loader(dl: &Downloader, m: &Manifest) -> Result<crate::loaders::Loader> {
    use crate::loaders::Loader;
    let spec = &m.loader;
    let uid = match spec.kind.as_str() {
        "" => return Ok(Loader::None),
        "forge" => "net.minecraftforge",
        "neoforge" => "net.neoforged",
        "fabric" => "net.fabricmc.fabric-loader",
        _ => return Err(error(format!("Unsupported loader: {}", spec.kind))),
    };
    let recommended = spec.metadata["recommended"].as_bool().unwrap_or(false);
    let latest = spec.metadata["latest"].as_bool().unwrap_or(false);
    let version = if latest || recommended {
        let index: Value = get(
            dl,
            &format!("https://meta.prismlauncher.org/v1/{uid}/index.json"),
        )
        .await?;
        select_loader_version(&index, &spec.kind, &m.minecraft, recommended)?
    } else {
        spec.metadata[if spec.kind == "fabric" {
            "loader"
        } else {
            "version"
        }]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            error("No explicit loader version (choose-only manifests require a version)")
        })?
        .into()
    };
    safe_path(&version)?;
    if version.contains('/') {
        return Err(error("Unsafe loader version"));
    }
    Ok(match spec.kind.as_str() {
        "forge" => Loader::Forge { version },
        "neoforge" => Loader::NeoForge { version },
        _ => Loader::Fabric { version },
    })
}

fn select_loader_version(
    index: &Value,
    kind: &str,
    minecraft: &str,
    recommended: bool,
) -> Result<String> {
    index["versions"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|v| {
            (!recommended || v["recommended"].as_bool().unwrap_or(false))
                && (kind == "fabric"
                    || v["requires"].as_array().is_some_and(|r| {
                        r.iter()
                            .any(|r| r["uid"] == "net.minecraft" && r["equals"] == minecraft)
                    }))
        })
        .and_then(|v| v["version"].as_str())
        .map(str::to_owned)
        .ok_or_else(|| error("No matching loader release"))
}

struct Pending(Instance, bool);
impl Drop for Pending {
    fn drop(&mut self) {
        if !self.1 {
            let _ = fs::remove_dir_all(&self.0.root);
        }
    }
}

pub async fn install(
    paths: &Paths,
    dl: &Downloader,
    pack: &str,
    version: &str,
    name: &str,
    m: &Manifest,
    selection: &Selection,
) -> Result<Instance> {
    paths.ensure()?;
    safe_path(&m.minecraft)?;
    if m.version != version {
        return Err(error("Manifest version does not match requested version"));
    }
    selection.validate(m)?;
    let base = pack_url(pack, version)?;
    let mods: Vec<_> = m
        .mods
        .iter()
        .filter(|e| e.client && (!e.optional || selection.enabled.contains(&e.name)))
        .collect();
    let manual: Vec<_> = mods
        .iter()
        .filter(|e| e.download == "browser")
        .map(|e| format!("{}: {}", e.file, e.url))
        .collect();
    if !manual.is_empty() {
        return Err(error(format!(
            "Download these files manually (browser downloads unsupported):\n{}",
            manual.join("\n")
        )));
    }
    let mut pending = Pending(
        InstanceList::create(paths, name, "default", &m.minecraft)?,
        false,
    );
    let inst = &pending.0;
    let temp = tempfile::tempdir_in(paths.cache_dir())?;
    let game = inst.game_root();
    if !m.no_configs {
        let archive = temp.path().join("configs.zip");
        fetch_file(
            dl,
            format!("{base}Configs.zip"),
            &archive,
            "",
            &m.configs.sha1,
        )
        .await?;
        extract(&archive, &game, None, None)?;
    }
    let mut loader = resolve_loader(dl, m).await?;
    let mut archives = Vec::new();
    for (i, entry) in mods.iter().enumerate() {
        safe_path(&entry.file)?;
        let kind = if entry.kind == "jar" && entry.name == "Minecraft Forge" {
            "forge"
        } else {
            &entry.kind
        };
        if kind == "forge" {
            safe_path(&entry.version)?;
            if entry.version.contains('/') {
                return Err(error("Unsafe Forge version"));
            }
            let url = format!(
                "https://meta.prismlauncher.org/v1/net.minecraftforge/{}.json",
                entry.version
            );
            if get::<Value>(dl, &url).await.is_ok() {
                loader = crate::loaders::Loader::Forge {
                    version: entry.version.clone(),
                };
                continue;
            }
        }
        if matches!(kind, "root" | "mcpc" | "millenaire") {
            continue;
        }
        let file = temp.path().join(format!("mod-{i}.zip"));
        fetch_file(
            dl,
            download_url(&entry.download, &entry.url)?,
            &file,
            &entry.md5,
            "",
        )
        .await?;
        match kind {
            "extract" | "texturepackextract" | "resourcepackextract" => {
                let target = match kind {
                    "texturepackextract" => "texturepacks/extracted".into(),
                    "resourcepackextract" => "resourcepacks/extracted".into(),
                    _ => destination(&entry.extract_to, &m.minecraft)?.unwrap_or_default(),
                };
                let folder = entry.extract_folder.replace("%s%", "/");
                extract(
                    &file,
                    &game.join(target),
                    Some(folder.trim_start_matches('/')),
                    None,
                )?;
            }
            "decomp" => {
                let target = destination(&entry.decomp_type, &m.minecraft)?.unwrap_or_default();
                extract(&file, &game.join(target), None, Some(&entry.decomp_file))?;
            }
            _ => {
                if let Some(dir) = destination(kind, &m.minecraft)? {
                    let target = game.join(dir).join(safe_path(&entry.file)?);
                    fs::create_dir_all(target.parent().unwrap())?;
                    fs::copy(&file, &target)?;
                    if matches!(kind, "jar" | "forge") {
                        archives.push(target);
                    }
                }
            }
        }
    }
    crate::instance::set_loader(inst, &loader)?;
    for archive in archives {
        Pack::load(&inst.root)?.add_archive(&archive, ArchiveKind::JarMod)?;
    }
    let mut libraries = Vec::new();
    for (i, lib) in m.libraries.iter().enumerate() {
        safe_path(&lib.file)?;
        let path = inst.root.join("atl-libraries").join(format!("{i}.jar"));
        fetch_file(
            dl,
            download_url(&lib.download, &lib.url)?,
            &path,
            &lib.md5,
            "",
        )
        .await?;
        libraries.push(json!({"path":format!("atl-libraries/{i}.jar"),"name":library_name(lib)}));
    }
    let includes = |dep: &str| dep.is_empty() || mods.iter().any(|e| e.name == dep);
    let overlay = json!({"libraries":libraries, "mainClass":if includes(&m.main_class.depends) { &m.main_class.main_class } else { "" }, "extraArguments":if includes(&m.extra_arguments.depends) { &m.extra_arguments.arguments } else { "" }});
    fs::write(
        inst.root.join(OVERLAY),
        serde_json::to_vec_pretty(&overlay)?,
    )?;
    let mut cfg = IniFile::load(&inst.root.join("instance.cfg"))?;
    for (key, value) in [
        ("ManagedPack", "true".into()),
        ("ManagedPackType", "atlauncher".into()),
        ("ManagedPackID", safe_name(pack)),
        ("ManagedPackName", pack.into()),
        ("ManagedPackVersionID", version.into()),
        ("ManagedPackVersionName", version.into()),
    ] {
        cfg.set(key, value);
    }
    cfg.save(&inst.root.join("instance.cfg"))?;
    pending.1 = true;
    Ok(pending.0.clone())
}

pub fn apply_launch(root: &Path, profile: &mut LaunchProfile) -> std::io::Result<()> {
    let path = root.join(OVERLAY);
    if !path.exists() {
        return Ok(());
    }
    let overlay: Value = serde_json::from_slice(&fs::read(path)?)?;
    if let Some(main) = overlay["mainClass"].as_str().filter(|s| !s.is_empty()) {
        profile.main_class = main.into();
    }
    let mut args = crate::launch::split_args(overlay["extraArguments"].as_str().unwrap_or(""))
        .map_err(std::io::Error::other)?;
    let mut existing = profile
        .modern_arguments
        .as_ref()
        .map(|a| {
            a.game
                .iter()
                .filter_map(|a| match a {
                    crate::meta::ArgValue::Str(s) => Some(s.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    existing.extend(
        crate::launch::split_args(profile.legacy_arguments.as_deref().unwrap_or(""))
            .unwrap_or_default(),
    );
    let mut filtered = Vec::new();
    let mut iter = args.drain(..);
    while let Some(arg) = iter.next() {
        if arg == "--tweakClass" {
            if let Some(class) = iter.next() {
                if !existing
                    .iter()
                    .any(|s| s == &class || s == &format!("--tweakClass={class}"))
                {
                    filtered.extend([arg, class]);
                }
            }
        } else if !arg.starts_with("--tweakClass=")
            || !existing
                .iter()
                .any(|s| s == &arg || Some(s.as_str()) == arg.strip_prefix("--tweakClass="))
        {
            filtered.push(arg);
        }
    }
    let args = filtered;
    if !args.is_empty() {
        if let Some(modern) = profile.modern_arguments.as_mut() {
            modern
                .game
                .extend(args.into_iter().map(crate::meta::ArgValue::Str));
        } else {
            let base = profile.legacy_arguments.get_or_insert_with(String::new);
            for arg in args {
                base.push(' ');
                base.push('"');
                base.push_str(&arg.replace('\\', "\\\\").replace('"', "\\\""));
                base.push('"');
            }
        }
    }
    for value in overlay["libraries"].as_array().into_iter().flatten() {
        let p = safe_path(value["path"].as_str().unwrap_or("")).map_err(std::io::Error::other)?;
        let name = value["name"]
            .as_str()
            .ok_or_else(|| std::io::Error::other("Invalid ATLauncher library"))?;
        let identity = |s: &str| s.split(':').take(2).collect::<Vec<_>>().join(":");
        let id = identity(name);
        let version = |s: &str| {
            s.split(':')
                .nth(2)
                .unwrap_or("")
                .split(['.', '-', '_'])
                .map(|s| s.parse::<u64>().unwrap_or(0))
                .collect::<Vec<_>>()
        };
        if profile
            .libraries
            .iter()
            .any(|l| identity(&l.spec) == id && version(&l.spec) >= version(name))
        {
            continue;
        }
        profile.libraries.retain(|l| identity(&l.spec) != id);
        profile.libraries.push(crate::profile::ResolvedLibrary {
            spec: name.into(),
            path: root.join(p),
            url: None,
            sha1: None,
            native: false,
        });
    }
    Ok(())
}

fn library_name(lib: &Library) -> String {
    let parts: Vec<_> = lib.server.split('/').collect();
    if parts.len() >= 4 {
        let n = parts.len();
        return format!(
            "{}:{}:{}",
            parts[..n - 3].join("."),
            parts[n - 3],
            parts[n - 2]
        );
    }
    if let Some((name, version)) = lib.file.trim_end_matches(".jar").rsplit_once('-') {
        let group = match name {
            "guava" => Some("com.google.guava"),
            "commons-lang3" => Some("org.apache.commons"),
            _ => None,
        };
        if let Some(group) = group {
            return format!("{group}:{name}:{version}");
        }
    }
    format!("org.multimc.atlauncher:{}:1", lib.md5)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    fn fixture() -> Manifest {
        serde_json::from_value(json!({"version":"1", "minecraft":"1.20.1", "noConfigs":true,
            "loader":{"type":"fabric","metadata":{"loader":"0.16.0"}},
            "mods":[
                {"name":"a","client":true,"optional":true,"selected":true,"group":"choice","depends":["lib"]},
                {"name":"b","client":true,"optional":true,"group":"choice"},
                {"name":"lib","client":true,"optional":true,"library":true}
            ]})).unwrap()
    }
    #[test]
    fn manifest_parse() {
        let p: IndexedPack = serde_json::from_value(
            json!({"name":"Pack","type":"private","versions":[],"description":null}),
        )
        .unwrap();
        assert!(p.description.is_empty());
        let m = fixture();
        assert_eq!(m.minecraft, "1.20.1");
        assert!(m.no_configs);
        assert_eq!(m.loader.metadata["loader"], "0.16.0");
        assert!(serde_json::from_str::<Manifest>("{bad}").is_err());
        let empty: Manifest = serde_json::from_str("{}").unwrap();
        assert!(empty.mods.is_empty());
    }
    #[test]
    fn selection_dependencies_groups() {
        let m = fixture();
        let mut s = Selection::defaults(&m).unwrap();
        assert!(s.enabled.contains("lib"));
        s.set(&m, "b", true).unwrap();
        assert!(!s.enabled.contains("a"));
        assert!(!s.enabled.contains("lib"));
        s.validate(&m).unwrap();
        s.set(&m, "a", true).unwrap();
        s.set(&m, "lib", false).unwrap();
        assert!(!s.enabled.contains("a"));
        assert!(s.set(&m, "missing", true).is_err());
        assert!(Selection::clear(&Manifest::default())
            .unwrap()
            .enabled
            .is_empty());
    }
    #[test]
    fn rejects_unsafe_paths_and_archives() {
        for raw in [
            "../escape",
            "a/../b",
            "/abs",
            "a\\b",
            "C:/drive",
            ".",
            "a/./b",
            "",
        ] {
            assert!(safe_path(raw).is_err(), "{raw}");
        }
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("bad.zip");
        let mut zip = zip::ZipWriter::new(fs::File::create(&path).unwrap());
        zip.start_file("../escape", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"x").unwrap();
        zip.finish().unwrap();
        assert!(extract(&path, &temp.path().join("out"), None, None).is_err());
        assert!(!temp.path().join("escape").exists());
    }
    #[test]
    fn extract_and_decomp() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("good.zip");
        let mut zip = zip::ZipWriter::new(fs::File::create(&path).unwrap());
        zip.start_file("folder/a.txt", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"hello").unwrap();
        zip.finish().unwrap();
        let out = temp.path().join("out");
        extract(&path, &out, Some("folder"), None).unwrap();
        assert_eq!(fs::read(out.join("a.txt")).unwrap(), b"hello");
        extract(&path, &out, None, Some("folder/a.txt")).unwrap();
        assert!(out.join("folder/a.txt").exists());
        assert!(extract(&path, &out, None, Some("missing")).is_err());
    }
    #[tokio::test]
    async fn failed_install_cleanup() {
        let temp = tempfile::tempdir().unwrap();
        let paths = Paths {
            root: temp.path().into(),
        };
        let mut m = fixture();
        m.mods.clear();
        m.loader.kind = "unsupported".into();
        let result = install(
            &paths,
            &Downloader::new(),
            "Example",
            "1",
            "Example",
            &m,
            &Selection::default(),
        )
        .await;
        assert!(result.is_err());
        assert_eq!(fs::read_dir(paths.instances_dir()).unwrap().count(), 0);
    }
    #[tokio::test]
    async fn empty_install_and_launch_overlay() {
        let temp = tempfile::tempdir().unwrap();
        let paths = Paths {
            root: temp.path().into(),
        };
        let m = Manifest {
            version: "1".into(),
            minecraft: "1.20.1".into(),
            no_configs: true,
            main_class: MainClass {
                main_class: "example.Main".into(),
                depends: String::new(),
            },
            extra_arguments: ExtraArguments {
                arguments: "--tweakClass example.Tweak".into(),
                depends: String::new(),
            },
            ..Default::default()
        };
        let inst = install(
            &paths,
            &Downloader::new(),
            "Example",
            "1",
            "Example",
            &m,
            &Selection::default(),
        )
        .await
        .unwrap();
        let cfg = IniFile::load(&inst.root.join("instance.cfg")).unwrap();
        assert_eq!(cfg.get("ManagedPackType"), Some("atlauncher"));
        let mut profile = LaunchProfile::default();
        apply_launch(&inst.root, &mut profile).unwrap();
        assert_eq!(profile.main_class, "example.Main");
        assert!(profile.legacy_arguments.unwrap().contains("example.Tweak"));
    }
    #[tokio::test]
    async fn browser_error_and_client_filter() {
        let temp = tempfile::tempdir().unwrap();
        let paths = Paths {
            root: temp.path().into(),
        };
        let mut m = fixture();
        m.loader = LoaderSpec::default();
        m.mods = vec![Mod {
            name: "Manual".into(),
            file: "manual.jar".into(),
            url: "https://example.com/manual".into(),
            download: "browser".into(),
            client: true,
            ..Default::default()
        }];
        let selection = Selection::defaults(&m).unwrap();
        let err = install(
            &paths,
            &Downloader::new(),
            "Pack",
            "1",
            "Pack",
            &m,
            &selection,
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(err.contains("manual.jar: https://example.com/manual"));
        m.mods[0].client = false;
        assert!(install(
            &paths,
            &Downloader::new(),
            "Pack",
            "1",
            "Pack",
            &m,
            &Selection::default()
        )
        .await
        .is_ok());
    }
    #[tokio::test]
    async fn checksum_checks() {
        fn serve() -> String {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            std::thread::spawn(move || {
                use std::io::Read;
                let (mut socket, _) = listener.accept().unwrap();
                let mut buf = [0; 4096];
                let _ = socket.read(&mut buf);
                socket
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello",
                    )
                    .unwrap();
            });
            format!("http://{address}/file")
        }
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("file");
        let dl = Downloader::new();
        fetch_file(
            &dl,
            serve(),
            &path,
            "5d41402abc4b2a76b9719d911017c592",
            "aaf4c61ddcc5e8a2dabede0f3b482cd9aea9434d",
        )
        .await
        .unwrap();
        assert!(
            fetch_file(&dl, serve(), &path, "00000000000000000000000000000000", "")
                .await
                .unwrap_err()
                .to_string()
                .contains("MD5 mismatch")
        );
        assert!(fetch_file(
            &dl,
            serve(),
            &path,
            "",
            "0000000000000000000000000000000000000000"
        )
        .await
        .is_err());
    }
    #[test]
    fn destinations() {
        for (kind, path) in [
            ("mods", "mods"),
            ("jar", "jarmods"),
            ("forge", "jarmods"),
            ("flan", "Flan"),
            ("dependency", "mods/1.20.1"),
            ("depandency", "mods/1.20.1"),
            ("ic2lib", "mods/ic2"),
            ("denlib", "mods/denlib"),
            ("coremods", "coremods"),
            ("plugins", "plugins"),
            ("texturepack", "texturepacks"),
            ("resourcepack", "resourcepacks"),
            ("shaderpack", "shaderpacks"),
        ] {
            assert_eq!(
                destination(kind, "1.20.1").unwrap(),
                Some(PathBuf::from(path))
            );
        }
        assert_eq!(destination("root", "1").unwrap(), None);
        assert!(destination("unknown", "1").is_err());
    }
    #[tokio::test]
    async fn loaders_explicit_latest_recommended() {
        let dl = Downloader::new();
        for kind in ["forge", "neoforge", "fabric"] {
            let mut m = fixture();
            m.loader.kind = kind.into();
            m.loader.metadata = json!({"version":"1.2.3","loader":"1.2.3"});
            assert_eq!(
                resolve_loader(&dl, &m).await.unwrap().version(),
                Some("1.2.3")
            );
            m.loader.metadata = json!({"version":"../unsafe","loader":"../unsafe"});
            assert!(resolve_loader(&dl, &m).await.is_err());
        }
        let index = json!({"versions":[{"version":"new-wrong-mc","requires":[{"uid":"net.minecraft","equals":"1.21"}]},{"version":"latest","requires":[{"uid":"net.minecraft","equals":"1.20.1"}]},{"version":"recommended","recommended":true,"requires":[{"uid":"net.minecraft","equals":"1.20.1"}]}]});
        assert_eq!(
            select_loader_version(&index, "forge", "1.20.1", false).unwrap(),
            "latest"
        );
        assert_eq!(
            select_loader_version(&index, "neoforge", "1.20.1", true).unwrap(),
            "recommended"
        );
        assert_eq!(
            select_loader_version(&index, "fabric", "1.20.1", false).unwrap(),
            "new-wrong-mc"
        );
        assert!(select_loader_version(&json!({"versions":[]}), "forge", "1", true).is_err());
    }
}
