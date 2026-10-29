//! prism-core: domain logic for prism-rs.
//!
//! Mirrors the structure of PrismLauncher's `launcher/` directory:
//! - [`instance`]: instance model and list scanning (BaseInstance / InstanceList)
//! - [`ini`]: INI file handling for `instance.cfg` and `prismlauncher.cfg`
//! - [`paths`]: launcher data directory layout

pub mod auth;
pub mod ini;
pub mod instance;
pub mod java;
pub mod launch;
pub mod loaders;
pub mod meta;
pub mod modplatform;
pub mod net;
pub mod news;
pub mod paths;
pub mod profile;
pub mod resources;
pub mod settings;

pub use auth::AccountList;
pub use instance::{Instance, InstanceGroups, InstanceList};
pub use paths::Paths;
