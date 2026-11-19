//! prism-core: domain logic for prism-rs.
//!
//! Mirrors the structure of PrismLauncher's `launcher/` directory:
//! - [`instance`]: instance model and list scanning (BaseInstance / InstanceList)
//! - [`ini`]: INI file handling for `instance.cfg` and `prismlauncher.cfg`
//! - [`paths`]: launcher data directory layout

pub mod auth;
pub mod copy;
pub mod datapacks;
pub mod export;
pub mod horizon;
pub mod import;
pub mod icons;
pub mod ini;
pub mod instance;
pub mod instance_settings;
pub mod java;
pub mod launch;
pub mod loaders;
pub mod logs;
pub mod managed_pack;
pub mod meta;
pub mod mod_list;
pub mod modplatform;
pub mod net;
pub mod news;
pub mod nbt;
pub mod paths;
pub mod profile;
pub mod resources;
pub mod settings;
pub mod servers;
pub mod screenshots;
pub mod version_components;
pub mod worlds;

pub use auth::AccountList;
pub use instance::{Instance, InstanceGroups, InstanceKind, InstanceList};
pub use paths::Paths;
