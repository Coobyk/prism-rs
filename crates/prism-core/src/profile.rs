//! Resolved launch profile: the merged, platform-evaluated view of a version
//! (Mojang JSON, later extended with loader components). Mirrors
//! PrismLauncher's `LaunchProfile` + `LibraryUtils`.

use crate::meta::{self, MoLibrary, RuleAction, VersionData};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct ResolvedLibrary {
    /// Gradle specifier.
    pub spec: String,
    /// Maven-relative path under the libraries directory.
    pub path: PathBuf,
    /// Absolute download URL, when known.
    pub url: Option<String>,
    pub sha1: Option<String>,
    /// This jar is a natives jar: extracted into the natives dir, not put on
    /// the classpath.
    pub native: bool,
}

#[derive(Debug, Clone, Default)]
pub struct LaunchProfile {
    pub minecraft_version: String,
    pub version_type: Option<String>,
    pub main_class: String,
    /// Legacy `minecraftArguments` string (pre 1.13).
    pub legacy_arguments: Option<String>,
    /// Modern structured arguments (1.13+).
    pub modern_arguments: Option<meta::Arguments>,
    /// Assets index id (`assets` / `assetIndex.id`).
    pub assets_id: String,
    /// (url, sha1) of the asset index to fetch.
    pub asset_index: Option<(String, Option<String>)>,
    pub java_component: Option<String>,
    pub java_major: Option<i64>,
    /// Classpath libraries (natives jars excluded).
    pub libraries: Vec<ResolvedLibrary>,
    /// Natives jars to extract.
    pub natives: Vec<ResolvedLibrary>,
}

pub const DEFAULT_JVM_ARGUMENTS: &[&str] = &[
    "-Djava.library.path=${natives_directory}",
    "-Djna.tmpdir=${natives_directory}",
    "-Dorg.lwjgl.system.SharedLibraryExtractPath=${natives_directory}",
    "-Dio.netty.native.workdir=${natives_directory}",
    "-Dminecraft.launcher.brand=${launcher_name}",
    "-Dminecraft.launcher.version=${launcher_version}",
    "-cp",
    "${classpath}",
];

pub const LEGACY_GAME_ARGUMENTS: &str = "--username ${auth_player_name} --version ${version_name} --gameDir ${game_directory} --assetsDir ${game_assets} --uuid ${auth_uuid} --accessToken ${auth_access_token} --userProperties ${user_properties} --userType ${user_type}";

impl LaunchProfile {
    /// Resolve a Mojang version JSON into a platform-evaluated profile.
    pub fn from_mojang(vd: &VersionData) -> LaunchProfile {
        let features = BTreeMap::new(); // no optional features (demo/resolution) enabled
        let mut libraries = Vec::new();
        let mut natives = Vec::new();

        for lib in &vd.libraries {
            if !meta::rules_allow(&lib.rules, &features) {
                continue;
            }
            push_library(lib, &mut libraries, &mut natives);
        }

        LaunchProfile {
            minecraft_version: vd.id.clone(),
            version_type: vd.version_type.clone(),
            main_class: vd.main_class.clone(),
            legacy_arguments: vd.minecraft_arguments.clone(),
            modern_arguments: vd.arguments.clone(),
            assets_id: vd
                .asset_index
                .as_ref()
                .map(|a| a.id.clone())
                .or_else(|| vd.assets.clone())
                .unwrap_or_else(|| "legacy".to_string()),
            asset_index: vd
                .asset_index
                .as_ref()
                .map(|a| (a.url.clone(), a.sha1.clone())),
            java_component: vd.java_version.as_ref().map(|j| j.component.clone()),
            java_major: vd.java_version.as_ref().map(|j| j.majorVersion),
            libraries,
            natives,
        }
    }

    /// Final classpath entries (library dir prefix added by the caller).
    pub fn classpath(&self) -> Vec<PathBuf> {
        self.libraries.iter().map(|l| l.path.clone()).collect()
    }

    pub fn is_legacy(&self) -> bool {
        self.legacy_arguments.is_some() || self.main_class.contains("minecraft.server.MinecraftServer")
    }

    /// Overlay a loader profile (Fabric/Quilt `profile/json`): its main
    /// class, libraries and arguments are applied on top of the Mojang base.
    pub fn apply_loader(&mut self, loader_vd: &VersionData) {
        let features = BTreeMap::new();
        self.main_class = loader_vd.main_class.clone();

        for lib in &loader_vd.libraries {
            if meta::rules_allow(&lib.rules, &features) {
                push_library(lib, &mut self.libraries, &mut self.natives);
            }
        }

        // Merge the loader's structured arguments after the base ones.
        if let Some(loader_args) = &loader_vd.arguments {
            let base = self.modern_arguments.get_or_insert_with(Default::default);
            base.jvm.extend(loader_args.jvm.iter().cloned());
            base.game.extend(loader_args.game.iter().cloned());
        }
        // Loader profiles never carry legacy string args.
    }
}

fn push_library(lib: &MoLibrary, libraries: &mut Vec<ResolvedLibrary>, natives: &mut Vec<ResolvedLibrary>) {
    let base = lib
        .downloads
        .as_ref()
        .and_then(|d| d.artifact.as_ref())
        .map(|a| (a.path.clone(), Some(a.url.clone()), a.sha1.clone()));

    if let Some((path, url, sha1)) = base {
        let path = path.map(PathBuf::from).unwrap_or_else(|| meta::maven_path(&lib.name));
        libraries.push(ResolvedLibrary {
            spec: lib.name.clone(),
            path,
            url,
            sha1,
            native: false,
        });
    } else {
        // Older format: only a specifier + repo base url.
        let path = meta::maven_path(&lib.name);
        let url = lib.url.as_deref().map(|base| {
            let base = base.trim_end_matches('/');
            format!("{base}/{}", path.to_string_lossy().replace('\\', "/"))
        });
        libraries.push(ResolvedLibrary {
            spec: lib.name.clone(),
            path,
            url,
            sha1: None,
            native: false,
        });
    }

    // Natives classifier for the current platform.
    if let Some(natives_map) = &lib.natives {
        let classifier = natives_map.get(meta::os_name()).map(|c| {
            c.replace("${arch}", if cfg!(target_arch = "x86_64") { "64" } else { "32" })
        });
        if let Some(classifier) = classifier {
            let spec = format!("{}:{}", lib.name, classifier);
            let path = meta::maven_path(&spec);
            let url = lib
                .downloads
                .as_ref()
                .and_then(|d| d.classifiers.get(&classifier))
                .map(|a| (a.url.clone(), a.sha1.clone()));
            let (url, sha1) = match url {
                Some((u, s)) => (Some(u), s),
                None => (
                    lib.url.as_deref().map(|base| {
                        let base = base.trim_end_matches('/');
                        format!("{base}/{}", path.to_string_lossy().replace('\\', "/"))
                    }),
                    None,
                ),
            };
            natives.push(ResolvedLibrary { spec, path, url, sha1, native: true });
        }
    }
}

/// Rules helper re-exported for loader components that carry their own rules.
pub fn rules_allow(rules: &[crate::meta::Rule], features: &BTreeMap<String, bool>) -> bool {
    // Special-case: never let a loader drop required libraries by rule.
    if rules.is_empty() {
        return true;
    }
    let mut allowed = false;
    let mut vetoed = false;
    for rule in rules {
        let passes = match (&rule.os, &rule.features) {
            (Some(os), _) => {
                let name_ok = os.name.as_deref().map(|n| n == meta::os_name()).unwrap_or(true);
                let arch_ok = os.arch.as_deref().map(|a| a == meta::os_arch()).unwrap_or(true);
                name_ok && arch_ok
            }
            (None, Some(f)) => f.iter().all(|(k, v)| features.get(k).copied().unwrap_or(false) == *v),
            (None, None) => true,
        };
        match rule.action {
            RuleAction::Allow => allowed |= passes,
            RuleAction::Disallow => vetoed |= passes,
        }
    }
    allowed && !vetoed
}
