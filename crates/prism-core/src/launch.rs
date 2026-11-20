//! Launch pipeline: fetch version metadata, download game files, extract
//! natives, and build the java command line. Mirrors PrismLauncher's
//! `launch/` + `MinecraftInstance::createLaunchTask` (direct java exec
//! instead of the bundled launcher agent).

use crate::meta::{self, VersionData};
use crate::loaders::{self, Loader};
use crate::net::{DownloadRequest, Downloader, NetError};
use crate::profile::{LaunchProfile, ResolvedLibrary, LEGACY_GAME_ARGUMENTS};
use crate::version_components::{self, ArchiveKind, Pack};
use crate::Paths;
use md5::{Digest as Md5Digest, Md5};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const LAUNCHER_NAME: &str = "prism-rs";
pub const MSA_CLIENT_ID: &str = "c36a9fb6-4f2a-41ff-90bd-ae7cc92031eb";

/// A world or server to join immediately after Minecraft starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuickPlayTarget {
    Singleplayer { world: String },
    Multiplayer { host: String, port: u16 },
}

impl QuickPlayTarget {
    /// Parse a server address, using Minecraft's default port when omitted.
    pub fn parse_server(address: &str) -> Result<Self, String> {
        let address = address.trim();
        let (host, port) = if let Some(rest) = address.strip_prefix('[') {
            let (host, suffix) = rest.split_once(']').ok_or("Missing closing bracket")?;
            let port = if suffix.is_empty() {
                None
            } else {
                Some(suffix.strip_prefix(':').ok_or("Invalid server address")?)
            };
            (host, port)
        } else if address.matches(':').count() == 1 {
            let (host, port) = address.split_once(':').unwrap();
            (host, Some(port))
        } else {
            (address, None)
        };
        if host.is_empty() || host.chars().any(char::is_whitespace) || host.contains(['[', ']']) {
            return Err("Invalid or empty server host".into());
        }
        let port = match port {
            Some(port) => port.parse::<u16>().map_err(|_| "Invalid server port")?,
            None => 25565,
        };
        Ok(Self::Multiplayer {
            host: host.into(),
            port,
        })
    }

    pub fn address(&self) -> String {
        match self {
            Self::Multiplayer { host, port } if host.contains(':') => format!("[{host}]:{port}"),
            Self::Multiplayer { host, port } => format!("{host}:{port}"),
            Self::Singleplayer { world } => world.clone(),
        }
    }
}

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
    pub quick_play: Option<QuickPlayTarget>,
    pub instance_name: String,
    pub instance_id: String,
    pub game_dir: PathBuf,
    pub mc_version: String,
    pub java_path: PathBuf,
    /// `java_path` came from the user's settings, so launch verifies it
    /// instead of picking a Java automatically.
    pub java_path_explicit: bool,
    /// Download Mojang's Java runtime when no compatible install is found.
    pub auto_java_download: bool,
    /// Launch with an explicit Java even when its major version is wrong.
    pub ignore_java_compatibility: bool,
    pub min_memory_mb: u32,
    pub max_memory_mb: u32,
    pub extra_jvm_args: Vec<String>,
    pub raw_jvm_args: String,
    pub window_width: u32,
    pub window_height: u32,
    pub env: Vec<(String, String)>,
    pub instance_dir: PathBuf,
    pub pre_launch_command: String,
    pub wrapper_command: String,
    pub post_exit_command: String,
}

impl LaunchSettings {
    pub fn for_instance(inst: &crate::instance::Instance, paths: &Paths) -> LaunchSettings {
        let global = crate::settings::Settings::load(&paths.global_config());
        LaunchSettings {
            quick_play: None,
            instance_name: inst.name.clone(),
            instance_id: inst.id.clone(),
            game_dir: inst.game_root(),
            mc_version: default_version_for(inst),
            java_path: default_java_path(),
            java_path_explicit: false,
            auto_java_download: global.get_bool("AutomaticJavaDownload", true),
            ignore_java_compatibility: global.get_bool("IgnoreJavaCompatibility", false),
            min_memory_mb: global.get_int("MinMemAlloc", 512).max(128) as u32,
            max_memory_mb: global.get_int("MaxMemAlloc", 4096).max(256) as u32,
            extra_jvm_args: Vec::new(),
            raw_jvm_args: String::new(),
            window_width: global.get_int("MinecraftWinWidth", 854).max(320) as u32,
            window_height: global.get_int("MinecraftWinHeight", 480).max(240) as u32,
            env: Vec::new(),
            instance_dir: inst.root.clone(),
            pre_launch_command: String::new(),
            wrapper_command: String::new(),
            post_exit_command: String::new(),
        }
        .with_instance_cfg(inst, &global)
        .with_paths(paths)
    }

    fn with_instance_cfg(
        mut self,
        inst: &crate::instance::Instance,
        global: &crate::settings::Settings,
    ) -> Self {
        let cfg = crate::ini::IniFile::load(&inst.root.join("instance.cfg")).unwrap_or_default();
        let choose = |flag: &str, key: &str, legacy: Option<&str>| -> Option<String> {
            let local = || cfg.get(key).or_else(|| legacy.and_then(|k| cfg.get(k)));
            match cfg.get(flag) {
                Some("true" | "1") => local().map(str::to_owned),
                Some(_) => global.get(key).map(str::to_owned),
                None => local().or_else(|| global.get(key)).map(str::to_owned),
            }
        };
        let number = |flag: &str, key: &str, default, inherited| {
            let local = cfg.get(key).and_then(|v| v.parse::<u32>().ok());
            match cfg.get(flag) {
                Some("true" | "1") => local.unwrap_or(default),
                Some(_) => inherited,
                None => local.unwrap_or(inherited),
            }
        };
        self.min_memory_mb = number("OverrideMemory", "MinMemAlloc", 512, self.min_memory_mb);
        self.max_memory_mb = number("OverrideMemory", "MaxMemAlloc", 4096, self.max_memory_mb);
        self.window_width = number("OverrideWindow", "MinecraftWinWidth", 854, self.window_width);
        self.window_height = number("OverrideWindow", "MinecraftWinHeight", 480, self.window_height);
        let local_java = if cfg.get("OverrideJavaLocation").is_none() {
            cfg.get("OverrideJavaPath")
                .or_else(|| cfg.get("JavaPath"))
        } else {
            cfg.get("JavaPath")
        };
        // Imported automatic picks must not pin a stale Java installation.
        let local_java = local_java.filter(|v| !v.is_empty() && cfg.get("AutomaticJava") != Some("true"));
        let global_java = global.get("JavaPath").filter(|v| !v.is_empty());
        let java = match cfg.get("OverrideJavaLocation") {
            Some("true" | "1") => local_java,
            Some(_) => global_java,
            None => local_java.or(global_java),
        };
        if let Some(java) = java {
            self.java_path = java.into();
            self.java_path_explicit = true;
        }
        if let Some(v) = cfg.get("IgnoreJavaCompatibility") {
            self.ignore_java_compatibility = v == "true" || v == "1";
        }
        self.raw_jvm_args = choose("OverrideJavaArgs", "JvmArgs", Some("jvmArgs")).unwrap_or_default();
        self.extra_jvm_args = split_args(&self.raw_jvm_args).unwrap_or_default();
        self.pre_launch_command =
            choose("OverrideCommands", "PreLaunchCommand", None).unwrap_or_default();
        self.wrapper_command =
            choose("OverrideCommands", "WrapperCommand", None).unwrap_or_default();
        self.post_exit_command =
            choose("OverrideCommands", "PostExitCommand", None).unwrap_or_default();
        self.env = choose("OverrideEnv", "Env", None)
            .and_then(|v| serde_json::from_str::<std::collections::BTreeMap<String, String>>(&v).ok())
            .unwrap_or_default()
            .into_iter()
            .collect();
        self
    }

    fn with_paths(mut self, paths: &Paths) -> Self {
        if !self.java_path.exists() {
            self.java_path_explicit = false;
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
                    return comp["version"]
                        .as_str()
                        .or_else(|| comp["cachedVersion"].as_str())
                        .unwrap_or("1.20.1")
                        .to_string();
                }
            }
        }
    }
    "1.20.1".to_string()
}

/// PrismLauncher splitArgs rules: backslashes are literal outside quotes and
/// escape only the matching quote or another backslash inside quotes.
pub fn split_args(input: &str) -> Result<Vec<String>, String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if let Some(delimiter) = quote {
            if ch == '\\' && chars.peek().is_some_and(|next| *next == delimiter || *next == '\\') {
                current.push(chars.next().unwrap());
            } else if ch == delimiter {
                quote = None;
            } else {
                current.push(ch);
            }
        } else if ch == '\'' || ch == '"' {
            quote = Some(ch);
        } else if ch.is_whitespace() {
            if !current.is_empty() {
                args.push(std::mem::take(&mut current));
            }
        } else {
            current.push(ch);
        }
    }
    if let Some(delimiter) = quote {
        return Err(format!("Unterminated {delimiter} quote"));
    }
    if !current.is_empty() {
        args.push(current);
    }
    Ok(args)
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
    #[error("Minecraft {mc} needs Java {}, but {} is Java {found}", join_majors(required), path.display())]
    IncompatibleJava { mc: String, path: PathBuf, found: u32, required: Vec<u32> },
    #[error("java download: {0}")]
    JavaDownload(#[from] crate::java::JavaDownloadError),
    #[error("could not prepare {loader:?}: {message}")]
    Loader { loader: Loader, message: String },
}

fn join_majors(majors: &[u32]) -> String {
    majors.iter().map(u32::to_string).collect::<Vec<_>>().join(" or ")
}

/// A fully prepared launch: resolved command line, ready to spawn.
#[derive(Debug, Clone)]
pub struct PreparedLaunch {
    pub command: Vec<String>,
    pub working_dir: PathBuf,
    pub env: Vec<(String, String)>,
    pub instance_dir: PathBuf,
    pub pre_launch_command: String,
    pub post_exit_command: String,
    pub command_error: Option<String>,
}

pub type HookOutput = Box<dyn FnMut(String)>;

impl PreparedLaunch {
    fn hook(&self, script: &str, label: &'static str, output: Option<HookOutput>) -> std::io::Result<()> {
        use std::process::Stdio;

        if script.trim().is_empty() {
            return Ok(());
        }
        let mut command = if cfg!(windows) {
            let mut command = std::process::Command::new("cmd");
            command.args(["/C", script]);
            command
        } else {
            let mut command = std::process::Command::new("/bin/sh");
            command.args(["-c", script]);
            command
        };
        command
            .current_dir(&self.instance_dir)
            .envs(self.env.iter().cloned())
            .stdin(Stdio::null());
        let status = if let Some(mut output) = output {
            let captured = command.output()?;
            for (stream, bytes) in [("stdout", &captured.stdout), ("stderr", &captured.stderr)] {
                for line in String::from_utf8_lossy(bytes).lines() {
                    output(format!("{label} ({stream}): {line}"));
                }
            }
            captured.status
        } else {
            command.status()?
        };
        if !status.success() {
            return Err(std::io::Error::other(format!(
                "Hook command failed ({status}): {script}"
            )));
        }
        Ok(())
    }

    /// Call after waiting for the game, including unsuccessful game exits.
    pub fn run_post_exit(&self) -> std::io::Result<()> {
        self.hook(&self.post_exit_command, "Post-exit", None)
    }

    pub fn run_post_exit_with_output(&self, output: HookOutput) -> std::io::Result<()> {
        self.hook(&self.post_exit_command, "Post-exit", Some(output))
    }

    /// POSIX script with the same environment, wrapper and hooks as spawn().
    pub fn shell_script(&self) -> std::io::Result<String> {
        if let Some(error) = &self.command_error {
            return Err(std::io::Error::other(error.clone()));
        }
        let mut script = String::from("#!/bin/sh\n");
        script.push_str(&format!(
            "cd {} || exit $?\n",
            shell_quote(&self.instance_dir.to_string_lossy())
        ));
        // env accepts names that are not shell identifiers.
        let environment = self
            .env
            .iter()
            .map(|(k, v)| shell_quote(&format!("{k}={v}")))
            .collect::<Vec<_>>()
            .join(" ");
        let prefix = format!("env {environment}");
        if !self.pre_launch_command.trim().is_empty() {
            script.push_str(&format!(
                "{prefix} /bin/sh -c {} || exit $?\n",
                shell_quote(&self.pre_launch_command)
            ));
        }
        script.push_str(&format!(
            "cd {} || exit $?\n",
            shell_quote(&self.working_dir.to_string_lossy())
        ));
        let command = self.command.iter().map(|v| shell_quote(v)).collect::<Vec<_>>().join(" ");
        script.push_str(&format!("{prefix} {command}\nstatus=$?\n"));
        if !self.post_exit_command.trim().is_empty() {
            script.push_str(&format!(
                "(cd {} && {prefix} /bin/sh -c {}) || exit $?\n",
                shell_quote(&self.instance_dir.to_string_lossy()),
                shell_quote(&self.post_exit_command)
            ));
        }
        script.push_str("exit \"$status\"\n");
        Ok(script)
    }

    pub fn spawn(&self) -> std::io::Result<std::process::Child> {
        self.spawn_with(None, std::process::Command::spawn)
    }

    pub fn spawn_with_output(
        &self,
        output: HookOutput,
    ) -> std::io::Result<std::process::Child> {
        self.spawn_with(Some(output), std::process::Command::spawn)
    }

    pub fn spawn_with(
        &self,
        output: Option<HookOutput>,
        spawn: impl FnOnce(&mut std::process::Command) -> std::io::Result<std::process::Child>,
    ) -> std::io::Result<std::process::Child> {
        use std::process::{Command, Stdio};
        if let Some(error) = &self.command_error {
            return Err(std::io::Error::other(error.clone()));
        }
        self.hook(&self.pre_launch_command, "Pre-launch", output)?;
        let mut cmd = Command::new(&self.command[0]);
        cmd.args(&self.command[1..])
            .current_dir(&self.working_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (k, v) in &self.env {
            cmd.env(k, v);
        }
        spawn(&mut cmd)
    }
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub struct Launcher {
    pub paths: Paths,
    pub downloader: Arc<Downloader>,
}

impl Launcher {
    pub fn new(paths: Paths, downloader: Arc<Downloader>) -> Launcher {
        Launcher { paths, downloader }
    }

    pub async fn component_document(
        &self,
        uid: &str,
        version: &str,
    ) -> Result<serde_json::Value, LaunchError> {
        if uid != version_components::MINECRAFT {
            return Err(std::io::Error::other("Customize is only available for Minecraft; an equivalent loader component is not available.").into());
        }
        self.fetch_version_data(version).await?;
        let path = self
            .paths
            .metadata_dir()
            .join(uid)
            .join(format!("{version}.json"));
        let mut value: serde_json::Value = serde_json::from_slice(&tokio::fs::read(path).await?)?;
        value["uid"] = serde_json::json!(uid);
        value["version"] = serde_json::json!(version);
        value["formatVersion"] = serde_json::json!(1);
        value["name"] = serde_json::json!("Minecraft");
        value.as_object_mut().unwrap().remove("logging");
        crate::component::Component::parse(&serde_json::to_string(&value)?, Some(uid))?
            .validate_launch()?;
        Ok(value)
    }

    /// Fetch and cache the Mojang version JSON for one version id.
    pub async fn fetch_version_data(&self, version_id: &str) -> Result<VersionData, LaunchError> {
        let version_id = version_components::version_id(version_id)?;
        // Cached?
        let cache_path = self.paths.metadata_dir().join("net.minecraft").join(format!("{version_id}.json"));
        if let Ok(text) = tokio::fs::read_to_string(&cache_path).await {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
                if let Ok(vd) = VersionData::from_json(&json) {
                    return Ok(vd);
                }
            }
        }

        let manifest = meta::VersionManifest::fetch(&self.downloader.client()).await?;
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

    pub async fn fetch_lwjgl(&self, uid: &str, version: &str) -> Result<VersionData, LaunchError> {
        let version = version_components::version_id(version)?;
        if !matches!(uid, version_components::LWJGL | version_components::LWJGL3) {
            return Err(std::io::Error::other("Unknown LWJGL component.").into());
        }
        let cache = self.paths.metadata_dir().join(uid).join(format!("{version}.json"));
        let cached = tokio::fs::read(&cache).await.ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .filter(|json| json["uid"] == uid && json["version"] == version && json["libraries"].is_array());
        let fetched = cached.is_none();
        let mut json = match cached {
            Some(json) => json,
            None => self.downloader
                .get_json(&format!("https://meta.prismlauncher.org/v1/{uid}/{version}.json"))
                .await?,
        };
        if json["uid"] != uid || json["version"] != version || !json["libraries"].is_array() {
            return Err(std::io::Error::other("Invalid LWJGL component metadata.").into());
        }
        // Prism metadata represents LWJGL 3 native jars as separate classpath
        // libraries, not the extracted LWJGL 2 native classifiers.
        for lib in json["libraries"].as_array_mut().unwrap() {
            let spec = lib["name"].as_str().unwrap_or_default();
            let segments = spec.split(':').collect::<Vec<_>>();
            if segments.len() != 3 || segments.iter().any(|part| version_components::version_id(part).is_err()) {
                return Err(std::io::Error::other("Invalid LWJGL library coordinate.").into());
            }
            let coordinate = if let Some((name, classifier)) = segments[1].split_once("-natives-") {
                format!("{}:{name}:{}:natives-{classifier}", segments[0], segments[2])
            } else {
                spec.to_owned()
            };
            if lib["downloads"]["artifact"].is_object() {
                lib["downloads"]["artifact"]["path"] = serde_json::json!(meta::maven_path(&coordinate));
            }
        }
        let data = VersionData::from_json(&json)?;
        if fetched {
            if let Some(dir) = cache.parent() {
                let _ = tokio::fs::create_dir_all(dir).await;
                let _ = tokio::fs::write(cache, serde_json::to_vec(&json)?).await;
            }
        }
        Ok(data)
    }

    /// Download everything the profile needs, then extract natives.
    /// Point `settings.java_path` at a Java that can run `profile` and return
    /// a log line saying which. A Java the user configured is only verified;
    /// otherwise a launcher-managed runtime, then any detected install with
    /// a compatible major, then a fresh download of Mojang's runtime is used.
    /// Mirrors `AutoInstallJava` + `VerifyJavaInstall`.
    pub async fn resolve_java(&self, settings: &mut LaunchSettings, profile: &LaunchProfile) -> Result<String, LaunchError> {
        let majors = profile.compatible_java_majors();
        if majors.is_empty() {
            return Ok(format!("Using Java at {} (the version names no required Java)", settings.java_path.display()));
        }
        let mut downloaded = None;
        if !settings.java_path_explicit {
            let java_dir = self.paths.java_dir();
            let managed = profile
                .java_component
                .as_deref()
                .and_then(|component| crate::java::runtime_java_binary(&java_dir.join(component)));
            let java = match managed {
                Some(java) => Some(java),
                None => {
                    let majors = majors.clone();
                    let java_dir = java_dir.clone();
                    tokio::task::spawn_blocking(move || {
                        let installs = crate::java::detect_installs(&[java_dir]);
                        crate::java::compatible_install(&installs, &majors).map(|java| java.path.clone())
                    })
                    .await
                    .ok()
                    .flatten()
                }
            };
            if let Some(java) = java {
                settings.java_path = java;
            } else if let (true, Some(component)) = (settings.auto_java_download, profile.java_component.as_deref()) {
                settings.java_path =
                    crate::java::install_mojang_runtime(&self.downloader, &java_dir, component).await?;
                downloaded = Some(component);
            }
        }

        let java_path = settings.java_path.clone();
        let found = tokio::task::spawn_blocking(move || crate::java::check_java(&java_path)).await.ok().flatten();
        let path = settings.java_path.display();
        match found {
            Some(version) if majors.contains(&version.major) => Ok(match downloaded {
                Some(component) => format!("Downloaded Mojang's {component} runtime (Java {}) to {path}", version.raw),
                None => format!("Using Java {} at {path}", version.raw),
            }),
            Some(version) if settings.ignore_java_compatibility => Ok(format!(
                "Warning: Minecraft {} needs Java {}, but {path} is Java {}; launching anyway because the compatibility check is off",
                profile.minecraft_version,
                join_majors(&majors),
                version.major
            )),
            Some(version) => Err(LaunchError::IncompatibleJava {
                mc: profile.minecraft_version.clone(),
                path: settings.java_path.clone(),
                found: version.major,
                required: majors,
            }),
            None => Ok(format!("Warning: could not read the Java version of {path}; launching anyway")),
        }
    }

    async fn ensure_client_jar(&self, version: &str) -> Result<PathBuf, LaunchError> {
        let client_jar = self.client_jar_path(version);
        if !client_jar.exists() {
            let url = self
                .cached_client_url(version)
                .await
                .unwrap_or_else(|| {
                    format!(
                        "https://s3.amazonaws.com/Minecraft.Download/versions/{version}/{version}.jar"
                    )
                });
            self.downloader
                .download(&DownloadRequest::new(url, &client_jar))
                .await?;
        }
        Ok(client_jar)
    }

    pub async fn prepare_files(&self, profile: &LaunchProfile, settings: &LaunchSettings) -> Result<(), LaunchError> {
        let libraries_dir = self.paths.libraries_dir();
        if profile.component_main_jar.is_none() {
            self.ensure_client_jar(&profile.minecraft_version).await?;
        }

        // Libraries.
        // Old versions list some artifacts twice (once plain, once with
        // natives); downloading one path twice at once races on the rename.
        let mut queued = std::collections::HashSet::new();
        let mut requests = Vec::new();
        for lib in profile
            .libraries
            .iter()
            .chain(profile.natives.iter())
            .chain(profile.component_artifacts.iter())
            .chain(profile.component_main_jar.iter())
        {
            let dest = libraries_dir.join(&lib.path);
            if lib.path.is_absolute() && lib.url.is_none() {
                crate::component::check_local(&dest).map_err(|e| {
                    std::io::Error::new(
                        e.kind(),
                        format!("Missing or unsafe local artifact {}: {e}", dest.display()),
                    )
                })?;
            }
            if dest.exists() || queued.contains(&dest) {
                continue;
            }
            if let Some(url) = &lib.url {
                queued.insert(dest.clone());
                let mut req = DownloadRequest::new(url.clone(), dest);
                if let Some(sha1) = &lib.sha1 {
                    req = req.with_sha1(sha1.clone());
                }
                requests.push(req);
            } else if !dest.exists() {
                return Err(std::io::Error::other(format!(
                    "Missing library {}: {}",
                    lib.spec,
                    dest.display()
                ))
                .into());
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
                    if dest.exists() || !queued.insert(dest.clone()) {
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
        let natives_dir = profile.natives_override.clone().unwrap_or_else(|| settings.game_dir.join("bin"));
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

    /// Resolve the game and loader, select Java, and prepare the files for an installed instance.
    pub async fn prepare_instance(
        &self,
        settings: &mut LaunchSettings,
        loader: Loader,
        progress: impl FnMut(String),
    ) -> Result<LaunchProfile, LaunchError> {
        self.prepare_instance_with_loader(settings, loader, progress, |loader, version| async move {
            loaders::fetch_loader_profile(&self.downloader.client(), &loader, &version).await
        }).await
    }

    async fn prepare_instance_with_loader<F, Fut>(
        &self,
        settings: &mut LaunchSettings,
        loader: Loader,
        mut progress: impl FnMut(String),
        fetch_loader: F,
    ) -> Result<LaunchProfile, LaunchError>
    where
        F: Fn(Loader, String) -> Fut,
        Fut: std::future::Future<Output = Result<VersionData, String>>,
    {
        let mut profile = LaunchProfile {
            minecraft_version: settings.mc_version.clone(),
            assets_id: "legacy".into(),
            ..Default::default()
        };
        let pack = match Pack::load(&settings.instance_dir) {
            Ok(pack) => Some(pack),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        let components: Vec<(String, String)> = match &pack {
            Some(pack) => pack
                .enabled_components()
                .map(|(uid, version)| (uid.to_owned(), version.to_owned()))
                .collect(),
            None => std::iter::once((
                version_components::MINECRAFT.to_owned(),
                settings.mc_version.clone(),
            ))
            .chain((loader != Loader::None).then(|| {
                (
                    loader.uid().to_owned(),
                    loader.version().unwrap_or_default().to_owned(),
                )
            }))
            .collect(),
        };
        let mut loader_applied = false;
        for (uid, version) in components {
            let is_loader = matches!(uid.as_str(), "net.fabricmc.fabric-loader" | "org.quiltmc.quilt-loader" | "net.minecraftforge" | "net.neoforged");
            if is_loader && loader_applied {
                progress(format!("Warning: ignoring additional loader component {uid}; the first enabled loader takes precedence."));
                continue;
            }
            if let Some(patch) = pack
                .as_ref()
                .map(|pack| pack.patch(&uid))
                .transpose()?
                .flatten()
            {
                loader_applied |= is_loader;
                if matches!(
                    uid.as_str(),
                    version_components::LWJGL | version_components::LWJGL3
                ) {
                    profile
                        .libraries
                        .retain(|lib| !lib.spec.starts_with("org.lwjgl"));
                    profile
                        .natives
                        .retain(|lib| !lib.spec.starts_with("org.lwjgl"));
                }
                patch.apply(&mut profile, &settings.instance_dir)?;
                continue;
            }
            if (uid != version_components::MINECRAFT && !is_loader
                && pack.as_ref().is_some_and(|pack| pack.dependency_only(&uid)))
                || matches!(uid.as_str(), "net.fabricmc.intermediary" | "org.quiltmc.hashed")
            {
                progress(format!("Using implicit dependency {uid} from the Minecraft/loader profile"));
                continue;
            }
            if uid == version_components::MINECRAFT {
                let data = self.fetch_version_data(&version).await?;
                profile.apply_minecraft(&data);
                profile.component_main_jar = Some(ResolvedLibrary {
                    spec: format!("net.minecraft:client:{version}"),
                    path: self.client_jar_path(&version),
                    url: Some(self.cached_client_url(&version).await.unwrap_or_else(|| format!("https://s3.amazonaws.com/Minecraft.Download/versions/{version}/{version}.jar"))),
                    sha1: None,
                    native: false,
                });
                continue;
            }
            if matches!(
                uid.as_str(),
                version_components::LWJGL | version_components::LWJGL3
            ) {
                if profile.lwjgl_family() != Some(uid.as_str()) {
                    return Err(std::io::Error::other("The LWJGL override does not match this Minecraft version. Reset it on the Version page.").into());
                }
                let replacement = self.fetch_lwjgl(&uid, &version).await?;
                profile.apply_lwjgl(&replacement)?;
                profile.natives_override = Some(
                    settings
                        .game_dir
                        .join("bin")
                        .join(format!("lwjgl-{uid}-{version}")),
                );
                progress(format!("Using {uid} {version}"));
                continue;
            }
            let loader =
                match uid.as_str() {
                    "net.fabricmc.fabric-loader" => Loader::Fabric { version },
                    "org.quiltmc.quilt-loader" => Loader::Quilt { version },
                    "net.minecraftforge" => Loader::Forge { version },
                    "net.neoforged" => Loader::NeoForge { version },
                    _ => {
                        progress(format!("Warning: component {uid} has no local patch or supported metadata resolver; ignoring it for compatibility."));
                        continue;
                    }
                };
            loader_applied = true;
            progress(format!("Preparing loader: {loader:?}"));
            let version = settings.mc_version.clone();
            if !crate::modplatform::technic::apply_forge_patch(
                &mut profile, &settings.instance_dir, &settings.game_dir, &loader,
            ).map_err(|error| LaunchError::Loader { loader: loader.clone(), message: error.to_string() })? {
                let overlay = match &loader {
                    Loader::Forge { .. } | Loader::NeoForge { .. } => {
                        progress(self.resolve_java(settings, &profile).await?);
                        let versions_dir = settings.game_dir.join("versions").join(&version);
                        let destination = versions_dir.join(format!("{version}.jar"));
                        if !destination.exists() {
                            let client = self.ensure_client_jar(&version).await?;
                            tokio::fs::create_dir_all(&versions_dir).await?;
                            tokio::fs::copy(client, destination).await?;
                        }
                        loaders::ensure_forge_installed(
                            &settings.java_path,
                            &loader,
                            &version,
                            &settings.game_dir,
                            &self.downloader,
                        )
                        .await
                    }
                    _ => fetch_loader(loader.clone(), version).await,
                }
                .map_err(|message| LaunchError::Loader { loader, message })?;
                profile.apply_loader(&overlay);
            }
        }

        progress(self.resolve_java(settings, &profile).await?);

        crate::modplatform::atlauncher::apply_launch(&settings.instance_dir, &mut profile)?;
        progress(format!(
            "Resolved profile: mainClass={}, libraries={}, natives={}",
            profile.main_class,
            profile.libraries.len(),
            profile.natives.len()
        ));
        if matches!(settings.quick_play, Some(QuickPlayTarget::Singleplayer { .. }))
            && !profile.supports_quick_play_singleplayer()
        {
            progress(format!(
                "Minecraft {} does not support Quick Play singleplayer; launching to the title screen",
                settings.mc_version
            ));
        }
        self.prepare_files(&profile, settings).await?;
        let mut mods: Vec<_> = profile
            .component_jar_mods
            .iter()
            .map(|lib| self.paths.libraries_dir().join(&lib.path))
            .collect();
        if let Some(main) = &profile.component_main_jar {
            profile.client_override = Some(self.paths.libraries_dir().join(&main.path));
        }
        if let Some(pack) = &pack {
            mods.extend(pack.archive_paths(ArchiveKind::JarMod)?);
            profile.extra_classpath = pack.archive_paths(ArchiveKind::Classpath)?;
        }
        if !mods.is_empty() {
            let client = profile
                .client_override
                .clone()
                .unwrap_or_else(|| self.client_jar_path(&settings.mc_version));
            let root = settings.instance_dir.clone();
            std::fs::create_dir_all(root.join("version-files"))?;
            let count = mods.len();
            let patched = tokio::task::spawn_blocking(move || {
                version_components::patched_client(&root, &client, &mods)
            })
            .await
            .map_err(std::io::Error::other)??;
            profile.client_override = Some(patched);
            progress(format!(
                "Applied {count} jar mod(s) to the Minecraft client"
            ));
        }
        if !profile.extra_classpath.is_empty() {
            progress(format!(
                "Added {} extra classpath archive(s)",
                profile.extra_classpath.len()
            ));
        }
        std::env::join_paths(
            profile
                .extra_classpath
                .iter()
                .chain(profile.client_override.iter()),
        )
        .map_err(std::io::Error::other)?;
        progress("Downloads complete".to_string());
        Ok(profile)
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
        let natives_dir = profile.natives_override.clone().unwrap_or_else(|| settings.game_dir.join("bin"));
        let client_jar = profile.client_override.clone().unwrap_or_else(|| self.client_jar_path(&profile.minecraft_version));

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
        classpath.extend(profile.extra_classpath.iter().map(|path| path.to_string_lossy().into_owned()));
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

        match &settings.quick_play {
            Some(QuickPlayTarget::Singleplayer { world }) => {
                tokens.insert("quickPlaySingleplayer".into(), world.clone());
            }
            Some(target @ QuickPlayTarget::Multiplayer { .. }) => {
                tokens.insert("quickPlayMultiplayer".into(), target.address());
            }
            None => {}
        }
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
        let java_args = command[1..].join(" ");

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
            command.push(format!(
                "-Djava.library.path={}",
                natives_dir.to_string_lossy()
            ));
        }

        command.extend(profile.component_jvm_args.iter().map(|arg| substitute(arg)));
        for (agent, argument) in &profile.agents {
            let path = libraries_dir.join(&agent.path);
            command.push(format!(
                "-javaagent:{}{}",
                path.to_string_lossy(),
                if argument.is_empty() {
                    String::new()
                } else {
                    format!("={argument}")
                }
            ));
        }

        // Classpath + main class.
        command.push("-cp".to_string());
        command.push(classpath_str);
        command.push(profile.main_class.clone());

        // Game arguments.
        if let Some(args) = &profile.modern_arguments {
            let mut features = std::collections::BTreeMap::new();
            features.insert(
                "is_quick_play_singleplayer".into(),
                matches!(
                    settings.quick_play,
                    Some(QuickPlayTarget::Singleplayer { .. })
                ),
            );
            features.insert(
                "is_quick_play_multiplayer".into(),
                matches!(
                    settings.quick_play,
                    Some(QuickPlayTarget::Multiplayer { .. })
                ),
            );
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

        command.extend([
            "--width".into(),
            settings.window_width.to_string(),
            "--height".into(),
            settings.window_height.to_string(),
        ]);
        if !profile.supports_quick_play_multiplayer() {
            if let Some(QuickPlayTarget::Multiplayer { host, port }) = &settings.quick_play {
                command.extend([
                    "--server".into(),
                    host.clone(),
                    "--port".into(),
                    port.to_string(),
                ]);
            }
        }
        let mut env = vec![
            ("INST_NAME".into(), settings.instance_name.clone()),
            ("INST_ID".into(), settings.instance_id.clone()),
            ("INST_DIR".into(), settings.instance_dir.to_string_lossy().into_owned()),
            ("INST_MC_DIR".into(), settings.game_dir.to_string_lossy().into_owned()),
            ("INST_JAVA".into(), settings.java_path.to_string_lossy().into_owned()),
            ("INST_JAVA_ARGS".into(), java_args),
            ("NO_COLOR".into(), "1".into()),
        ];
        env.extend(settings.env.iter().cloned());
        let mut command_error = split_args(&settings.raw_jvm_args)
            .err()
            .map(|error| format!("Invalid JVM arguments: {error}"));
        match split_args(&settings.wrapper_command) {
            Ok(mut wrapper) => {
                wrapper.extend(command);
                command = wrapper;
            }
            Err(error) => command_error = Some(format!("Invalid wrapper command: {error}")),
        }

        PreparedLaunch {
            command,
            working_dir: settings.game_dir.clone(),
            env,
            instance_dir: settings.instance_dir.clone(),
            pre_launch_command: settings.pre_launch_command.clone(),
            post_exit_command: settings.post_exit_command.clone(),
            command_error,
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

    #[cfg(unix)]
    #[test]
    fn post_exit_hook_reports_partial_output_on_the_calling_thread() {
        use std::{sync::mpsc, time::Duration};

        let dir = tempfile::tempdir().unwrap();
        let launch = PreparedLaunch {
            command: Vec::new(),
            working_dir: dir.path().into(),
            env: Vec::new(),
            instance_dir: dir.path().into(),
            pre_launch_command: String::new(),
            post_exit_command: r#"
                printf foreground
                printf diagnostic >&2
                (
                    i=0
                    while [ ! -f release ] && [ "$i" -lt 100 ]; do
                        sleep 0.05
                        i=$((i+1))
                    done
                    printf 'background\n'
                    touch background-finished
                ) >background.log 2>&1 &
            "#.into(),
            command_error: None,
        };
        let (tx, rx) = mpsc::channel();
        let runner = std::thread::spawn(move || {
            let (output_tx, output_rx) = mpsc::channel();
            let caller = std::thread::current().id();
            let result = launch.run_post_exit_with_output(Box::new(move |line| {
                let _ = output_tx.send((std::thread::current().id(), line));
            }));
            let _ = tx.send(result.map(|()| (caller, output_rx.try_iter().collect::<Vec<_>>())));
        });
        let result = rx.recv_timeout(Duration::from_secs(3));
        std::fs::write(dir.path().join("release"), []).unwrap();
        runner.join().unwrap();
        let (caller, mut output) = result.unwrap().unwrap();
        output.sort_by(|(_, lhs), (_, rhs)| lhs.cmp(rhs));
        assert_eq!(output, [
            (caller, "Post-exit (stderr): diagnostic".into()),
            (caller, "Post-exit (stdout): foreground".into()),
        ]);
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while !dir.path().join("background-finished").exists() {
            assert!(std::time::Instant::now() < deadline, "background hook did not finish");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            std::fs::read_to_string(dir.path().join("background.log")).unwrap(),
            "background\n"
        );
    }

    #[test]
    fn server_addresses() {
        for (input, host, port) in [
            (" example.org ", "example.org", 25565),
            ("h:42", "h", 42),
            ("[::1]:123", "::1", 123),
            ("::1", "::1", 25565),
            ("[::1]", "::1", 25565),
        ] {
            assert_eq!(
                QuickPlayTarget::parse_server(input).unwrap(),
                QuickPlayTarget::Multiplayer {
                    host: host.into(),
                    port
                }
            );
        }
        for input in [
            "",
            " ",
            ":12",
            "h:",
            "h:no",
            "h:65536",
            "[::1",
            "[]:12",
            "[::1]oops",
        ] {
            assert!(QuickPlayTarget::parse_server(input).is_err(), "{input}");
        }
        assert_eq!(
            QuickPlayTarget::parse_server("[::1]:42").unwrap().address(),
            "[::1]:42"
        );
    }

    #[test]
    fn quick_play_commands() {
        let paths = Paths {
            root: std::env::temp_dir().join("prism-quick-play-command-test"),
        };
        let inst = crate::instance::Instance {
            kind: crate::instance::InstanceKind::Java,
            id: "test".into(),
            root: paths.root.clone(),
            name: "Test".into(),
            icon_key: "default".into(),
            notes: String::new(),
            total_time_played: 0,
            last_launch: 0,
            group: None,
        };
        let mut settings = LaunchSettings::for_instance(&inst, &paths);
        let launcher = Launcher::new(paths, Arc::new(Downloader::new()));
        let mut profile = LaunchProfile::default();
        profile.modern_arguments = Some(meta::Arguments {
            jvm: vec![],
            game: [
                ("singleplayer", "Singleplayer"),
                ("multiplayer", "Multiplayer"),
            ]
            .into_iter()
            .map(|(feature, token)| meta::ArgValue::Conditional {
                rules: vec![meta::Rule {
                    action: meta::RuleAction::Allow,
                    os: None,
                    features: Some([(format!("is_quick_play_{feature}"), true)].into()),
                }],
                value: vec![
                    format!("--quickPlay{token}"),
                    format!("${{quickPlay{token}}}"),
                ],
            })
            .collect(),
        });
        assert!(profile.supports_quick_play_singleplayer());
        assert!(profile.supports_quick_play_multiplayer());
        let command = launcher.build_command(&settings, None, &profile).command;
        assert!(!command.iter().any(|arg| arg.contains("quickPlay")));
        for (target, flag, value) in [
            (
                QuickPlayTarget::parse_server("host:42").unwrap(),
                "--quickPlayMultiplayer",
                "host:42",
            ),
            (
                QuickPlayTarget::Singleplayer {
                    world: "World".into(),
                },
                "--quickPlaySingleplayer",
                "World",
            ),
        ] {
            settings.quick_play = Some(target);
            let command = launcher.build_command(&settings, None, &profile).command;
            assert!(command.windows(2).any(|args| args == [flag, value]));
            assert_eq!(
                command
                    .iter()
                    .filter(|arg| arg.starts_with("--quickPlay"))
                    .count(),
                1
            );
        }
        profile.modern_arguments = None;
        profile.legacy_arguments = Some("--username ${auth_player_name}".into());
        settings.quick_play = Some(QuickPlayTarget::parse_server("h:43").unwrap());
        assert!(launcher
            .build_command(&settings, None, &profile)
            .command
            .ends_with(&["--server".into(), "h".into(), "--port".into(), "43".into()]));
        settings.quick_play = Some(QuickPlayTarget::Singleplayer {
            world: "World".into(),
        });
        assert!(!launcher
            .build_command(&settings, None, &profile)
            .command
            .iter()
            .any(|arg| arg.contains("quickPlay")));
    }

    #[test]
    fn forge_legacy_profile_replaces_vanilla_game_arguments() {
        let base = meta::VersionData::from_json(&serde_json::json!({
            "id": "1.12.2", "mainClass": "net.minecraft.client.main.Main",
            "minecraftArguments": "--username ${auth_player_name} --versionType release"
        })).unwrap();
        let forge = meta::VersionData::from_json(&serde_json::json!({
            "id": "1.12.2-forge-14.23.5.2864", "mainClass": "net.minecraft.launchwrapper.Launch",
            "minecraftArguments": "--username ${auth_player_name} --tweakClass net.minecraftforge.fml.common.launcher.FMLTweaker --versionType Forge"
        })).unwrap();
        let mut profile = LaunchProfile::from_mojang(&base);
        profile.apply_loader(&forge);
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths { root: dir.path().into() };
        let instance = crate::InstanceList::create(&paths, "Forge", "default", "1.12.2").unwrap();
        let settings = LaunchSettings::for_instance(&instance, &paths);
        let launcher = Launcher::new(paths, Arc::new(Downloader::new()));
        let command = launcher.build_command(&settings, None, &profile).command;
        assert!(command.windows(2).any(|args| args == ["--tweakClass", "net.minecraftforge.fml.common.launcher.FMLTweaker"]));
        assert!(command.windows(2).any(|args| args == ["--versionType", "Forge"]));
        assert_eq!(command.iter().filter(|arg| *arg == "--username").count(), 1);
    }

    #[tokio::test]
    async fn fetch_version_data_accepts_official_ids_with_spaces() {
        let temp = tempfile::tempdir().unwrap();
        let paths = Paths { root: temp.path().into() };
        let cache = paths.metadata_dir().join("net.minecraft");
        std::fs::create_dir_all(&cache).unwrap();
        let launcher = Launcher::new(paths, Arc::new(Downloader::new()));
        for id in ["3D Shareware v1.34", "1.14.2 Pre-Release 4"] {
            std::fs::write(
                cache.join(format!("{id}.json")),
                serde_json::json!({"id": id}).to_string(),
            )
            .unwrap();
            assert_eq!(launcher.fetch_version_data(id).await.unwrap().id, id);
        }
        for id in [
            "..",
            "../other version",
            "..\\other version",
            "/other version",
            "C:other version",
        ] {
            assert!(version_components::version_id(id).is_err(), "{id:?}");
        }
    }

    #[test]
    fn patched_client_and_classpath_entries_keep_the_profile_order() {
        let temp = tempfile::tempdir().unwrap();
        let paths = Paths { root: temp.path().into() };
        let inst = crate::instance::InstanceList::create(&paths, "QA", "default", "1.20.4").unwrap();
        let settings = LaunchSettings::for_instance(&inst, &paths);
        let launcher = Launcher::new(paths, Arc::new(Downloader::new()));
        let patched = temp.path().join("patched.jar");
        let extra = [temp.path().join("extra one.jar"), temp.path().join("extra two.jar")];
        let profile = LaunchProfile {
            minecraft_version: "1.20.4".into(),
            client_override: Some(patched.clone()),
            extra_classpath: extra.to_vec(),
            ..Default::default()
        };
        let command = launcher.build_command(&settings, None, &profile).command;
        let classpath = command.windows(2).find(|args| args[0] == "-cp").unwrap();
        assert_eq!(std::env::split_paths(&classpath[1]).collect::<Vec<_>>(),
            [patched, extra[0].clone(), extra[1].clone()]);
    }

    #[test]
    fn agents_are_single_jvm_arguments_and_never_classpath_entries() {
        let temp = tempfile::tempdir().unwrap();
        let paths = Paths {
            root: temp.path().into(),
        };
        let inst =
            crate::instance::InstanceList::create(&paths, "QA", "default", "1.20.1").unwrap();
        let settings = LaunchSettings::for_instance(&inst, &paths);
        let launcher = Launcher::new(paths, Arc::new(Downloader::new()));
        let mut profile = LaunchProfile {
            main_class: "GameMain".into(),
            minecraft_version: "1.20.1".into(),
            ..Default::default()
        };
        for (index, argument) in ["", "spaces and = signs"].iter().enumerate() {
            profile.agents.push((
                ResolvedLibrary {
                    spec: format!("custom:agent:{index}"),
                    path: temp.path().join(format!("agent {index}.jar")),
                    url: None,
                    sha1: None,
                    native: false,
                },
                (*argument).into(),
            ));
        }
        let command = launcher.build_command(&settings, None, &profile).command;
        let agents: Vec<_> = command
            .iter()
            .filter(|arg| arg.starts_with("-javaagent:"))
            .collect();
        assert_eq!(agents.len(), 2);
        assert!(agents[0].ends_with("agent 0.jar"));
        assert!(agents[1].ends_with("agent 1.jar=spaces and = signs"));
        let main = command.iter().position(|arg| arg == "GameMain").unwrap();
        assert!(command[..main].iter().any(|arg| arg == agents[1]));
        let classpath = command.windows(2).find(|args| args[0] == "-cp").unwrap();
        assert!(!classpath[1].contains("agent"));
    }

    #[tokio::test]
    async fn minecraft_customize_preserves_launch_arguments_assets_java_and_downloads() {
        let temp = tempfile::tempdir().unwrap();
        let paths = Paths { root: temp.path().into() };
        let inst = crate::instance::InstanceList::create(&paths, "QA", "default", "1.20.1").unwrap();
        let settings = LaunchSettings::for_instance(&inst, &paths);
        let launcher = Launcher::new(paths.clone(), Arc::new(Downloader::new()));
        let raw = serde_json::json!({"id":"1.20.1","type":"release","mainClass":"GameMain",
            "arguments":{"jvm":["-cp","${classpath}"],"game":["--version","${version_name}"]},
            "javaVersion":{"component":"java-runtime-gamma","majorVersion":17},
            "downloads":{"client":{"url":"https://example.invalid/client.jar","sha1":"123"}},
            "assetIndex":{"id":"5","url":"https://example.invalid/assets.json","sha1":"456"},
            "logging":{"client":{"argument":"unused by existing launch"}},
            "libraries":[{"name":"group:lib:1","downloads":{"artifact":{"url":"https://example.invalid/lib.jar"}}}]
        });
        let cache = paths.metadata_dir().join("net.minecraft");
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::write(cache.join("1.20.1.json"), raw.to_string()).unwrap();
        let doc = launcher.component_document(version_components::MINECRAFT, "1.20.1").await.unwrap();
        assert_eq!(doc["downloads"], raw["downloads"]);
        assert_eq!(doc["assetIndex"], raw["assetIndex"]);
        let data = VersionData::from_json(&raw).unwrap();
        let mut baseline = LaunchProfile::from_mojang(&data);
        let mut customized = LaunchProfile::default();
        crate::component::Component(doc).apply(&mut customized, &inst.root).unwrap();
        assert_eq!(customized.assets_id, baseline.assets_id);
        assert_eq!(customized.asset_index, baseline.asset_index);
        assert_eq!(customized.java_major, baseline.java_major);
        assert_eq!(customized.java_component, baseline.java_component);
        baseline.client_override = Some(paths.libraries_dir().join(&customized.component_main_jar.as_ref().unwrap().path));
        customized.client_override = baseline.client_override.clone();
        assert!(customized.libraries[0].path.starts_with(inst.root.join("libraries/.component-cache")));
        baseline.libraries[0].path = customized.libraries[0].path.clone();
        assert_eq!(launcher.build_command(&settings, None, &baseline).command,
            launcher.build_command(&settings, None, &customized).command);
        let mut ordered = LaunchProfile::default();
        ordered.component_jvm_args.push("-Dearlier=yes".into());
        ordered.apply_minecraft(&data);
        assert_eq!(ordered.main_class, baseline.main_class);
        assert_eq!(ordered.component_jvm_args, ["-Dearlier=yes"]);
        assert!(launcher.component_document("net.fabricmc.fabric-loader", "0.16").await.is_err());
    }

    #[tokio::test]
    async fn missing_local_agent_fails_preparation_without_metadata_fetch() {
        let temp = tempfile::tempdir().unwrap();
        let paths = Paths { root: temp.path().into() };
        let inst = crate::instance::InstanceList::create(&paths, "QA", "default", "1.20.1").unwrap();
        let settings = LaunchSettings::for_instance(&inst, &paths);
        let mut profile = LaunchProfile::default();
        crate::component::Component::parse(r#"{"uid":"test","formatVersion":1,"mainJar":{"name":"custom:main:1","MMC-hint":"local"},"+agents":[{"name":"custom:agent:1","MMC-hint":"local"}]}"#, None)
            .unwrap().apply(&mut profile, &inst.root).unwrap();
        let launcher = Launcher::new(paths, Arc::new(Downloader::new()));
        assert!(launcher.prepare_files(&profile, &settings).await.unwrap_err().to_string().contains("local artifact"));
    }

    async fn upstream_loader_profile_regression(loader: Loader, rows: serde_json::Value) {
        use std::cell::Cell;
        let temp = tempfile::tempdir().unwrap();
        let paths = Paths { root: temp.path().into() };
        let inst = crate::instance::InstanceList::create(&paths, "QA", "default", "1.20.1").unwrap();
        let mut settings = LaunchSettings::for_instance(&inst, &paths);
        let launcher = Launcher::new(paths.clone(), Arc::new(Downloader::new()));
        let base = serde_json::json!({"id":"1.20.1","mainClass":"VanillaMain","assets":"legacy",
            "arguments":{"jvm":["-Dbase=yes"],"game":["--base"]},
            "libraries":[{"name":"org.lwjgl:lwjgl:3.3.1"}]});
        let overlay = serde_json::json!({"mainClass":"LoaderMain",
            "arguments":{"jvm":["-Dloader=yes"],"game":["--loader"]},
            "libraries":[{"name":"net.fabricmc:intermediary:1.20.1"}]});
        let cache = paths.metadata_dir().join("net.minecraft");
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::write(cache.join("1.20.1.json"), base.to_string()).unwrap();
        std::fs::write(inst.root.join("mmc-pack.json"), serde_json::json!({"formatVersion":1,"components":rows}).to_string()).unwrap();
        let client = launcher.client_jar_path("1.20.1");
        std::fs::create_dir_all(client.parent().unwrap()).unwrap();
        zip::ZipWriter::new(std::fs::File::create(&client).unwrap()).finish().unwrap();
        let mut expected = LaunchProfile::from_mojang(&VersionData::from_json(&base).unwrap());
        let overlay = VersionData::from_json(&overlay).unwrap();
        expected.apply_loader(&overlay);
        for lib in &expected.libraries {
            let dest = paths.libraries_dir().join(&lib.path);
            std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
            std::fs::write(dest, "fixture library").unwrap();
        }
        if matches!(loader, Loader::Forge { .. }) {
            let id = "1.20.1-forge-47.2.0";
            let dir = settings.game_dir.join("versions").join(id);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(format!("{id}.json")), serde_json::json!({"mainClass":"LoaderMain",
                "arguments":{"jvm":["-Dloader=yes"],"game":["--loader"]},
                "libraries":[{"name":"net.fabricmc:intermediary:1.20.1"}]}).to_string()).unwrap();
        }
        let calls = Cell::new(0);
        let mut messages = Vec::new();
        assert_eq!(crate::instance::loader_of(&inst), loader);
        let actual = launcher.prepare_instance_with_loader(&mut settings, Loader::None, |s| messages.push(s), |resolved, version| {
            assert_eq!(resolved, loader);
            assert_eq!(version, "1.20.1");
            calls.set(calls.get() + 1);
            std::future::ready(Ok(overlay.clone()))
        }).await.unwrap();
        assert_eq!(calls.get(), usize::from(!matches!(loader, Loader::Forge { .. })));
        assert_eq!(launcher.build_command(&settings, None, &actual).command,
            launcher.build_command(&settings, None, &expected).command);
        assert_eq!(actual.libraries.len(), expected.libraries.len());
        assert_eq!(actual.main_class, expected.main_class);
        assert!(messages.iter().any(|s| s.contains("implicit dependency org.lwjgl3")));
        assert!(messages.iter().any(|s| s.contains("ignoring it for compatibility")));
    }

    #[tokio::test]
    async fn upstream_fabric_dependencies_do_not_add_profiles_or_duplicate_loaders() {
        upstream_loader_profile_regression(Loader::Fabric { version: "0.16.0".into() }, serde_json::json!([
            {"uid":"net.minecraft","version":"1.20.1","important":true},
            {"uid":"org.lwjgl3","version":"3.3.1","dependencyOnly":true},
            {"uid":"net.fabricmc.intermediary","version":"1.20.1"},
            {"uid":"org.quiltmc.hashed","version":"1.20.1"},
            {"uid":"some.upstream.dependency","version":"1","dependencyOnly":true},
            {"uid":"net.fabricmc.fabric-loader","version":"0.16.0"},
            {"uid":"org.quiltmc.quilt-loader","version":"0.26.0"},
            {"uid":"unknown.unpatched","version":"1"}
        ])).await;
    }

    #[tokio::test]
    async fn upstream_forge_dependencies_preserve_the_installed_loader_profile() {
        upstream_loader_profile_regression(Loader::Forge { version: "47.2.0".into() }, serde_json::json!([
            {"uid":"net.minecraft","version":"1.20.1","important":true},
            {"uid":"org.lwjgl3","version":"3.3.1","dependencyOnly":true},
            {"uid":"net.minecraftforge","version":"47.2.0"},
            {"uid":"unknown.unpatched","version":"1"}
        ])).await;
    }

    #[tokio::test]
    async fn patches_replace_metadata_and_legacy_mods_overlay_component_mods() {
        use std::io::{Read, Write};
        fn jar(path: &std::path::Path, content: &str) {
            let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
            zip.start_file("marker", zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(content.as_bytes()).unwrap();
            zip.finish().unwrap();
        }
        let temp = tempfile::tempdir().unwrap();
        let paths = Paths {
            root: temp.path().into(),
        };
        let inst =
            crate::instance::InstanceList::create(&paths, "QA", "default", "1.20.1").unwrap();
        let mut settings = LaunchSettings::for_instance(&inst, &paths);
        let launcher = Launcher::new(paths.clone(), Arc::new(Downloader::new()));
        std::fs::create_dir_all(inst.root.join("libraries")).unwrap();
        std::fs::create_dir_all(inst.root.join("jarmods")).unwrap();
        jar(
            &inst.root.join("libraries/customjar-1.jar"),
            "custom client",
        );
        jar(&inst.root.join("jarmods/component.jar"), "component");
        let legacy = temp.path().join("legacy.jar");
        jar(&legacy, "legacy");
        let mc = serde_json::json!({"uid":"net.minecraft","formatVersion":1,"version":"1.20.1","mainClass":"PatchMain",
            "mainJar":{"name":"custom:customjar:1","MMC-hint":"local"},
            "jarMods":[{"name":"custom:component:1","MMC-hint":"local","MMC-filename":"component.jar"}]});
        let pack = Pack::load(&inst.root)
            .unwrap()
            .customize(version_components::MINECRAFT, &mc)
            .unwrap();
        let profile = launcher
            .prepare_instance(&mut settings, Loader::None, |_| {})
            .await
            .unwrap();
        assert_eq!(profile.main_class, "PatchMain");
        let read_marker = |profile: &LaunchProfile| {
            let mut archive = zip::ZipArchive::new(
                std::fs::File::open(profile.client_override.as_ref().unwrap()).unwrap(),
            )
            .unwrap();
            let mut text = String::new();
            archive
                .by_name("marker")
                .unwrap()
                .read_to_string(&mut text)
                .unwrap();
            text
        };
        assert_eq!(read_marker(&profile), "component");
        let pack = pack.add_archive(&legacy, ArchiveKind::JarMod).unwrap();
        let profile = launcher
            .prepare_instance(&mut settings, Loader::None, |_| {})
            .await
            .unwrap();
        assert_eq!(read_marker(&profile), "legacy");
        assert!(
            std::fs::read_to_string(paths.metadata_dir().join("net.minecraft/1.20.1.json"))
                .is_err(),
            "local Minecraft patch must not fetch metadata"
        );
        let unsupported = mc
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .chain([("+traits".into(), serde_json::json!(["unsupported"]))])
            .collect::<serde_json::Map<_, _>>();
        std::fs::write(
            inst.root.join("patches/net.minecraft.json"),
            serde_json::to_vec(&unsupported).unwrap(),
        )
        .unwrap();
        assert!(launcher
            .prepare_instance(&mut settings, Loader::None, |_| {})
            .await
            .unwrap_err()
            .to_string()
            .contains("+traits"));
        drop(pack);
    }

    #[cfg(unix)]
    fn fake_java(home: &Path, version: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let bin = home.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let java = bin.join("java");
        std::fs::write(&java, format!("#!/bin/sh\necho 'openjdk version \"{version}\"' >&2\n")).unwrap();
        std::fs::set_permissions(&java, std::fs::Permissions::from_mode(0o755)).unwrap();
        java
    }

    #[cfg(unix)]
    fn java_test_setup(name: &str) -> (Paths, LaunchSettings, Launcher) {
        let root = std::env::temp_dir().join(format!("prism-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let paths = Paths { root };
        let inst = crate::instance::Instance {
            kind: crate::instance::InstanceKind::Java,
            id: "test".into(),
            root: paths.instances_dir().join("test"),
            name: "Test".into(),
            icon_key: "default".into(),
            notes: String::new(),
            total_time_played: 0,
            last_launch: 0,
            group: None,
        };
        let mut settings = LaunchSettings::for_instance(&inst, &paths);
        settings.auto_java_download = false;
        let launcher = Launcher::new(paths.clone(), Arc::new(Downloader::new()));
        (paths, settings, launcher)
    }

    #[cfg(unix)]
    fn java_profile(component: &str, major: i64) -> LaunchProfile {
        LaunchProfile {
            minecraft_version: "test".into(),
            java_component: Some(component.into()),
            java_major: Some(major),
            ..LaunchProfile::default()
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn launch_picks_java_matching_minecraft_version() {
        let (paths, mut settings, launcher) = java_test_setup("java-pick");
        let java8 = fake_java(&paths.java_dir().join("jre-legacy"), "1.8.0_392");
        let java21 = fake_java(&paths.java_dir().join("java-runtime-delta"), "21.0.3");

        for (component, major, expected) in [("jre-legacy", 8, &java8), ("java-runtime-delta", 21, &java21)] {
            launcher.resolve_java(&mut settings, &java_profile(component, major)).await.unwrap();
            assert_eq!(&settings.java_path, expected, "Java {major}");
        }

        // No runtime under the requested component name: any detected
        // install with the right major is used.
        launcher.resolve_java(&mut settings, &java_profile("java-runtime-gamma", 8)).await.unwrap();
        assert_eq!(crate::java::check_java(&settings.java_path).unwrap().major, 8);
        let _ = std::fs::remove_dir_all(&paths.root);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn launch_rejects_incompatible_explicit_java() {
        let (paths, mut settings, launcher) = java_test_setup("java-verify");
        let java8 = fake_java(&paths.root.join("jdk8"), "1.8.0_392");
        fake_java(&paths.java_dir().join("java-runtime-delta"), "21.0.3");
        settings.java_path = java8.clone();
        settings.java_path_explicit = true;
        let profile = java_profile("java-runtime-delta", 21);

        assert!(matches!(
            launcher.resolve_java(&mut settings, &profile).await,
            Err(LaunchError::IncompatibleJava { found: 8, .. })
        ));
        settings.ignore_java_compatibility = true;
        launcher.resolve_java(&mut settings, &profile).await.unwrap();
        assert_eq!(settings.java_path, java8);
        let _ = std::fs::remove_dir_all(&paths.root);
    }

    #[test]
    fn offline_uuid_format() {
        let uuid = offline_uuid("Notch");
        // UUID v3 of OfflinePlayer:Notch is a well-known constant.
        assert_eq!(uuid, "b50ad385-829d-3141-a216-7e7d7539ba7f");
        assert_eq!(offline_uuid("Player").len(), 36);
    }
}
