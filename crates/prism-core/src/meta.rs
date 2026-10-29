//! Mojang version metadata: the version manifest, per-version JSON
//! (piston-meta format), and conversion into a launchable profile.
//!
//! Mirrors `launcher/meta/` + `launcher/minecraft/MojangVersionFormat.cpp`.
//! Loader components (Fabric/Quilt/Forge) layer their own version files on
//! top of the Mojang one; see [`super::profile`].

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

pub const VERSION_MANIFEST_V2: &str =
    "https://launchermeta.mojang.com/mc/game/version_manifest_v2.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionManifest {
    pub latest: BTreeMap<String, String>,
    pub versions: Vec<ManifestVersion>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestVersion {
    pub id: String,
    #[serde(rename = "type")]
    pub version_type: String,
    pub url: String,
    #[serde(default)]
    pub time: String,
    #[serde(default)]
    #[serde(rename = "releaseTime")]
    pub release_time: String,
    #[serde(default)]
    pub sha1: Option<String>,
}

impl VersionManifest {
    pub async fn fetch(client: &reqwest::Client) -> Result<VersionManifest, super::net::NetError> {
        let json = crate::net::client_json(client, VERSION_MANIFEST_V2).await?;
        Ok(serde_json::from_value(json)?)
    }

    pub fn find(&self, id: &str) -> Option<&ManifestVersion> {
        self.versions.iter().find(|v| v.id == id)
    }
}

// ---------------------------------------------------------------------------
// piston-meta version JSON
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct VersionData {
    pub id: String,
    pub main_class: String,
    /// Legacy single-string arguments (pre 1.13).
    pub minecraft_arguments: Option<String>,
    /// Modern structured arguments.
    pub arguments: Option<Arguments>,
    pub libraries: Vec<MoLibrary>,
    pub asset_index: Option<MoAssetIndex>,
    /// Legacy `assets` id (used when assetIndex is absent).
    pub assets: Option<String>,
    pub java_version: Option<MoJavaVersion>,
    pub version_type: Option<String>,
    pub release_time: Option<String>,
    pub jar: Option<String>,
    /// `inheritsFrom` chains (rare in practice; resolved by re-fetching).
    pub inherits_from: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Arguments {
    pub game: Vec<ArgValue>,
    pub jvm: Vec<ArgValue>,
}

#[derive(Debug, Clone)]
pub enum ArgValue {
    Str(String),
    /// Conditional argument, applied when every rule allows it.
    Conditional { value: Vec<String>, rules: Vec<Rule> },
}

#[derive(Debug, Clone)]
pub struct Rule {
    pub action: RuleAction,
    pub os: Option<RuleOs>,
    pub features: Option<BTreeMap<String, bool>>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RuleAction {
    Allow,
    Disallow,
}

#[derive(Debug, Clone, Default)]
pub struct RuleOs {
    pub name: Option<String>,
    pub arch: Option<String>,
}

#[derive(Debug, Clone)]
pub struct MoLibrary {
    /// Gradle specifier, e.g. `com.mojang:brigadier:1.0.18`.
    pub name: String,
    pub downloads: Option<MoDownloads>,
    pub rules: Vec<Rule>,
    /// Base URL for older versions that use the `url` field instead of full
    /// download info.
    pub url: Option<String>,
    pub natives: Option<BTreeMap<String, String>>,
}

#[derive(Debug, Clone, Default)]
pub struct MoDownloads {
    pub artifact: Option<MoArtifact>,
    pub classifiers: BTreeMap<String, MoArtifact>,
}

#[derive(Debug, Clone, Default)]
pub struct MoArtifact {
    pub path: Option<String>,
    pub url: String,
    pub sha1: Option<String>,
    pub size: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct MoAssetIndex {
    pub id: String,
    pub url: String,
    pub sha1: Option<String>,
    pub total_size: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoJavaVersion {
    pub component: String,
    pub majorVersion: i64,
}

impl VersionData {
    pub fn from_json(json: &serde_json::Value) -> Result<VersionData, serde_json::Error> {
        let mut out = VersionData::default();
        out.id = json["id"].as_str().unwrap_or_default().to_string();
        out.main_class = json["mainClass"].as_str().unwrap_or_default().to_string();
        out.minecraft_arguments = json["minecraftArguments"].as_str().map(String::from);
        out.assets = json["assets"].as_str().map(String::from);
        out.version_type = json["type"].as_str().map(String::from);
        out.release_time = json["releaseTime"].as_str().map(String::from);
        out.jar = json["jar"].as_str().map(String::from);
        out.inherits_from = json["inheritsFrom"].as_str().map(String::from);
        out.java_version = (!json["javaVersion"].is_null()).then(|| {
            serde_json::from_value(json["javaVersion"].clone()).unwrap_or(MoJavaVersion {
                component: "java-runtime-delta".into(),
                majorVersion: 21,
            })
        });
        if let Some(a) = json["assetIndex"].as_object() {
            let get = |k: &str| a.get(k).cloned().unwrap_or_default();
            out.asset_index = Some(MoAssetIndex {
                id: get("id").as_str().unwrap_or_default().to_string(),
                url: get("url").as_str().unwrap_or_default().to_string(),
                sha1: get("sha1").as_str().map(String::from),
                total_size: get("totalSize").as_u64(),
            });
        }
        if let Some(args) = json["arguments"].as_object() {
            let mut arguments = Arguments::default();
            for v in args.get("game").and_then(|v| v.as_array()).unwrap_or(&vec![]) {
                if let Some(a) = parse_arg_value(v) {
                    arguments.game.push(a);
                }
            }
            for v in args.get("jvm").and_then(|v| v.as_array()).unwrap_or(&vec![]) {
                if let Some(a) = parse_arg_value(v) {
                    arguments.jvm.push(a);
                }
            }
            out.arguments = Some(arguments);
        }
        for lib in json["libraries"].as_array().unwrap_or(&vec![]) {
            let name = lib["name"].as_str().unwrap_or_default().to_string();
            let mut downloads = MoDownloads::default();
            if let Some(d) = lib["downloads"].as_object() {
                if let Some(a) = d.get("artifact").and_then(|v| v.as_object()) {
                    downloads.artifact = Some(artifact_from(a));
                }
                if let Some(cls) = d.get("classifiers").and_then(|v| v.as_object()) {
                    for (k, v) in cls {
                        if let Some(v) = v.as_object() {
                            downloads.classifiers.insert(k.clone(), artifact_from(v));
                        }
                    }
                }
            }
            let mut rules = Vec::new();
            for r in lib["rules"].as_array().unwrap_or(&vec![]) {
                if let Some(rule) = parse_rule(r) {
                    rules.push(rule);
                }
            }
            let natives = lib["natives"].as_object().map(|m| {
                m.iter()
                    .map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_string()))
                    .collect()
            });
            out.libraries.push(MoLibrary {
                name,
                downloads: (!downloads.classifiers.is_empty() || downloads.artifact.is_some())
                    .then_some(downloads),
                rules,
                url: lib["url"].as_str().map(String::from),
                natives,
            });
        }
        Ok(out)
    }
}

fn artifact_from(a: &serde_json::Map<String, serde_json::Value>) -> MoArtifact {
    MoArtifact {
        path: a.get("path").and_then(|v| v.as_str()).map(String::from),
        url: a.get("url").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
        sha1: a.get("sha1").and_then(|v| v.as_str()).map(String::from),
        size: a.get("size").and_then(|v| v.as_u64()),
    }
}

fn parse_arg_value(v: &serde_json::Value) -> Option<ArgValue> {
    match v {
        serde_json::Value::String(s) => Some(ArgValue::Str(s.clone())),
        serde_json::Value::Object(o) => {
            let mut value = Vec::new();
            match o.get("value") {
                Some(serde_json::Value::String(s)) => value.push(s.clone()),
                Some(serde_json::Value::Array(arr)) => {
                    for x in arr {
                        if let Some(s) = x.as_str() {
                            value.push(s.to_string());
                        }
                    }
                }
                _ => return None,
            }
            let mut rules = Vec::new();
            for r in o.get("rules").and_then(|v| v.as_array()).unwrap_or(&vec![]) {
                if let Some(rule) = parse_rule(r) {
                    rules.push(rule);
                }
            }
            Some(ArgValue::Conditional { value, rules })
        }
        _ => None,
    }
}

fn parse_rule(r: &serde_json::Value) -> Option<Rule> {
    let action = match r.get("action").and_then(|v| v.as_str())? {
        "allow" => RuleAction::Allow,
        "disallow" => RuleAction::Disallow,
        _ => return None,
    };
    let os = r["os"].as_object().map(|o| RuleOs {
        name: o.get("name").and_then(|v| v.as_str()).map(String::from),
        arch: o.get("arch").and_then(|v| v.as_str()).map(String::from),
    });
    let features = r["features"].as_object().map(|f| {
        f.iter()
            .map(|(k, v)| (k.clone(), v.as_bool().unwrap_or(false)))
            .collect()
    });
    Some(Rule { action, os, features })
}

/// Current OS name in Mojang manifest terms.
pub fn os_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "osx"
    } else {
        "linux"
    }
}

pub fn os_arch() -> &'static str {
    if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "unknown"
    }
}

/// Evaluate a rule list against the current platform. An empty list allows.
/// Semantics: every `allow` rule must pass, `disallow` rules veto.
pub fn rules_allow(rules: &[Rule], features: &BTreeMap<String, bool>) -> bool {
    if rules.is_empty() {
        return true;
    }
    let mut allowed = false;
    let mut vetoed = false;
    for rule in rules {
        let rule_passes = match (&rule.os, &rule.features) {
            (Some(os), _) => {
                let name_ok = os
                    .name
                    .as_deref()
                    .map(|n| n == os_name())
                    .unwrap_or(true);
                let arch_ok = os
                    .arch
                    .as_deref()
                    .map(|a| a == os_arch())
                    .unwrap_or(true);
                name_ok && arch_ok
            }
            (None, Some(features_req)) => features_req
                .iter()
                .all(|(k, v)| features.get(k).copied().unwrap_or(false) == *v),
            (None, None) => true,
        };
        match rule.action {
            RuleAction::Allow => {
                if rule_passes {
                    allowed = true;
                }
            }
            RuleAction::Disallow => {
                if rule_passes {
                    vetoed = true;
                }
            }
        }
    }
    allowed && !vetoed
}

/// Split a Gradle specifier into (group, artifact, version, classifier).
pub fn parse_gradle_specifier(spec: &str) -> (String, String, String, Option<String>) {
    let mut parts = spec.split(':');
    let group = parts.next().unwrap_or_default().to_string();
    let artifact = parts.next().unwrap_or_default().to_string();
    let rest = parts.next().unwrap_or_default().to_string();
    let (version, classifier) = match rest.split_once('@') {
        Some((v, ext)) => (v.to_string(), Some(format!("@{ext}"))),
        None => match rest.split_once('-') {
            // Heuristic for classifier; gradle specifiers with classifiers
            // look like group:artifact:version:classifier
            _ => {
                let mut it = spec.split(':');
                let _g = it.next();
                let _a = it.next();
                let v = it.next().unwrap_or_default().to_string();
                let c = it.next().map(String::from);
                (v, c)
            }
        },
    };
    (group, artifact, version, classifier)
}

/// Maven repo path for a gradle specifier:
/// `group/artifact/version/artifact-version[-classifier].ext`
pub fn maven_path(spec: &str) -> PathBuf {
    let (group, artifact, version, classifier) = parse_gradle_specifier(spec);
    let mut path = PathBuf::from(group.replace('.', "/"));
    path.push(&artifact);
    path.push(&version);
    let ext = classifier
        .as_deref()
        .filter(|c| c.starts_with('@'))
        .map(|c| c.to_string())
        .unwrap_or_else(|| ".jar".to_string());
    let classifier = classifier.filter(|c| !c.starts_with('@'));
    let file = match classifier {
        Some(c) => format!("{artifact}-{version}-{c}{ext}"),
        None => format!("{artifact}-{version}{ext}"),
    };
    path.push(file);
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maven_paths() {
        assert_eq!(
            maven_path("com.mojang:brigadier:1.0.18"),
            PathBuf::from("com/mojang/brigadier/1.0.18/brigadier-1.0.18.jar")
        );
        assert_eq!(
            maven_path("org.lwjgl.lwjgl:lwjgl-platform:2.9.4-nightly-20150209:natives-linux"),
            PathBuf::from(
                "org/lwjgl/lwjgl/lwjgl-platform/2.9.4-nightly-20150209/lwjgl-platform-2.9.4-nightly-20150209-natives-linux.jar"
            )
        );
    }

    #[test]
    fn empty_rules_allow() {
        assert!(rules_allow(&[], &Default::default()));
    }

    #[test]
    fn os_rules() {
        let rules = vec![Rule {
            action: RuleAction::Allow,
            os: Some(RuleOs { name: Some("linux".into()), arch: None }),
            features: None,
        }];
        assert!(rules_allow(&rules, &Default::default()));
        let rules = vec![Rule {
            action: RuleAction::Allow,
            os: Some(RuleOs { name: Some("windows".into()), arch: None }),
            features: None,
        }];
        assert!(!rules_allow(&rules, &Default::default()));
    }
}
