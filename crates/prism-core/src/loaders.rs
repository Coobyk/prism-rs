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
