//! Loader components: Fabric / Quilt (meta APIs) and the profile overlay
//! they produce. Mirrors PrismLauncher's Component/PackProfile system in a
//! simplified form: the loader profile JSON is applied on top of the Mojang
//! one.

use crate::meta::VersionData;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Loader {
    None,
    Fabric { version: String },
    Quilt { version: String },
    // Forge/NeoForge use installers rather than self-contained profiles.
    Forge { version: String },
    NeoForge { version: String },
}

impl Loader {
    pub fn uid(&self) -> &'static str {
        match self {
            Loader::None => "net.minecraft",
            Loader::Fabric { .. } => "net.fabricmc.fabric-loader",
            Loader::Quilt { .. } => "org.quiltmc.quilt-loader",
            Loader::Forge { .. } => "net.minecraftforge",
            Loader::NeoForge { .. } => "net.neoforged",
        }
    }

    pub fn version(&self) -> Option<&str> {
        match self {
            Loader::None => None,
            Loader::Fabric { version }
            | Loader::Quilt { version }
            | Loader::Forge { version }
            | Loader::NeoForge { version } => Some(version),
        }
    }
}

/// Loader entries as returned by the Fabric/Quilt meta APIs.
#[derive(Debug, Clone, Deserialize)]
pub struct MetaLoaderEntry {
    pub loader: MetaLoaderVersion,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MetaLoaderVersion {
    pub version: String,
    #[serde(default)]
    pub stable: bool,
}

pub const FABRIC_META: &str = "https://meta.fabricmc.net/v2";
pub const QUILT_META: &str = "https://meta.quiltmc.org/v3";

/// List available loader versions for a Minecraft version.
pub async fn loader_versions(
    client: &reqwest::Client,
    loader: &Loader,
    mc_version: &str,
) -> Result<Vec<String>, String> {
    let url = match loader {
        Loader::Fabric { .. } => format!("{FABRIC_META}/versions/loader/{mc_version}"),
        Loader::Quilt { .. } => format!("{QUILT_META}/versions/loader/{mc_version}"),
        _ => return Ok(Vec::new()),
    };
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("loader meta: {}", resp.status()));
    }
    let entries: Vec<MetaLoaderEntry> = resp.json().await.map_err(|e| e.to_string())?;
    Ok(entries.into_iter().map(|e| e.loader.version).collect())
}

/// Fetch the loader's self-contained profile JSON (Fabric/Quilt style) for
/// one Minecraft + loader version.
pub async fn fetch_loader_profile(
    client: &reqwest::Client,
    loader: &Loader,
    mc_version: &str,
) -> Result<VersionData, String> {
    let url = match loader {
        Loader::Fabric { version } => {
            format!("{FABRIC_META}/versions/loader/{mc_version}/{version}/profile/json")
        }
        Loader::Quilt { version } => {
            format!("{QUILT_META}/versions/loader/{mc_version}/{version}/profile/json")
        }
        _ => return Err("unsupported loader".to_string()),
    };
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("loader profile: {}", resp.status()));
    }
    let json: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    VersionData::from_json(&json).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Forge / NeoForge (installer-based)
// ---------------------------------------------------------------------------

pub const FORGE_MAVEN: &str = "https://maven.minecraftforge.net/net/minecraftforge/forge";
pub const NEOFORGE_MAVEN: &str = "https://maven.neoforged.net/releases/net/neoforged/neoforge";

/// List Forge versions for one Minecraft version, from the maven metadata.
pub async fn forge_versions(client: &reqwest::Client, mc_version: &str) -> Result<Vec<String>, String> {
    let url = format!("{FORGE_MAVEN}/maven-metadata.xml");
    let text = client.get(&url).send().await.map_err(|e| e.to_string())?.text().await.map_err(|e| e.to_string())?;
    let prefix = format!("{mc_version}-forge-");
    let mut out = Vec::new();
    for capture in text.split(&format!("<version>{prefix}")).skip(1) {
        if let Some(end) = capture.find("</version>") {
            out.push(format!("{prefix}{}", &capture[..end]));
        }
    }
    Ok(out.into_iter().rev().take(15).collect())
}

/// List NeoForge versions for one Minecraft version (e.g. 1.20.4 -> 20.4.x).
pub async fn neoforge_versions(client: &reqwest::Client, mc_version: &str) -> Result<Vec<String>, String> {
    let mc_num: String = mc_version
        .split('.')
        .filter(|p| p.chars().all(|c| c.is_ascii_digit()))
        .collect::<Vec<_>>()
        .join(".");
    let url = format!("{NEOFORGE_MAVEN}/maven-metadata.xml");
    let text = client.get(&url).send().await.map_err(|e| e.to_string())?.text().await.map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for capture in text.split("<version>").skip(1) {
        if let Some(end) = capture.find("</version>") {
            let v = &capture[..end];
            if v.starts_with(&format!("{mc_num}.")) {
                out.push(v.to_string());
            }
        }
    }
    Ok(out.into_iter().rev().take(15).collect())
}

fn installer_url(loader: &Loader) -> Option<(String, String)> {
    match loader {
        Loader::Forge { version } => Some((
            format!("{FORGE_MAVEN}/{version}/forge-{version}-installer.jar"),
            version.clone(),
        )),
        Loader::NeoForge { version } => Some((
            format!("{NEOFORGE_MAVEN}/{version}/neoforge-{version}-installer.jar"),
            version.clone(),
        )),
        _ => None,
    }
}

/// Install Forge/NeoForge into `mc_dir` by running the official installer
/// headlessly, then parse the generated version JSON. `java_path` is used to
/// run the installer.
pub async fn ensure_forge_installed(
    java_path: &std::path::Path,
    loader: &Loader,
    mc_version: &str,
    mc_dir: &std::path::Path,
    downloader: &crate::net::Downloader,
) -> Result<VersionData, String> {
    // Already installed?
    if let Ok(vd) = read_forge_version_json(loader, mc_dir, mc_version) {
        return Ok(vd);
    }

    let Some((url, full_version)) = installer_url(loader) else {
        return Err("not a forge loader".to_string());
    };
    let installer_path = mc_dir.join(format!("{full_version}-installer.jar"));
    downloader
        .download(&crate::net::DownloadRequest::new(url, &installer_path))
        .await
        .map_err(|e| format!("installer download: {e}"))?;

    // The installer wants a vanilla install layout: versions/<mc>/<mc>.jar.
    let versions_dir = mc_dir.join("versions").join(mc_version);
    tokio::fs::create_dir_all(&versions_dir)
        .await
        .map_err(|e| e.to_string())?;
    // The vanilla jar location is resolved by the caller (Launcher places it
    // in the cache); link it in if absent.
    let client_jar = versions_dir.join(format!("{mc_version}.jar"));
    if !client_jar.exists() {
        return Err(format!(
            "vanilla client jar not found at {}; run a vanilla launch once first",
            client_jar.display()
        ));
    }

    let output = tokio::process::Command::new(java_path)
        .arg("-jar")
        .arg(&installer_path)
        .arg("--installClient")
        .arg(mc_dir)
        .output()
        .await
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "forge installer failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    read_forge_version_json(loader, mc_dir, mc_version)
}

/// Parse the version JSON the installer wrote into versions/<id>/.
pub fn read_forge_version_json(
    loader: &Loader,
    mc_dir: &std::path::Path,
    _mc_version: &str,
) -> Result<VersionData, String> {
    let dir_name = match loader {
        Loader::Forge { version } => version.clone(),
        Loader::NeoForge { version } => version.clone(),
        _ => return Err("not a forge loader".to_string()),
    };
    let json_path = mc_dir
        .join("versions")
        .join(&dir_name)
        .join(format!("{dir_name}.json"));
    if !json_path.exists() {
        return Err(format!("installer output not found: {}", json_path.display()));
    }
    let text = std::fs::read_to_string(&json_path).map_err(|e| e.to_string())?;
    let json: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    VersionData::from_json(&json).map_err(|e| e.to_string())
}
