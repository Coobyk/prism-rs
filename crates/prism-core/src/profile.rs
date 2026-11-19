//! Resolved launch profile: the merged, platform-evaluated view of a version
//! (Mojang JSON, later extended with loader components). Mirrors
//! PrismLauncher's `LaunchProfile` + `LibraryUtils`.

use crate::meta::{self, MoLibrary, VersionData};
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
    pub(crate) client_override: Option<PathBuf>,
    pub(crate) extra_classpath: Vec<PathBuf>,
    pub(crate) natives_override: Option<PathBuf>,
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
    pub fn supports_quick_play_singleplayer(&self) -> bool {
        self.has_game_feature("is_quick_play_singleplayer")
    }

    pub fn supports_quick_play_multiplayer(&self) -> bool {
        self.has_game_feature("is_quick_play_multiplayer")
    }

    fn has_game_feature(&self, feature: &str) -> bool {
        self.modern_arguments.as_ref().is_some_and(|args| {
            args.game.iter().any(|arg| {
                matches!(arg, meta::ArgValue::Conditional { rules, .. } if rules.iter().any(|rule| {
                    rule.features.as_ref().is_some_and(|features| features.contains_key(feature))
                }))
            })
        })
    }

    /// Resolve a Mojang version JSON into a platform-evaluated profile.
    pub fn from_mojang(vd: &VersionData) -> LaunchProfile {
        Self::from_version(vd, meta::rules_allow)
    }

    fn from_version(
        vd: &VersionData,
        rules_allow: impl Fn(&[meta::Rule], &BTreeMap<String, bool>) -> bool,
    ) -> LaunchProfile {
        let features = BTreeMap::new(); // no optional features (demo/resolution) enabled
        let mut libraries = Vec::new();
        let mut natives = Vec::new();

        for lib in &vd.libraries {
            if !rules_allow(&lib.rules, &features) {
                continue;
            }
            push_library(lib, &mut libraries, &mut natives);
        }

        LaunchProfile {
            client_override: None,
            extra_classpath: Vec::new(),
            natives_override: None,
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

    /// Java majors this version runs on, most preferred first. Like
    /// meta.prismlauncher.org, Java 16 versions (1.17) also accept Java 17.
    pub fn compatible_java_majors(&self) -> Vec<u32> {
        match self.java_major.and_then(|major| u32::try_from(major).ok()) {
            Some(16) => vec![16, 17],
            Some(major) => vec![major],
            None => Vec::new(),
        }
    }

    /// Final classpath entries (library dir prefix added by the caller).
    pub fn classpath(&self) -> Vec<PathBuf> {
        self.libraries.iter().map(|l| l.path.clone()).collect()
    }

    pub fn lwjgl_family(&self) -> Option<&'static str> {
        self.libraries.iter().find_map(|lib| match lib.spec.split(':').next() {
            Some("org.lwjgl") => Some(crate::version_components::LWJGL3),
            Some("org.lwjgl.lwjgl") => Some(crate::version_components::LWJGL),
            _ => None,
        })
    }

    pub fn apply_lwjgl(&mut self, replacement: &VersionData) -> std::io::Result<()> {
        let old_family = self.lwjgl_family();
        let replacement = Self::from_version(replacement, meta::prism_rules_allow);
        if old_family.is_none() || replacement.lwjgl_family() != old_family {
            return Err(std::io::Error::other("LWJGL metadata does not match the game's LWJGL family or platform."));
        }
        let replaced = |library: &ResolvedLibrary| {
            let group = library.spec.split(':').next().unwrap_or_default();
            matches!(group, "org.lwjgl" | "org.lwjgl.lwjgl")
                || (old_family == Some(crate::version_components::LWJGL)
                    && matches!(group, "net.java.jinput" | "net.java.jutils"))
        };
        self.libraries.retain(|library| !replaced(library));
        self.natives.retain(|library| !replaced(library));
        self.libraries.extend(replacement.libraries);
        self.natives.extend(replacement.natives);
        Ok(())
    }

    pub fn is_legacy(&self) -> bool {
        self.legacy_arguments.is_some() || self.main_class.contains("minecraft.server.MinecraftServer")
    }

    /// Overlay a loader profile: its main
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
        if let Some(arguments) = &loader_vd.minecraft_arguments {
            self.legacy_arguments = Some(arguments.clone());
            if let Some(modern) = &mut self.modern_arguments {
                modern.game = arguments
                    .split_whitespace()
                    .map(|arg| meta::ArgValue::Str(arg.into()))
                    .collect();
            }
        }
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
    } else if lib.natives.is_none() {
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
pub use meta::rules_allow;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installer_forge_overlay_preserves_the_vanilla_guava() {
        let vanilla = VersionData::from_json(&serde_json::json!({
            "libraries": [
                {"name": "com.google.guava:guava:33.3.1-jre"},
                {"name": "com.google.guava:guava:15.0:classified"},
                {"name": "org.example:unchanged:1"}
            ]
        }))
        .unwrap();
        let loader = VersionData::from_json(&serde_json::json!({
            "libraries": [
                {"name": "com.google.guava:guava:32.1.2-jre"},
                {"name": "org.example:unchanged:2",
                    "rules": [{"action":"disallow"}]}
            ]
        }))
        .unwrap();
        let mut profile = LaunchProfile::from_mojang(&vanilla);
        profile.apply_loader(&loader);
        let specs: Vec<_> = profile.libraries.iter().map(|lib| lib.spec.as_str()).collect();
        assert_eq!(
            specs,
            [
                "com.google.guava:guava:33.3.1-jre",
                "com.google.guava:guava:15.0:classified",
                "org.example:unchanged:1",
                "com.google.guava:guava:32.1.2-jre"
            ]
        );
    }
}
