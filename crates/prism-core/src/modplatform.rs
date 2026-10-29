//! Mod platform integrations. Mirrors `launcher/modplatform/`.
//! Modrinth first (api.modrinth.com/v2), CurseForge next.

pub mod flame;
pub mod modrinth;

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ResourceType {
    Mod,
    ResourcePack,
    ShaderPack,
    TexturePack,
    DataPack,
}

impl ResourceType {
    pub fn modrinth_project_type(&self) -> &'static str {
        match self {
            ResourceType::Mod => "mod",
            ResourceType::ResourcePack => "resourcepack",
            ResourceType::ShaderPack => "shader",
            ResourceType::TexturePack => "resourcepack",
            ResourceType::DataPack => "datapack",
        }
    }

    /// The instance subdirectory this resource type installs into.
    pub fn folder(&self) -> &'static str {
        match self {
            ResourceType::Mod => "mods",
            ResourceType::ResourcePack => "resourcepacks",
            ResourceType::ShaderPack => "shaderpacks",
            ResourceType::TexturePack => "texturepacks",
            ResourceType::DataPack => "datapacks",
        }
    }
}

/// Where a downloaded file goes: the instance's game root + folder.
pub fn install_path(game_root: &Path, r#type: ResourceType, file_name: &str) -> PathBuf {
    game_root.join(r#type.folder()).join(file_name)
}

/// A locally installed resource (mod / resource pack / shader pack file).
#[derive(Debug, Clone)]
pub struct LocalResource {
    pub file_name: String,
    pub path: PathBuf,
    pub title: Option<String>,
    pub version: Option<String>,
    pub loader: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum PlatformError {
    #[error("network: {0}")]
    Net(#[from] crate::net::NetError),
    #[error("request failed: {0}")]
    Request(String),
    #[error("http: {0}")]
    Http(#[from] reqwest::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}
