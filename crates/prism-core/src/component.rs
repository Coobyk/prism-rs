use crate::{
    meta,
    profile::{LaunchProfile, ResolvedLibrary},
    version_components::version_id,
};
use serde_json::Value;
use std::{
    fs, io,
    path::{Path, PathBuf},
};

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

pub fn uid(value: &str) -> io::Result<&str> {
    if value.is_empty()
        || value.trim() != value
        || value == "."
        || value == ".."
        || !value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
    {
        return Err(invalid(
            "Component UID must contain only letters, numbers, dots, hyphens or underscores.",
        ));
    }
    Ok(value)
}

#[derive(Clone, Debug)]
pub struct Component(pub Value);

impl Component {
    pub fn parse(text: &str, expected: Option<&str>) -> io::Result<Self> {
        let value: Value = serde_json::from_str(text)?;
        let id = value["uid"]
            .as_str()
            .ok_or_else(|| invalid("Component requires a uid."))?;
        uid(id)?;
        if expected.is_some_and(|expected| expected != id) {
            return Err(invalid("The component UID cannot be changed."));
        }
        if value["formatVersion"] != 1 {
            return Err(invalid("Component formatVersion must be 1."));
        }
        Ok(Self(value))
    }

    pub fn id(&self) -> &str {
        self.0["uid"].as_str().unwrap()
    }

    pub fn validate_launch(&self) -> io::Result<()> {
        const FIELDS: &[&str] = &[
            "uid",
            "name",
            "version",
            "formatVersion",
            "order",
            "requires",
            "conflicts",
            "volatile",
            "releaseTime",
            "time",
            "type",
            "id",
            "minimumLauncherVersion",
            "complianceLevel",
            "mainClass",
            "minecraftArguments",
            "arguments",
            "libraries",
            "mavenFiles",
            "mainJar",
            "jarMods",
            "+jarMods",
            "+agents",
            "+jvmArgs",
            "downloads",
            "assetIndex",
            "assets",
            "javaVersion",
        ];
        for key in self.0.as_object().unwrap().keys() {
            if !FIELDS.contains(&key.as_str()) {
                return Err(invalid(format!(
                    "Component {}: unsupported field {key}",
                    self.id()
                )));
            }
        }
        for field in ["libraries", "mavenFiles", "jarMods", "+jarMods", "+agents"] {
            if let Some(value) = self.0.get(field) {
                let entries = value
                    .as_array()
                    .ok_or_else(|| invalid(format!("{field} must be an array")))?;
                for entry in entries {
                    validate_library(entry)?;
                }
            }
        }
        if let Some(lib) = self.0.get("mainJar") {
            validate_library(lib)?;
        }
        for key in [
            "mainClass",
            "minecraftArguments",
            "assets",
            "name",
            "version",
        ] {
            if self.0.get(key).is_some_and(|v| !v.is_string()) {
                return Err(invalid(format!("{key} must be a string")));
            }
        }
        if let Some(args) = self.0.get("+jvmArgs") {
            if !args
                .as_array()
                .is_some_and(|a| a.iter().all(Value::is_string))
            {
                return Err(invalid("+jvmArgs must be an array of strings"));
            }
        }
        Ok(())
    }

    pub fn local_artifacts(&self, root: &Path) -> io::Result<Vec<PathBuf>> {
        let mut result = Vec::new();
        for key in [
            "libraries",
            "mavenFiles",
            "jarMods",
            "+jarMods",
            "+agents",
            "mainJar",
        ] {
            let entries: Vec<&Value> = if key == "mainJar" {
                self.0.get(key).into_iter().collect()
            } else {
                self.0[key].as_array().into_iter().flatten().collect()
            };
            for entry in entries {
                if entry["MMC-hint"] == "local" {
                    result.push(library(entry, root, key.contains("jarMods"))?.path);
                }
            }
        }
        Ok(result)
    }

    pub fn apply(&self, profile: &mut LaunchProfile, root: &Path) -> io::Result<()> {
        self.validate_launch()?;
        let data = meta::VersionData::from_json(&self.0)?;
        if !data.main_class.is_empty() {
            profile.main_class = data.main_class.clone();
        }
        if data.minecraft_arguments.is_some() {
            profile.legacy_arguments = data.minecraft_arguments;
        }
        if let Some(args) = data.arguments {
            let target = profile
                .modern_arguments
                .get_or_insert_with(Default::default);
            target.jvm.extend(args.jvm);
            target.game.extend(args.game);
        }
        if let Some(java) = data.java_version {
            profile.java_component = Some(java.component);
            profile.java_major = Some(java.majorVersion);
        }
        if self.id() == crate::version_components::MINECRAFT {
            profile.minecraft_version = self.0["version"]
                .as_str()
                .filter(|s| !s.is_empty())
                .unwrap_or(&profile.minecraft_version)
                .to_owned();
            if let Some(index) = data.asset_index {
                profile.assets_id = index.id;
                profile.asset_index = Some((index.url, index.sha1));
            } else if let Some(assets) = data.assets {
                profile.assets_id = assets;
            }
            profile.version_type = data.version_type;
        }
        for arg in self.0["+jvmArgs"].as_array().into_iter().flatten() {
            profile
                .component_jvm_args
                .push(arg.as_str().unwrap().to_owned());
        }
        for raw in self.0["libraries"].as_array().into_iter().flatten() {
            if !meta::rules_allow(
                &meta::VersionData::from_json(&serde_json::json!({"libraries":[raw]}))?.libraries
                    [0]
                .rules,
                &Default::default(),
            ) {
                continue;
            }
            let mut resolved = LaunchProfile::from_mojang(&meta::VersionData::from_json(
                &serde_json::json!({"libraries":[raw]}),
            )?);
            if raw["MMC-hint"] == "local" {
                if !resolved.natives.is_empty() {
                    return Err(invalid("Local native libraries are not supported."));
                }
                resolved.libraries = vec![library(raw, root, false)?];
            }
            for lib in resolved.libraries {
                let name: Vec<_> = lib.spec.split(':').take(2).collect();
                profile
                    .libraries
                    .retain(|old| old.spec.split(':').take(2).collect::<Vec<_>>() != name);
                profile.libraries.push(lib);
            }
            profile.natives.extend(resolved.natives);
        }
        for raw in self.0["mavenFiles"].as_array().into_iter().flatten() {
            profile.component_artifacts.push(library(raw, root, false)?);
        }
        for key in ["jarMods", "+jarMods"] {
            if key == "+jarMods" && self.0.get("jarMods").is_some() {
                continue;
            }
            for raw in self.0[key].as_array().into_iter().flatten() {
                let lib = library(raw, root, true)?;
                profile.component_jar_mods.push(lib.clone());
                profile.component_artifacts.push(lib);
            }
        }
        if let Some(raw) = self.0.get("mainJar") {
            let lib = library(raw, root, false)?;
            profile.component_main_jar = Some(lib.clone());
            profile.component_artifacts.push(lib);
        }
        for raw in self.0["+agents"].as_array().into_iter().flatten() {
            let lib = library(raw, root, false)?;
            let argument = raw
                .get("argument")
                .map(|v| {
                    v.as_str()
                        .ok_or_else(|| invalid("Agent argument must be a string"))
                })
                .transpose()?
                .unwrap_or("")
                .to_owned();
            profile.agents.push((lib.clone(), argument));
            profile.component_artifacts.push(lib);
        }
        Ok(())
    }
}

fn validate_library(value: &Value) -> io::Result<()> {
    let name = value["name"]
        .as_str()
        .ok_or_else(|| invalid("Library requires a Maven name"))?;
    let parts: Vec<_> = name.split(':').collect();
    if !(3..=4).contains(&parts.len()) || parts.iter().any(|part| version_id(part).is_err()) {
        return Err(invalid("Invalid library Maven name"));
    }
    for key in value.as_object().unwrap().keys() {
        if ![
            "name",
            "url",
            "downloads",
            "rules",
            "natives",
            "extract",
            "MMC-hint",
            "MMC-filename",
            "MMC-displayname",
            "MMC-absoluteUrl",
            "argument",
        ]
        .contains(&key.as_str())
        {
            return Err(invalid(format!("Unsupported library field {key}")));
        }
    }
    if let Some(hint) = value.get("MMC-hint") {
        if hint != "local" {
            return Err(invalid("Only MMC-hint local is supported"));
        }
    }
    if let Some(file) = value.get("MMC-filename") {
        let file = file
            .as_str()
            .ok_or_else(|| invalid("Invalid MMC-filename"))?;
        if !crate::worlds::plain_name(file) || file.contains('\\') {
            return Err(invalid("Unsafe MMC-filename"));
        }
    }
    if let Some(downloads) = value.get("downloads") {
        for artifact in downloads.get("artifact").into_iter().chain(
            downloads["classifiers"]
                .as_object()
                .into_iter()
                .flat_map(|m| m.values()),
        ) {
            if let Some(path) = artifact.get("path") {
                let path = path
                    .as_str()
                    .ok_or_else(|| invalid("Invalid artifact path"))?;
                if path.contains('\\')
                    || !Path::new(path)
                        .components()
                        .all(|c| matches!(c, std::path::Component::Normal(_)))
                {
                    return Err(invalid("Unsafe artifact path"));
                }
            }
        }
    }
    Ok(())
}

pub fn library(raw: &Value, root: &Path, jar_mod: bool) -> io::Result<ResolvedLibrary> {
    validate_library(raw)?;
    let spec = raw["name"].as_str().unwrap();
    let path = meta::maven_path(spec);
    let filename = raw["MMC-filename"]
        .as_str()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(path.file_name().unwrap()));
    let local = raw["MMC-hint"] == "local";
    let resolved = if local {
        root.join(if jar_mod { "jarmods" } else { "libraries" })
            .join(filename)
    } else {
        raw["downloads"]["artifact"]["path"]
            .as_str()
            .map(PathBuf::from)
            .unwrap_or(path)
    };
    let url = if local {
        None
    } else {
        raw["MMC-absoluteUrl"]
            .as_str()
            .or(raw["downloads"]["artifact"]["url"].as_str())
            .map(str::to_owned)
            .or_else(|| {
                raw["url"].as_str().map(|url| {
                    format!(
                        "{}/{}",
                        url.trim_end_matches('/'),
                        resolved.to_string_lossy()
                    )
                })
            })
    };
    Ok(ResolvedLibrary {
        spec: spec.into(),
        path: resolved,
        url,
        sha1: raw["downloads"]["artifact"]["sha1"]
            .as_str()
            .map(str::to_owned),
        native: false,
    })
}

pub fn check_local(path: &Path) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| invalid("Invalid local artifact path"))?;
    if !fs::symlink_metadata(parent)?.is_dir() || !fs::symlink_metadata(path)?.is_file() {
        return Err(invalid(format!(
            "Local artifact must be a regular file: {}",
            path.display()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(value: Value) -> io::Result<Component> {
        Component::parse(&value.to_string(), None)
    }

    #[test]
    fn validates_identity_and_unsupported_launch_fields() {
        for text in ["", "[]", "{}", r#"{"uid":"../bad","formatVersion":1}"#] {
            assert!(Component::parse(text, None).is_err());
        }
        let component = parse(json!({"uid":"test","formatVersion":1,"+tweakers":["bad"]})).unwrap();
        assert!(component
            .validate_launch()
            .unwrap_err()
            .to_string()
            .contains("+tweakers"));
        assert!(Component::parse(r#"{"uid":"test","formatVersion":1}"#, Some("other")).is_err());
        assert!(parse(json!({"uid":"test","formatVersion":1,"libraries":[{"name":"a:b:1","MMC-filename":"../bad"}]})).unwrap().validate_launch().is_err());
    }

    #[test]
    fn local_library_main_jar_mods_agents_and_arguments() {
        let root = Path::new("/instance with spaces");
        let component = parse(json!({"uid":"test","formatVersion":1,"mainClass":"Main",
            "minecraftArguments":"--test", "+jvmArgs":["-Dtest=yes"],
            "libraries":[{"name":"custom:lib:1","MMC-hint":"local","MMC-filename":"lib.jar"}],
            "mainJar":{"name":"custom:customjar:1","MMC-hint":"local"},
            "jarMods":[{"name":"custom:mod:1","MMC-hint":"local","MMC-filename":"mod.jar"}],
            "+agents":[{"name":"custom:agent:1","MMC-hint":"local","MMC-filename":"agent.jar","argument":"hello world"}]
        })).unwrap();
        let mut profile = LaunchProfile::default();
        component.apply(&mut profile, root).unwrap();
        assert_eq!(profile.main_class, "Main");
        assert_eq!(profile.libraries[0].path, root.join("libraries/lib.jar"));
        assert_eq!(
            profile.component_main_jar.unwrap().path,
            root.join("libraries/customjar-1.jar")
        );
        assert_eq!(
            profile.component_jar_mods[0].path,
            root.join("jarmods/mod.jar")
        );
        assert_eq!(profile.agents[0].1, "hello world");
        assert_eq!(profile.component_jvm_args, ["-Dtest=yes"]);
    }

    #[test]
    fn empty_component_and_legacy_jar_mod_alias() {
        let mut profile = LaunchProfile::default();
        parse(json!({"uid":"test","formatVersion":1}))
            .unwrap()
            .apply(&mut profile, Path::new("/instance"))
            .unwrap();
        assert!(profile.agents.is_empty());
        parse(json!({"uid":"test","formatVersion":1,"+jarMods":[{"name":"custom:mod:1","MMC-hint":"local"}]})).unwrap()
            .apply(&mut profile, Path::new("/instance")).unwrap();
        assert_eq!(profile.component_jar_mods.len(), 1);
    }
}
