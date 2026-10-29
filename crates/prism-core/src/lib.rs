//! prism-core: domain logic for prism-rs.
//!
//! Mirrors the structure of PrismLauncher's `launcher/` directory:
//! - [`instance`]: instance model and list scanning (BaseInstance / InstanceList)
//! - [`ini`]: INI file handling for `instance.cfg` and `prismlauncher.cfg`
//! - [`paths`]: launcher data directory layout

pub mod ini;
pub mod instance;
pub mod paths;

pub use paths::Paths;
