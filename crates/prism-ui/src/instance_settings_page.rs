use crate::instance_window::InstanceWindow;
use crate::theme::Button;
use gpui_kit::base::Disableable;
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::*;
use prism_core::instance_settings::InstanceSettings;
use prism_core::launch::LaunchSettings;
use prism_core::{Instance, Paths};
use std::collections::BTreeMap;
use std::path::Path;

const GROUP_FIELDS: [&[usize]; 6] = [&[0, 1], &[2], &[3], &[4, 5], &[6, 7, 8], &[]];

#[cfg(test)]
mod tests {
    use super::{save_checked, SettingsForm};
    use prism_core::instance_settings::InstanceSettings;
    use prism_core::launch::LaunchSettings;
    use prism_core::{Instance, Paths};
    use std::collections::BTreeMap;

    fn form() -> SettingsForm {
        SettingsForm {
            overrides: [true; 6],
            loaded_java_path: "/usr/bin/java".into(),
            values: [
                "512",
                "4096",
                "/usr/bin/java",
                "-Dtest=yes",
                "1280",
                "720",
                "echo pre",
                "env TEST=yes",
                "echo post",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
            env: vec![("TEST_VAR".into(), "hello world".into())],
        }
    }

    #[test]
    fn converts_enabled_fields_and_preserves_disabled_raw_values() {
        let mut form = form();
        let raw = form.convert(&InstanceSettings::default()).unwrap();
        assert_eq!(raw.min_memory_mb, Some(512));
        assert_eq!(raw.max_memory_mb, Some(4096));
        assert_eq!(raw.window_width, Some(1280));
        assert_eq!(raw.window_height, Some(720));
        assert_eq!(raw.java_path.as_deref(), Some("/usr/bin/java"));
        assert_eq!(raw.jvm_args.as_deref(), Some("-Dtest=yes"));
        assert_eq!(raw.pre_launch_command.as_deref(), Some("echo pre"));
        assert_eq!(raw.wrapper_command.as_deref(), Some("env TEST=yes"));
        assert_eq!(raw.post_exit_command.as_deref(), Some("echo post"));
        assert_eq!(raw.env.as_ref().unwrap()["TEST_VAR"], "hello world");
        form.overrides = [false; 6];
        form.values = vec![String::new(); 9];
        form.env = vec![(String::new(), String::new())];
        let off = form.convert(&raw).unwrap();
        assert_eq!(off.override_memory, Some(false));
        assert_eq!(off.override_java_location, Some(false));
        assert_eq!(off.override_java_args, Some(false));
        assert_eq!(off.override_window, Some(false));
        assert_eq!(off.override_commands, Some(false));
        assert_eq!(off.override_env, Some(false));
        assert_eq!(off.min_memory_mb, raw.min_memory_mb);
        assert_eq!(off.env, raw.env);
    }

    #[test]
    fn validates_positive_numbers_and_memory_order() {
        for index in [0, 1, 4, 5] {
            for invalid in ["", "0", "-1", "1.5", "abc", "4294967296"] {
                let mut form = form();
                form.values[index] = invalid.into();
                assert!(form
                    .convert(&InstanceSettings::default())
                    .unwrap_err()
                    .contains("positive whole number"));
            }
        }
        let mut form = form();
        form.values[0] = "8192".into();
        assert!(form
            .convert(&InstanceSettings::default())
            .unwrap_err()
            .contains("must not exceed"));
        form.values[0] = "4096".into();
        assert!(form.convert(&InstanceSettings::default()).is_ok());
    }

    #[test]
    fn validates_environment_and_allows_empty_optional_fields() {
        for key in [
            "",
            " ",
            "1START",
            "HAS SPACE",
            "BAD=NAME",
            "NO-DASH",
            "NUL\0",
            "é",
        ] {
            let mut form = form();
            form.env = vec![(key.into(), "value".into())];
            assert!(
                form.convert(&InstanceSettings::default()).is_err(),
                "{key:?}"
            );
        }
        let mut form = form();
        form.env = vec![("SAME".into(), "1".into()), (" SAME ".into(), "2".into())];
        assert!(form
            .convert(&InstanceSettings::default())
            .unwrap_err()
            .contains("Duplicate"));
        form.env = vec![("VALID".into(), "\0".into())];
        assert!(form.convert(&InstanceSettings::default()).is_err());
        form.env = vec![("_VALID_2".into(), "".into())];
        for index in [2, 3, 6, 7, 8] {
            form.values[index].clear();
        }
        assert!(form.convert(&InstanceSettings::default()).is_ok());
        form.env.clear();
        assert_eq!(
            form.convert(&InstanceSettings::default()).unwrap().env,
            Some(BTreeMap::new())
        );
    }

    struct Fixture(Paths);
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "prism-ui-settings-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&root).unwrap();
            Self(Paths { root })
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0.root);
        }
    }

    #[test]
    fn stale_and_deleted_instances_cannot_be_saved() {
        let fixture = Fixture::new();
        let root = fixture.0.instances_dir().join("test");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("instance.cfg"), "name=Test\nUnrelated=before\n").unwrap();
        let baseline = InstanceSettings::load(&root).unwrap();
        let updated = form().convert(&baseline).unwrap();
        let mut cfg = prism_core::ini::IniFile::load(&root.join("instance.cfg")).unwrap();
        cfg.set("Unrelated", "after");
        cfg.save(&root.join("instance.cfg")).unwrap();
        save_checked(&root, &baseline, &updated).unwrap();
        assert_eq!(
            prism_core::ini::IniFile::load(&root.join("instance.cfg"))
                .unwrap()
                .get("Unrelated"),
            Some("after")
        );
        assert!(save_checked(&root, &baseline, &updated)
            .unwrap_err()
            .contains("another window"));
        std::fs::remove_dir_all(&root).unwrap();
        assert!(save_checked(&root, &updated, &updated)
            .unwrap_err()
            .contains("deleted"));
        assert!(!root.exists());
    }

    #[test]
    fn validates_argument_quoting_without_rewriting_it() {
        for index in [3, 7] {
            let mut form = form();
            for value in [
                r#"-Dmessage="hello world""#,
                r"C:\tools\wrapper.exe",
                r#""C:\Program Files\wrapper.exe""#,
                r"C:\tools\",
            ] {
                form.values[index] = value.into();
                let converted = form.convert(&InstanceSettings::default()).unwrap();
                let saved = if index == 3 {
                    converted.jvm_args
                } else {
                    converted.wrapper_command
                };
                assert_eq!(saved.as_deref(), Some(value));
            }
            form.values[index] = "\"unterminated".into();
            assert!(form
                .convert(&InstanceSettings::default())
                .unwrap_err()
                .contains("Unterminated"));
            form.overrides[if index == 3 { 2 } else { 4 }] = false;
            assert!(form.convert(&InstanceSettings::default()).is_ok());
        }
    }

    #[test]
    fn disabled_overrides_restore_saved_locals_after_reopening() {
        let fixture = Fixture::new();
        let root = fixture.0.instances_dir().join("test");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("instance.cfg"), "name=Test\n").unwrap();
        let baseline = InstanceSettings::load(&root).unwrap();
        let mut edited = form();
        edited.values[0] = "768".into();
        edited.values[3] = r#"-Dmessage="hello world""#.into();
        let saved = edited.convert(&baseline).unwrap();
        save_checked(&root, &baseline, &saved).unwrap();
        let inherited = SettingsForm::inherited(&fixture.0);
        let mut draft = SettingsForm::local(&saved, &inherited);
        for group in 0..6 {
            edited.toggle(group, false, &inherited, &mut draft);
        }
        assert_eq!(edited.values, inherited.values);
        assert_eq!(edited.env, inherited.env);
        let disabled = edited.convert(&saved).unwrap();
        save_checked(&root, &saved, &disabled).unwrap();
        let instance = Instance::load(&root).unwrap();
        let raw = InstanceSettings::load(&root).unwrap();
        let mut reopened =
            SettingsForm::resolved(&LaunchSettings::for_instance(&instance, &fixture.0), &raw);
        let mut draft = SettingsForm::local(&raw, &inherited);
        assert_eq!(reopened.values, inherited.values);
        for group in 0..6 {
            reopened.toggle(group, true, &inherited, &mut draft);
        }
        assert_eq!(reopened.convert(&raw).unwrap(), saved);
        reopened.values[0] = "900".into();
        reopened.env.push(("DRAFT".into(), "unsaved".into()));
        for group in [0, 5] {
            reopened.toggle(group, false, &inherited, &mut draft);
            reopened.toggle(group, true, &inherited, &mut draft);
        }
        assert_eq!(reopened.values[0], "900");
        assert_eq!(
            reopened.env.last().unwrap(),
            &("DRAFT".into(), "unsaved".into())
        );
        let mut empty = inherited.clone();
        let mut fallback = SettingsForm::local(&InstanceSettings::default(), &inherited);
        empty.toggle(0, true, &inherited, &mut fallback);
        empty.toggle(5, true, &inherited, &mut fallback);
        assert_eq!(empty.values, inherited.values);
        assert_eq!(empty.env, inherited.env);
    }

    #[test]
    #[cfg(unix)]
    fn unrelated_saves_preserve_legacy_java_selection() {
        let fixture = Fixture::new();
        let root = fixture.0.instances_dir().join("test");
        std::fs::create_dir_all(&root).unwrap();
        for java_path in [None, Some("/bin/echo")] {
            let mut cfg = prism_core::ini::IniFile::default();
            cfg.set("name", "Test");
            cfg.set("OverrideJavaPath", "/bin/sh");
            cfg.set("OverrideMemory", "true");
            if let Some(java_path) = java_path {
                cfg.set("JavaPath", java_path);
            }
            cfg.save(&root.join("instance.cfg")).unwrap();
            let instance = Instance::load(&root).unwrap();
            let before = LaunchSettings::for_instance(&instance, &fixture.0);
            assert_eq!(before.java_path, std::path::Path::new("/bin/sh"));
            assert!(before.java_path_explicit);
            let mut baseline = InstanceSettings::load(&root).unwrap();
            for memory in ["768", "1024"] {
                let mut form = SettingsForm::resolved(
                    &LaunchSettings::for_instance(&instance, &fixture.0),
                    &baseline,
                );
                form.values[0] = memory.into();
                let updated = form.convert(&baseline).unwrap();
                save_checked(&root, &baseline, &updated).unwrap();
                let after = LaunchSettings::for_instance(&instance, &fixture.0);
                assert_eq!(after.java_path, before.java_path);
                assert_eq!(after.java_path_explicit, before.java_path_explicit);
                let saved = prism_core::ini::IniFile::load(&root.join("instance.cfg")).unwrap();
                assert_eq!(saved.get("OverrideJavaLocation"), None);
                assert_eq!(saved.get("OverrideJavaPath"), Some("/bin/sh"));
                assert_eq!(saved.get("JavaPath"), java_path);
                assert_eq!(saved.get("AutomaticJava"), None);
                baseline = InstanceSettings::load(&root).unwrap();
            }

            let mut form = SettingsForm::resolved(&before, &baseline);
            form.values[2] = "/bin/echo".into();
            let updated = form.convert(&baseline).unwrap();
            save_checked(&root, &baseline, &updated).unwrap();
            let after = LaunchSettings::for_instance(&instance, &fixture.0);
            assert_eq!(after.java_path, std::path::Path::new("/bin/echo"));
            assert!(after.java_path_explicit);
            let saved = InstanceSettings::load(&root).unwrap();
            assert_eq!(saved.override_java_location, Some(true));
            assert_eq!(saved.legacy_java_path, None);
            assert_eq!(saved.automatic_java, Some(false));
        }
    }

    #[test]
    #[cfg(unix)]
    fn unrelated_saves_preserve_automatic_java_but_manual_edits_disable_it() {
        let fixture = Fixture::new();
        let root = fixture.0.instances_dir().join("test");
        std::fs::create_dir_all(&root).unwrap();
        for (enabled, path) in [
            ("true", "/imported/automatic/java"),
            ("true", "/bin/echo"),
            ("false", "/bin/echo"),
        ] {
            std::fs::write(
                root.join("instance.cfg"),
                format!("name=Test\nAutomaticJava=true\nOverrideJavaLocation={enabled}\nJavaPath={path}\nOverrideMemory=true\n"),
            )
            .unwrap();
            let instance = Instance::load(&root).unwrap();
            let mut baseline = InstanceSettings::load(&root).unwrap();
            let mut form = SettingsForm::resolved(
                &LaunchSettings::for_instance(&instance, &fixture.0),
                &baseline,
            );
            if enabled == "true" {
                assert_ne!(form.values[2], path);
            }
            for memory in ["768", "1024"] {
                form.values[0] = memory.into();
                let updated = form.convert(&baseline).unwrap();
                save_checked(&root, &baseline, &updated).unwrap();
                baseline = InstanceSettings::load(&root).unwrap();
                assert_eq!(baseline.java_path.as_deref(), Some(path));
                let cfg = prism_core::ini::IniFile::load(&root.join("instance.cfg")).unwrap();
                assert_eq!(cfg.get("AutomaticJava"), Some("true"));
                assert!(!LaunchSettings::for_instance(&instance, &fixture.0).java_path_explicit);
            }

            form.overrides[1] = true;
            form.values[2] = "/bin/echo".into();
            let updated = form.convert(&baseline).unwrap();
            save_checked(&root, &baseline, &updated).unwrap();
            let cfg = prism_core::ini::IniFile::load(&root.join("instance.cfg")).unwrap();
            assert_eq!(cfg.get("AutomaticJava"), Some("false"));
            let settings = LaunchSettings::for_instance(&instance, &fixture.0);
            assert_eq!(settings.java_path, std::path::Path::new("/bin/echo"));
            assert!(settings.java_path_explicit);

            let baseline = InstanceSettings::load(&root).unwrap();
            form.overrides[1] = false;
            let updated = form.convert(&baseline).unwrap();
            save_checked(&root, &baseline, &updated).unwrap();
            assert!(!LaunchSettings::for_instance(&instance, &fixture.0).java_path_explicit);
        }
    }

    #[test]
    fn reads_inherited_values_and_legacy_override_state() {
        let fixture = Fixture::new();
        let mut global = prism_core::settings::Settings::default();
        global.set("MinMemAlloc", "1024");
        global.set("JavaPath", "/bin/sh");
        global.set("JvmArgs", "-Dglobal=yes");
        global.set("Env", r#"{"GLOBAL":"value"}"#);
        global.save(&fixture.0.global_config()).unwrap();
        let inherited = SettingsForm::inherited(&fixture.0);
        assert_eq!(inherited.values[0], "1024");
        assert_eq!(inherited.values[2], "/bin/sh");
        assert_eq!(inherited.values[3], "-Dglobal=yes");
        assert_eq!(inherited.env, [("GLOBAL".into(), "value".into())]);
        let root = fixture.0.instances_dir().join("test");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("instance.cfg"), "MinMemAlloc=2048\nname=Test").unwrap();
        let instance = Instance::load(&root).unwrap();
        let raw = InstanceSettings::load(&root).unwrap();
        let effective = LaunchSettings::for_instance(&instance, &fixture.0);
        let legacy = SettingsForm::resolved(&effective, &raw);
        assert!(legacy.overrides[0]);
        assert!(!legacy.overrides[1]);
        assert_eq!(legacy.values[0], "2048");
    }
}

const LABELS: [&str; 9] = [
    "Minimum memory (MB)",
    "Maximum memory (MB)",
    "Java executable",
    "JVM arguments",
    "Width (px)",
    "Height (px)",
    "Pre-launch",
    "Wrapper",
    "Post-exit",
];

#[derive(Clone)]
struct SettingsForm {
    overrides: [bool; 6],
    loaded_java_path: String,
    values: Vec<String>,
    env: Vec<(String, String)>,
}

impl SettingsForm {
    fn local(raw: &InstanceSettings, inherited: &Self) -> Self {
        let mut local = inherited.clone();
        let values = [
            raw.min_memory_mb.map(|n| n.to_string()),
            raw.max_memory_mb.map(|n| n.to_string()),
            raw.legacy_java_path
                .clone()
                .or_else(|| raw.java_path.clone()),
            raw.jvm_args.clone(),
            raw.window_width.map(|n| n.to_string()),
            raw.window_height.map(|n| n.to_string()),
            raw.pre_launch_command.clone(),
            raw.wrapper_command.clone(),
            raw.post_exit_command.clone(),
        ];
        for (index, value) in values.into_iter().enumerate() {
            if let Some(value) = value {
                local.values[index] = value;
            }
        }
        if let Some(env) = &raw.env {
            local.env = env.iter().map(|(key, value)| (key.clone(), value.clone())).collect();
        }
        local
    }

    fn toggle(&mut self, group: usize, enabled: bool, inherited: &Self, draft: &mut Self) {
        if self.overrides[group] {
            for &index in GROUP_FIELDS[group] {
                draft.values[index] = self.values[index].clone();
            }
            if group == 5 {
                draft.env = self.env.clone();
            }
        }
        let source = if enabled { &*draft } else { inherited };
        for &index in GROUP_FIELDS[group] {
            self.values[index] = source.values[index].clone();
        }
        if group == 5 {
            self.env = source.env.clone();
        }
        self.overrides[group] = enabled;
    }

    fn resolved(settings: &LaunchSettings, raw: &InstanceSettings) -> Self {
        Self {
            loaded_java_path: settings.java_path.to_string_lossy().into_owned(),
            overrides: [
                raw.override_memory
                    .unwrap_or(raw.min_memory_mb.is_some() || raw.max_memory_mb.is_some()),
                raw.override_java_location
                    .unwrap_or(raw.java_path.is_some() || raw.legacy_java_path.is_some()),
                raw.override_java_args.unwrap_or(raw.jvm_args.is_some()),
                raw.override_window
                    .unwrap_or(raw.window_width.is_some() || raw.window_height.is_some()),
                raw.override_commands.unwrap_or(
                    raw.pre_launch_command.is_some()
                        || raw.wrapper_command.is_some()
                        || raw.post_exit_command.is_some(),
                ),
                raw.override_env.unwrap_or(raw.env.is_some()),
            ],
            values: vec![
                settings.min_memory_mb.to_string(),
                settings.max_memory_mb.to_string(),
                settings.java_path.to_string_lossy().into_owned(),
                settings.raw_jvm_args.clone(),
                settings.window_width.to_string(),
                settings.window_height.to_string(),
                settings.pre_launch_command.clone(),
                settings.wrapper_command.clone(),
                settings.post_exit_command.clone(),
            ],
            env: settings.env.clone(),
        }
    }

    fn inherited(paths: &Paths) -> Self {
        let global = prism_core::settings::Settings::load(&paths.global_config());
        let java = global
            .get("JavaPath")
            .filter(|v| !v.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| {
                prism_core::java::find_java()
                    .unwrap_or_else(|| "java".into())
                    .to_string_lossy()
                    .into_owned()
            });
        Self {
            overrides: [false; 6],
            loaded_java_path: java.clone(),
            values: vec![
                global.get_int("MinMemAlloc", 512).max(128).to_string(),
                global.get_int("MaxMemAlloc", 4096).max(256).to_string(),
                java,
                global.get_or("JvmArgs", ""),
                global
                    .get_int("MinecraftWinWidth", 854)
                    .max(320)
                    .to_string(),
                global
                    .get_int("MinecraftWinHeight", 480)
                    .max(240)
                    .to_string(),
                global.get_or("PreLaunchCommand", ""),
                global.get_or("WrapperCommand", ""),
                global.get_or("PostExitCommand", ""),
            ],
            env: global
                .get("Env")
                .and_then(|v| serde_json::from_str::<BTreeMap<String, String>>(v).ok())
                .unwrap_or_default()
                .into_iter()
                .collect(),
        }
    }

    fn convert(&self, baseline: &InstanceSettings) -> Result<InstanceSettings, String> {
        let positive = |index: usize| -> Result<u32, String> {
            self.values[index]
                .trim()
                .parse::<u32>()
                .ok()
                .filter(|n| *n > 0)
                .ok_or_else(|| format!("{} must be a positive whole number.", LABELS[index]))
        };
        let mut raw = baseline.clone();
        raw.override_memory = Some(self.overrides[0]);
        raw.override_java_args = Some(self.overrides[2]);
        raw.override_window = Some(self.overrides[3]);
        raw.override_commands = Some(self.overrides[4]);
        raw.override_env = Some(self.overrides[5]);
        if self.overrides[0] {
            let (min, max) = (positive(0)?, positive(1)?);
            if min > max {
                return Err("Minimum memory must not exceed maximum memory.".into());
            }
            raw.min_memory_mb = Some(min);
            raw.max_memory_mb = Some(max);
        }
        let java_was_enabled = baseline.override_java_location.unwrap_or(
            baseline.java_path.is_some() || baseline.legacy_java_path.is_some(),
        );
        if self.overrides[1] != java_was_enabled {
            raw.override_java_location = Some(self.overrides[1]);
        }
        if self.overrides[1]
            && (!java_was_enabled || self.values[2].trim() != self.loaded_java_path.trim())
        {
            raw.override_java_location = Some(true);
            raw.java_path = Some(self.values[2].trim().into());
            raw.legacy_java_path = None;
            if !self.values[2].trim().is_empty() {
                raw.automatic_java = Some(false);
            }
        }
        if self.overrides[2] {
            prism_core::launch::split_args(&self.values[3])
                .map_err(|error| format!("Invalid JVM arguments: {error}"))?;
            raw.jvm_args = Some(self.values[3].trim().into());
        }
        if self.overrides[3] {
            raw.window_width = Some(positive(4)?);
            raw.window_height = Some(positive(5)?);
        }
        if self.overrides[4] {
            prism_core::launch::split_args(&self.values[7])
                .map_err(|error| format!("Invalid wrapper command: {error}"))?;
            raw.pre_launch_command = Some(self.values[6].clone());
            raw.wrapper_command = Some(self.values[7].clone());
            raw.post_exit_command = Some(self.values[8].clone());
        }
        if self.overrides[5] {
            let mut env = BTreeMap::new();
            for (key, value) in &self.env {
                let key = key.trim();
                let mut chars = key.chars();
                if !chars
                    .next()
                    .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                    || !chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
                {
                    return Err("Environment names must start with a letter or underscore and contain only letters, digits or underscores.".into());
                }
                if value.contains('\0') {
                    return Err("Environment values cannot contain NUL characters.".into());
                }
                if env.insert(key.to_owned(), value.clone()).is_some() {
                    return Err(format!("Duplicate environment name: {key}"));
                }
            }
            raw.env = Some(env);
        }
        Ok(raw)
    }
}

fn save_checked(
    root: &Path,
    baseline: &InstanceSettings,
    updated: &InstanceSettings,
) -> Result<(), String> {
    if !root.join("instance.cfg").is_file() || Instance::load(root).is_none() {
        return Err("This instance was deleted. Settings were not saved.".into());
    }
    let current =
        InstanceSettings::load(root).map_err(|e| format!("Could not reload settings: {e}"))?;
    if &current != baseline {
        return Err("Settings changed in another window. Revert to reload before saving.".into());
    }
    updated.save(root).map_err(|e| format!("Save failed: {e}"))
}

pub(crate) struct SettingsEditor {
    root: std::path::PathBuf,
    paths: Paths,
    baseline: InstanceSettings,
    loaded_java_path: String,
    overrides: [bool; 6],
    draft: SettingsForm,
    inputs: Vec<Entity<InputState>>,
    env: Vec<(Entity<InputState>, Entity<InputState>)>,
    status: String,
    success: bool,
}

impl SettingsEditor {
    pub(crate) fn load(
        instance: &Instance,
        paths: &Paths,
        window: &mut Window,
        cx: &mut Context<InstanceWindow>,
    ) -> Result<Self, String> {
        let baseline =
            InstanceSettings::load(&instance.root).map_err(|e| format!("Load failed: {e}"))?;
        let form =
            SettingsForm::resolved(&LaunchSettings::for_instance(instance, paths), &baseline);
        let draft = SettingsForm::local(&baseline, &SettingsForm::inherited(paths));
        let inputs = form
            .values
            .into_iter()
            .enumerate()
            .map(|(i, value)| Self::input(value, LABELS[i], window, cx))
            .collect();
        let env = form
            .env
            .into_iter()
            .map(|(key, value)| Self::env_row(key, value, window, cx))
            .collect();
        Ok(Self {
            root: instance.root.clone(),
            paths: paths.clone(),
            baseline,
            loaded_java_path: form.loaded_java_path,
            overrides: form.overrides,
            draft,
            inputs,
            env,
            status: String::new(),
            success: false,
        })
    }

    fn input(
        value: String,
        placeholder: &str,
        window: &mut Window,
        cx: &mut Context<InstanceWindow>,
    ) -> Entity<InputState> {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder.to_owned()));
        input.update(cx, |input, cx| input.set_value(value, window, cx));
        cx.subscribe(&input, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                if let Some(editor) = &mut this.settings_editor {
                    editor.status.clear();
                }
                cx.notify();
            }
        })
        .detach();
        input
    }

    fn env_row(
        key: String,
        value: String,
        window: &mut Window,
        cx: &mut Context<InstanceWindow>,
    ) -> (Entity<InputState>, Entity<InputState>) {
        (
            Self::input(key, "VARIABLE_NAME", window, cx),
            Self::input(value, "Value", window, cx),
        )
    }

    fn toggle(
        &mut self,
        group: usize,
        enabled: bool,
        window: &mut Window,
        cx: &mut Context<InstanceWindow>,
    ) {
        let inherited = SettingsForm::inherited(&self.paths);
        let mut form = self.form(cx);
        form.toggle(group, enabled, &inherited, &mut self.draft);
        for &i in GROUP_FIELDS[group] {
            self.inputs[i].update(cx, |input, cx| {
                input.set_value(form.values[i].clone(), window, cx)
            });
        }
        if group == 5 {
            self.env = form
                .env
                .into_iter()
                .map(|(key, value)| Self::env_row(key, value, window, cx))
                .collect();
        }
        self.overrides[group] = enabled;
        self.status.clear();
        cx.notify();
    }

    fn form(&self, cx: &Context<InstanceWindow>) -> SettingsForm {
        SettingsForm {
            overrides: self.overrides,
            loaded_java_path: self.loaded_java_path.clone(),
            values: self
                .inputs
                .iter()
                .map(|input| input.read(cx).value().to_string())
                .collect(),
            env: self
                .env
                .iter()
                .map(|(key, value)| {
                    (
                        key.read(cx).value().to_string(),
                        value.read(cx).value().to_string(),
                    )
                })
                .collect(),
        }
    }

    fn save(&mut self, cx: &mut Context<InstanceWindow>) {
        let form = self.form(cx);
        match form.convert(&self.baseline).and_then(|updated| {
            save_checked(&self.root, &self.baseline, &updated)?;
            self.baseline = updated;
            self.loaded_java_path = form.values[2].trim().to_owned();
            Ok(())
        }) {
            Ok(()) => {
                self.status = "Settings saved.".into();
                self.success = true;
            }
            Err(error) => {
                self.status = error;
                self.success = false;
            }
        }
        cx.notify();
    }

    pub(crate) fn render(&self, cx: &mut Context<InstanceWindow>) -> AnyElement {
        let palette = crate::theme::Palette::get(cx);
        let mut list = div().flex().flex_col().gap_3().p_4();
        for (group, title) in [
            "Memory",
            "Java location",
            "Java arguments",
            "Game window",
            "Custom commands",
            "Environment variables",
        ]
        .into_iter()
        .enumerate()
        {
            let enabled = self.overrides[group];
            let mut section = div()
                .flex()
                .flex_col()
                .gap_2()
                .p_3()
                .bg(palette.panel)
                .rounded_md()
                .border_1()
                .border_color(palette.hover)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(div().font_weight(FontWeight::BOLD).child(title))
                        .child(
                            div()
                                .relative()
                                .children((!enabled).then(|| {
                                    div()
                                        .absolute()
                                        .left(px(-1.))
                                        .top(px(1.))
                                        .size(px(18.))
                                        .rounded(px(4.))
                                        .border_1()
                                        .border_color(palette.muted_text)
                                }))
                                .child(
                                    Checkbox::new(SharedString::from(format!("override-{group}")))
                                        .label("Override global settings")
                                        .checked(enabled)
                                        .on_change(cx.listener(move |this, checked: &bool, window, cx| {
                                            if let Some(editor) = &mut this.settings_editor {
                                                editor.toggle(group, *checked, window, cx);
                                            }
                                        })),
                                ),
                        ),
                );
            for &i in GROUP_FIELDS[group] {
                section = section.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(
                            div()
                                .w(px(200.))
                                .flex_shrink_0()
                                .text_color(palette.secondary_text)
                                .child(LABELS[i]),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .bg(palette.window)
                                .rounded_md()
                                .child(Input::new(&self.inputs[i]).disabled(!enabled)),
                        ),
                );
            }
            if group == 4 {
                section = section.child(div().text_size(px(12.)).text_color(palette.muted_text)
                    .child("Use $INST_NAME, $INST_ID, $INST_DIR, $INST_MC_DIR, $INST_JAVA and $INST_JAVA_ARGS in shell commands."));
            }
            if group == 5 {
                section = section.child(
                    div()
                        .flex()
                        .gap_2()
                        .text_size(px(12.))
                        .text_color(palette.muted_text)
                        .child(div().w(px(240.)).child("Variable"))
                        .child(div().flex_1().child("Value")),
                );
                for (index, (key, value)) in self.env.iter().enumerate() {
                    section = section.child(
                        div()
                            .flex()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .w(px(240.))
                                    .bg(palette.window)
                                    .rounded_md()
                                    .child(Input::new(key).disabled(!enabled)),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .bg(palette.window)
                                    .rounded_md()
                                    .child(Input::new(value).disabled(!enabled)),
                            )
                            .child(
                                Button::new(SharedString::from(format!("remove-env-{index}")))
                                    .label("Remove")
                                    .disabled(!enabled)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if let Some(editor) = &mut this.settings_editor {
                                            if index < editor.env.len() {
                                                editor.env.remove(index);
                                            }
                                            editor.status.clear();
                                        }
                                        cx.notify();
                                    })),
                            ),
                    );
                }
                if self.env.is_empty() {
                    section = section.child(
                        div()
                            .text_color(palette.muted_text)
                            .text_size(px(12.))
                            .child("No environment variables."),
                    );
                }
                section = section.child(
                    div().child(
                        Button::new("add-env")
                            .label("+ Add variable")
                            .disabled(!enabled)
                            .on_click(cx.listener(|this, _, window, cx| {
                                let row = Self::env_row(String::new(), String::new(), window, cx);
                                if let Some(editor) = &mut this.settings_editor {
                                    editor.env.push(row);
                                    editor.status.clear();
                                }
                                cx.notify();
                            })),
                    ),
                );
            }
            list = list.child(section);
        }
        div().flex_1().min_h_0().min_w_0().flex().flex_col().text_size(px(13.))
            .child(div().px_4().py_3().border_b_1().border_color(palette.hover)
                .child(div().font_weight(FontWeight::BOLD).text_size(px(16.)).child("Instance settings"))
                .child(div().text_size(px(12.)).text_color(palette.muted_text).child("Unchecked sections inherit launcher defaults. Changes apply to the next launch.")))
            .child(div().id("instance-settings-scroll").flex_1().min_h_0().overflow_y_scroll().child(list))
            .child(div().px_4().py_3().border_t_1().border_color(palette.hover).flex().items_center().gap_3()
                .child(Button::new("save-instance-settings").label("Save settings")
                    .on_click(cx.listener(|this, _, _, cx| {
                        if this.app.read(cx).instances.get(&this.instance_id).is_none() {
                            if let Some(editor) = &mut this.settings_editor { editor.status = "This instance was deleted. Settings were not saved.".into(); editor.success = false; }
                        } else if let Some(editor) = &mut this.settings_editor { editor.save(cx); }
                        cx.notify();
                    })))
                .child(Button::new("revert-instance-settings").label("Revert")
                    .on_click(cx.listener(|this, _, window, cx| {
                        let instance = this.app.read(cx).instances.get(&this.instance_id).cloned();
                        if let Some(instance) = instance.filter(|inst| inst.root.join("instance.cfg").is_file()) {
                            let paths = this.app.read(cx).paths.clone();
                            match Self::load(&instance, &paths, window, cx) {
                                Ok(editor) => this.settings_editor = Some(editor),
                                Err(error) => if let Some(editor) = &mut this.settings_editor { editor.status = error; editor.success = false; },
                            }
                        } else if let Some(editor) = &mut this.settings_editor { editor.status = "This instance was deleted.".into(); editor.success = false; }
                        cx.notify();
                    })))
                .child(div().flex_1().text_size(px(12.)).text_color(if self.success { palette.success } else { palette.error }).child(self.status.clone())))
            .into_any_element()
    }
}
