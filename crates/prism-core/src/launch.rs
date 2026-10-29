//! Launch pipeline: fetch version metadata, download game files, extract
//! natives, and build the java command line. Mirrors PrismLauncher's
//! `launch/` + `MinecraftInstance::createLaunchTask` (direct java exec
//! instead of the bundled launcher agent).

use crate::meta::{self, VersionData};
use crate::net::{DownloadRequest, Downloader, NetError};
use crate::profile::{LaunchProfile, ResolvedLibrary, DEFAULT_JVM_ARGUMENTS, LEGACY_GAME_ARGUMENTS};
use crate::Paths;
use md5::{Digest as Md5Digest, Md5};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const LAUNCHER_NAME: &str = "prism-rs";
pub const MSA_CLIENT_ID: &str = "c36a9fb6-4f2a-41ff-90bd-ae7cc92031eb";

/// Auth data handed to the launched game.
#[derive(Debug, Clone, Default)]
pub struct AuthSession {
    pub player_name: String,
    pub uuid: String,
    pub access_token: String,
    pub user_type: String,
    /// legacy `session` value, `token:{access}:{profile}`.
    pub session: String,
    pub xuid: String,
    pub client_id: String,
}

impl AuthSession {
    pub fn offline(player_name: &str) -> AuthSession {
        AuthSession {
            player_name: player_name.to_string(),
            uuid: offline_uuid(player_name),
            access_token: "0".to_string(),
            user_type: "legacy".to_string(),
            session: "token:0:0".to_string(),
            xuid: "0".to_string(),
            client_id: MSA_CLIENT_ID.to_string(),
        }
    }
}

/// Offline-mode UUID: UUID v3 of `OfflinePlayer:<name>` (Minecraft's scheme).
pub fn offline_uuid(player_name: &str) -> String {
    let mut hasher = Md5::new();
    hasher.update(format!("OfflinePlayer:{player_name}"));
    let digest = hasher.finalize();
    let mut b = [0u8; 16];
    b.copy_from_slice(&digest);
    b[6] = (b[6] & 0x0f) | 0x30; // version 3
    b[8] = (b[8] & 0x3f) | 0x80; // RFC 4122 variant
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7], b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]
    )
}

/// Everything needed to start one instance, gathered from global +
/// per-instance settings.
#[derive(Debug, Clone)]
pub struct LaunchSettings {
    pub instance_name: String,
    pub instance_id: String,
    pub game_dir: PathBuf,
    pub mc_version: String,
    pub java_path: PathBuf,
    pub min_memory_mb: u32,
    pub max_memory_mb: u32,
    pub extra_jvm_args: Vec<String>,
    pub window_width: u32,
    pub window_height: u32,
    pub env: Vec<(String, String)>,
}

impl LaunchSettings {
    pub fn for_instance(inst: &crate::instance::Instance, paths: &Paths) -> LaunchSettings {
        let global = crate::settings::Settings::load(&paths.global_config());
        LaunchSettings {
            instance_name: inst.name.clone(),
            instance_id: inst.id.clone(),
            game_dir: inst.game_root(),
            mc_version: default_version_for(inst),
            java_path: default_java_path(),
            min_memory_mb: global.get_int("MinMemAlloc", 512).max(128) as u32,
            max_memory_mb: global.get_int("MaxMemAlloc", 4096).max(256) as u32,
            extra_jvm_args: Vec::new(),
            window_width: global.get_int("MinecraftWinWidth", 854).max(320) as u32,
            window_height: global.get_int("MinecraftWinHeight", 480).max(240) as u32,
            env: Vec::new(),
        }
        .with_instance_cfg(inst)
        .with_paths(paths)
    }

    fn with_instance_cfg(mut self, inst: &crate::instance::Instance) -> Self {
        // Per-instance overrides stored in instance.cfg.
        let cfg = crate::ini::IniFile::load(&inst.root.join("instance.cfg")).unwrap_or_default();
        if let Some(v) = cfg.get("MinMemAlloc").and_then(|v| v.parse().ok()) {
            self.min_memory_mb = v;
        }
        if let Some(v) = cfg.get("MaxMemAlloc").and_then(|v| v.parse().ok()) {
            self.max_memory_mb = v;
        }
        if let Some(v) = cfg.get("MinecraftWinWidth").and_then(|v| v.parse().ok()) {
            self.window_width = v;
        }
        if let Some(v) = cfg.get("MinecraftWinHeight").and_then(|v| v.parse().ok()) {
            self.window_height = v;
        }
        if let Some(v) = cfg.get("OverrideJavaPath").or_else(|| cfg.get("JavaPath")) {
            if !v.is_empty() {
                self.java_path = PathBuf::from(v);
            }
        }
        if let Some(v) = cfg.get("jvmArgs") {
            if !v.is_empty() {
                self.extra_jvm_args = v.split_whitespace().map(String::from).collect();
            }
        }
        self
    }

    fn with_paths(mut self, paths: &Paths) -> Self {
        if !self.java_path.exists() {
            if let Some(java) = crate::java::find_java() {
                self.java_path = java;
            }
        }
        let _ = paths;
        self
    }
}

fn default_version_for(inst: &crate::instance::Instance) -> String {
    // Read from mmc-pack.json.
    let pack_path = inst.root.join("mmc-pack.json");
    if let Ok(text) = std::fs::read_to_string(&pack_path) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
            for comp in v["components"].as_array().unwrap_or(&vec![]) {
                if comp["uid"] == "net.minecraft" {
                    return comp["version"].as_str().unwrap_or("1.20.1").to_string();
                }
            }
        }
    }
    "1.20.1".to_string()
}

fn default_java_path() -> PathBuf {
    crate::java::find_java().unwrap_or_else(|| PathBuf::from("java"))
}

#[derive(Debug, thiserror::Error)]
pub enum LaunchError {
    #[error("network: {0}")]
    Net(#[from] NetError),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("version {0} not found in manifest")]
    VersionNotFound(String),
    #[error("zip: {0}")]
    Zip(String),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}

/// A fully prepared launch: resolved command line, ready to spawn.
#[derive(Debug, Clone)]
pub struct PreparedLaunch {
    pub command: Vec<String>,
    pub working_dir: PathBuf,
    pub env: Vec<(String, String)>,
}

impl PreparedLaunch {
    pub fn spawn(&self) -> std::io::Result<std::process::Child> {
        use std::process::{Command, Stdio};
        let mut cmd = Command::new(&self.command[0]);
        cmd.args(&self.command[1..])
            .current_dir(&self.working_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (k, v) in &self.env {
            cmd.env(k, v);
        }
        cmd.spawn()
    }
}

pub struct Launcher {
    pub paths: Paths,
    pub downloader: Arc<Downloader>,
}

impl Launcher {
    pub fn new(paths: Paths, downloader: Arc<Downloader>) -> Launcher {
        Launcher { paths, downloader }
    }

    /// Fetch and cache the Mojang version JSON for one version id.
    pub async fn fetch_version_data(&self, version_id: &str) -> Result<VersionData, LaunchError> {
        // Cached?
        let cache_path = self.paths.metadata_dir().join("net.minecraft").join(format!("{version_id}.json"));
        if let Ok(text) = tokio::fs::read_to_string(&cache_path).await {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
                if let Ok(vd) = VersionData::from_json(&json) {
                    return Ok(vd);
                }
            }
        }

        let manifest = meta::VersionManifest::fetch(self.downloader.client()).await?;
        let entry = manifest
            .find(version_id)
            .ok_or_else(|| LaunchError::VersionNotFound(version_id.to_string()))?;
        let json = self.downloader.get_json(&entry.url).await?;
        let vd = VersionData::from_json(&json)?;
        if let Some(parent) = cache_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let _ = tokio::fs::write(&cache_path, serde_json::to_vec(&json).unwrap_or_default()).await;
        // Keep the client-jar download URL alongside the cached metadata.
        if let Some(url) = json["downloads"]["client"]["url"].as_str() {
            let url_path = self
                .paths
                .metadata_dir()
                .join("net.minecraft")
                .join(format!("{version_id}.client-url"));
            let _ = tokio::fs::write(&url_path, url).await;
        }
        Ok(vd)
    }

    /// Download everything the profile needs, then extract natives.
    pub async fn prepare_files(&self, profile: &LaunchProfile, settings: &LaunchSettings) -> Result<(), LaunchError> {
        let libraries_dir = self.paths.libraries_dir();

        // Client jar.
        let client_jar = self.client_jar_path(&profile.minecraft_version);
        if !client_jar.exists() {
            let url = self
                .cached_client_url(&profile.minecraft_version)
                .await
                .unwrap_or_else(|| {
                    format!(
                        "https://s3.amazonaws.com/Minecraft.Download/versions/{0}/{0}.jar",
                        profile.minecraft_version
                    )
                });
            self.downloader
                .download(&DownloadRequest::new(url, &client_jar))
                .await?;
        }

        // Libraries.
        let mut requests = Vec::new();
        for lib in profile.libraries.iter().chain(profile.natives.iter()) {
            let dest = libraries_dir.join(&lib.path);
            if dest.exists() {
                continue;
            }
            if let Some(url) = &lib.url {
                let mut req = DownloadRequest::new(url.clone(), dest);
                if let Some(sha1) = &lib.sha1 {
                    req = req.with_sha1(sha1.clone());
                }
                requests.push(req);
            }
        }
        self.downloader
            .download_all(requests, 8, |_| {})
            .await?;

        // Asset index + objects.
        if let Some((index_url, index_sha1)) = &profile.asset_index {
            let index_path = self
                .paths
                .assets_dir()
                .join("indexes")
                .join(format!("{}.json", profile.assets_id));
            self.downloader
                .download(&{
                    let mut r = DownloadRequest::new(index_url.clone(), &index_path);
                    if let Some(s) = index_sha1 {
                        r = r.with_sha1(s.clone());
                    }
                    r
                })
                .await?;
            let index_text = tokio::fs::read_to_string(&index_path).await?;
            let index: serde_json::Value = serde_json::from_str(&index_text)?;
            let mut asset_requests = Vec::new();
            if let Some(objects) = index["objects"].as_object() {
                for (_name, obj) in objects {
                    let hash = obj["hash"].as_str().unwrap_or_default().to_string();
                    if hash.len() < 4 {
                        continue;
                    }
                    let dest = self
                        .paths
                        .assets_dir()
                        .join("objects")
                        .join(&hash[..2])
                        .join(&hash);
                    if dest.exists() {
                        continue;
                    }
                    asset_requests.push(DownloadRequest::new(
                        format!("https://resources.download.minecraft.net/{}/{}", &hash[..2], hash),
                        dest,
                    ));
                }
            }
            // Assets are big; tolerate individual failures by re-checking at
            // launch time in offline mode.
            let _ = self.downloader.download_all(asset_requests, 16, |_| {}).await;
        }

        // Extract natives.
        let natives_dir = settings.game_dir.join("bin");
        tokio::fs::create_dir_all(&natives_dir).await?;
        for native in &profile.natives {
            let jar = libraries_dir.join(&native.path);
            if jar.exists() {
                extract_natives(&jar, &natives_dir)?;
            }
        }
        Ok(())
    }

    pub fn client_jar_path(&self, version: &str) -> PathBuf {
        self.paths.cache_dir().join("versions").join(version).join(format!("{version}.jar"))
    }

    fn version_data_cache(&self, version: &str) -> Option<VersionData> {
        let p = self.paths.metadata_dir().join("net.minecraft").join(format!("{version}.json"));
        let text = std::fs::read_to_string(p).ok()?;
        VersionData::from_json(&serde_json::from_str(&text).ok()?).ok()
    }

    async fn cached_client_url(&self, version: &str) -> Option<String> {
        let p = self.paths.metadata_dir().join("net.minecraft").join(format!("{version}.client-url"));
        tokio::fs::read_to_string(p).await.ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
    }

    /// Full prepare: version data + files + command line.
    pub async fn prepare(
        &self,
        settings: &LaunchSettings,
        session: Option<&AuthSession>,
        profile: &LaunchProfile,
    ) -> Result<PreparedLaunch, LaunchError> {
        self.prepare_files(profile, settings).await?;
        Ok(self.build_command(settings, session, profile))
    }

    pub fn build_command(
        &self,
        settings: &LaunchSettings,
        session: Option<&AuthSession>,
        profile: &LaunchProfile,
    ) -> PreparedLaunch {
        let libraries_dir = self.paths.libraries_dir();
        let natives_dir = settings.game_dir.join("bin");
        let client_jar = self.client_jar_path(&profile.minecraft_version);

        // Classpath entries: prefer libraries installed by loader installers
        // inside the instance (game_dir/libraries), then the shared cache.
        let mut classpath: Vec<String> = profile
            .classpath()
            .iter()
            .map(|p| {
                let in_instance = settings.game_dir.join("libraries").join(p);
                if in_instance.exists() {
                    in_instance.to_string_lossy().to_string()
                } else {
                    libraries_dir.join(p).to_string_lossy().to_string()
                }
            })
            .collect();
        classpath.push(client_jar.to_string_lossy().to_string());
        let classpath_str = classpath.join(if cfg!(windows) { ";" } else { ":" });

        let assets_root = self.paths.assets_dir();
        let game_assets = assets_root.join("virtual").join(&profile.assets_id);

        let mut tokens: std::collections::BTreeMap<String, String> = Default::default();
        tokens.insert("version_name".into(), profile.minecraft_version.clone());
        tokens.insert("version_type".into(), profile.version_type.clone().unwrap_or_default());
        tokens.insert("game_directory".into(), settings.game_dir.to_string_lossy().to_string());
        tokens.insert("assets_root".into(), assets_root.to_string_lossy().to_string());
        tokens.insert("assets_index_name".into(), profile.assets_id.clone());
        tokens.insert(
            "game_assets".into(),
            if profile.is_legacy() {
                game_assets.to_string_lossy().to_string()
            } else {
                assets_root.join("virtual/legacy").to_string_lossy().to_string()
            },
        );
        tokens.insert("library_directory".into(), libraries_dir.to_string_lossy().to_string());
        tokens.insert(
            "classpath_separator".into(),
            if cfg!(windows) { ";" } else { ":" }.to_string(),
        );
        tokens.insert("natives_directory".into(), natives_dir.to_string_lossy().to_string());
        tokens.insert("launcher_name".into(), LAUNCHER_NAME.to_string());
        tokens.insert("launcher_version".into(), env!("CARGO_PKG_VERSION").to_string());
        tokens.insert("user_properties".into(), "{}".to_string());
        if let Some(session) = session {
            tokens.insert("auth_player_name".into(), session.player_name.clone());
            tokens.insert("auth_uuid".into(), session.uuid.clone());
            tokens.insert("auth_access_token".into(), session.access_token.clone());
            tokens.insert("auth_session".into(), session.session.clone());
            tokens.insert("user_type".into(), session.user_type.clone());
            tokens.insert("auth_xuid".into(), session.xuid.clone());
            tokens.insert("clientId".into(), session.client_id.clone());
        } else {
            tokens.insert("auth_player_name".into(), "Player".to_string());
            tokens.insert("auth_uuid".into(), offline_uuid("Player"));
            tokens.insert("auth_access_token".into(), "0".to_string());
            tokens.insert("auth_session".into(), "token:0:0".to_string());
            tokens.insert("user_type".into(), "legacy".to_string());
            tokens.insert("auth_xuid".into(), "0".to_string());
            tokens.insert("clientId".into(), MSA_CLIENT_ID.to_string());
        }
        // Mojang manifests use both spellings of the client id token.
        tokens.insert("classpath".into(), classpath_str.clone());
        tokens.insert("clientid".into(), tokens["clientId"].clone());

        let substitute = |arg: &str| -> String {
            let mut out = arg.to_string();
            for (k, v) in &tokens {
                out = out.replace(&format!("${{{k}}}"), v);
            }
            out
        };

        let mut command: Vec<String> = vec![settings.java_path.to_string_lossy().to_string()];

        // Memory.
        let (min, max) = if settings.min_memory_mb < settings.max_memory_mb {
            (settings.min_memory_mb, settings.max_memory_mb)
        } else {
            (settings.max_memory_mb, settings.min_memory_mb)
        };
        command.push(format!("-Xms{min}m"));
        command.push(format!("-Xmx{max}m"));
        command.push("-Duser.language=en".to_string());
        command.extend(settings.extra_jvm_args.iter().cloned());

        // Structured JVM args (1.13+) or plain library path (legacy).
        let mut jvm_done_library_path = false;
        if let Some(args) = &profile.modern_arguments {
            let features = Default::default();
            for arg in &args.jvm {
                match arg {
                    meta::ArgValue::Str(s) => command.push(substitute(s)),
                    meta::ArgValue::Conditional { value, rules } => {
                        if meta::rules_allow(rules, &features) {
                            for v in value {
                                command.push(substitute(v));
                            }
                        }
                    }
                }
            }
            jvm_done_library_path = command.iter().any(|a| a.starts_with("-Djava.library.path"));
        }
        if !jvm_done_library_path {
            command.push(format!("-Djava.library.path={}", natives_dir.to_string_lossy()));
        }

        // Classpath + main class.
        command.push("-cp".to_string());
        command.push(classpath_str);
        command.push(profile.main_class.clone());

        // Game arguments.
        if let Some(args) = &profile.modern_arguments {
            let features = Default::default();
            for arg in &args.game {
                match arg {
                    meta::ArgValue::Str(s) => command.push(substitute(s)),
                    meta::ArgValue::Conditional { value, rules } => {
                        if meta::rules_allow(rules, &features) {
                            for v in value {
                                command.push(substitute(v));
                            }
                        }
                    }
                }
            }
        } else {
            let legacy = profile.legacy_arguments.as_deref().unwrap_or(LEGACY_GAME_ARGUMENTS);
            for arg in legacy.split_whitespace() {
                command.push(substitute(arg));
            }
        }

        let mut env = settings.env.clone();
        env.push(("NO_COLOR".to_string(), "1".to_string()));

        PreparedLaunch {
            command,
            working_dir: settings.game_dir.clone(),
            env,
        }
    }
}

/// Extract a natives jar into the target dir, skipping META-INF and
/// non-native files (mirrors ExtractNatives.cpp).
pub fn extract_natives(jar: &Path, dest: &Path) -> Result<(), LaunchError> {
    let file = std::fs::File::open(jar).map_err(|e| LaunchError::Io(e))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| LaunchError::Zip(e.to_string()))?;
    std::fs::create_dir_all(dest).map_err(|e| LaunchError::Io(e))?;
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| LaunchError::Zip(e.to_string()))?;
        let name = entry.name().to_string();
        if name.starts_with("META-INF") || name.ends_with('/') {
            continue;
        }
        // Only extract files that look like natives (so/dll/dylib/jnilib) or
        // have an explicit git-lfs-free plain layout.
        let looks_native = name.ends_with(".so")
            || name.ends_with(".dll")
            || name.ends_with(".dylib")
            || name.ends_with(".jnilib")
            || name.ends_with(".dbg");
        if !looks_native {
            continue;
        }
        let safe_path = match PathBuf::from(&name)
            .components()
            .filter(|c| matches!(c, std::path::Component::Normal(_)))
            .collect::<std::path::PathBuf>()
            .file_name()
            .map(|f| dest.join(f))
        {
            Some(p) => p,
            None => continue,
        };
        let mut out = std::fs::File::create(&safe_path).map_err(|e| LaunchError::Io(e))?;
        std::io::copy(&mut entry, &mut out).map_err(|e| LaunchError::Io(e))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&safe_path, std::fs::Permissions::from_mode(0o755));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offline_uuid_format() {
        let uuid = offline_uuid("Notch");
        // UUID v3 of OfflinePlayer:Notch is a well-known constant.
        assert_eq!(uuid, "b50ad385-829d-3141-a216-7e7d7539ba7f");
        assert_eq!(offline_uuid("Player").len(), 36);
    }
}
