//! Raw per-instance launch preferences, preserving absent override flags.
use crate::ini::IniFile;
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstanceSettings {
    pub override_memory: Option<bool>,
    pub override_java_location: Option<bool>,
    pub override_java_args: Option<bool>,
    pub override_window: Option<bool>,
    pub override_commands: Option<bool>,
    pub override_env: Option<bool>,
    pub min_memory_mb: Option<u32>,
    pub max_memory_mb: Option<u32>,
    pub java_path: Option<String>,
    pub legacy_java_path: Option<String>,
    pub jvm_args: Option<String>,
    pub window_width: Option<u32>,
    pub window_height: Option<u32>,
    pub pre_launch_command: Option<String>,
    pub wrapper_command: Option<String>,
    pub post_exit_command: Option<String>,
    pub env: Option<BTreeMap<String, String>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launch::{LaunchSettings, Launcher};
    use crate::net::Downloader;
    use crate::profile::LaunchProfile;
    use crate::{Instance, Paths};
    use std::sync::Arc;

    struct Fixture {
        paths: Paths,
        instance: Instance,
    }

    impl Fixture {
        fn new() -> Self {
            let paths = Paths {
                root: std::env::temp_dir().join(format!("prism-settings-{}", uuid::Uuid::new_v4())),
            };
            let root = paths.instances_dir().join("example");
            std::fs::create_dir_all(root.join("minecraft")).unwrap();
            std::fs::write(root.join("instance.cfg"), "name=Example\n").unwrap();
            let instance = Instance::load(&root).unwrap();
            Self { paths, instance }
        }

        fn resolve(&self, local: &IniFile) -> LaunchSettings {
            local
                .save(&self.instance.root.join("instance.cfg"))
                .unwrap();
            LaunchSettings::for_instance(&self.instance, &self.paths)
        }

        fn command(&self, settings: &LaunchSettings) -> crate::launch::PreparedLaunch {
            let launcher = Launcher::new(self.paths.clone(), Arc::new(Downloader::new()));
            let profile = LaunchProfile {
                main_class: "example.Main".into(),
                minecraft_version: "1.20.1".into(),
                ..Default::default()
            };
            launcher.build_command(settings, None, &profile)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.paths.root);
        }
    }

    #[test]
    fn override_groups_and_global_inheritance() {
        let fixture = Fixture::new();
        let mut global = IniFile::default();
        let mut local = IniFile::default();
        let values = [
            ("MinMemAlloc", "768", "1024"),
            ("MaxMemAlloc", "3072", "8192"),
            ("MinecraftWinWidth", "900", "1200"),
            ("MinecraftWinHeight", "600", "800"),
            ("JavaPath", "/bin/sh", "/bin/echo"),
            ("JvmArgs", "-Dsource=global", "-Dsource=instance"),
            ("PreLaunchCommand", "global-pre", "instance-pre"),
            ("WrapperCommand", "global-wrapper", "instance-wrapper"),
            ("PostExitCommand", "global-post", "instance-post"),
            ("Env", r#"{"SOURCE":"global"}"#, r#"{"SOURCE":"instance"}"#),
        ];
        for (key, g, l) in values {
            global.set(key, g);
            local.set(key, l);
        }
        global.save(&fixture.paths.global_config()).unwrap();
        let flags = [
            "OverrideMemory",
            "OverrideJavaLocation",
            "OverrideJavaArgs",
            "OverrideWindow",
            "OverrideCommands",
            "OverrideEnv",
        ];
        for flag_value in [None, Some("true"), Some("false")] {
            for flag in flags {
                if let Some(value) = flag_value {
                    local.set(flag, value);
                } else {
                    local.remove(flag);
                }
            }
            let settings = fixture.resolve(&local);
            let use_global = flag_value == Some("false");
            assert_eq!(settings.min_memory_mb, if use_global { 768 } else { 1024 });
            assert_eq!(settings.max_memory_mb, if use_global { 3072 } else { 8192 });
            assert_eq!(settings.window_width, if use_global { 900 } else { 1200 });
            assert_eq!(settings.window_height, if use_global { 600 } else { 800 });
            assert_eq!(
                settings.java_path,
                Path::new(if use_global { "/bin/sh" } else { "/bin/echo" })
            );
            assert_eq!(
                settings.extra_jvm_args,
                [if use_global {
                    "-Dsource=global"
                } else {
                    "-Dsource=instance"
                }]
            );
            assert_eq!(
                settings.pre_launch_command,
                if use_global {
                    "global-pre"
                } else {
                    "instance-pre"
                }
            );
            assert_eq!(
                settings.wrapper_command,
                if use_global {
                    "global-wrapper"
                } else {
                    "instance-wrapper"
                }
            );
            assert_eq!(
                settings.post_exit_command,
                if use_global {
                    "global-post"
                } else {
                    "instance-post"
                }
            );
            assert_eq!(
                settings.env,
                vec![(
                    "SOURCE".into(),
                    if use_global {
                        "global".into()
                    } else {
                        "instance".into()
                    }
                )]
            );
        }
        let inherited = fixture.resolve(&IniFile::default());
        assert_eq!(inherited.java_path, Path::new("/bin/sh"));
        assert_eq!(inherited.extra_jvm_args, ["-Dsource=global"]);
        assert_eq!(inherited.max_memory_mb, 3072);
        let invalid_local = IniFile::parse("MinMemAlloc=bad\nMaxMemAlloc=bad\n");
        assert_eq!(fixture.resolve(&invalid_local).max_memory_mb, 3072);
        let numeric_flags =
            InstanceSettings::from_ini(&IniFile::parse("OverrideMemory=1\nOverrideWindow=0\n"));
        assert_eq!(numeric_flags.override_memory, Some(true));
        assert_eq!(numeric_flags.override_window, Some(false));
        for flag in flags {
            local.set(flag, "true");
        }
        for (key, _, _) in values {
            local.remove(key);
        }
        let empty = fixture.resolve(&local);
        assert_eq!(empty.max_memory_mb, 4096);
        assert!(empty.extra_jvm_args.is_empty());
        assert!(empty.wrapper_command.is_empty());
        assert!(empty.env.is_empty());
        local.set("MinMemAlloc", "invalid");
        local.set("Env", "not json");
        let invalid = fixture.resolve(&local);
        assert_eq!(invalid.min_memory_mb, 512);
        assert!(invalid.env.is_empty());
    }

    #[test]
    fn legacy_keys_and_explicit_flags() {
        let fixture = Fixture::new();
        let mut local = IniFile::default();
        local.set("OverrideJavaPath", "/bin/sh");
        local.set("JavaPath", "/bin/echo");
        local.set("jvmArgs", "-Dlegacy=yes");
        let settings = fixture.resolve(&local);
        assert_eq!(settings.java_path, Path::new("/bin/sh"));
        assert_eq!(settings.extra_jvm_args, ["-Dlegacy=yes"]);
        local.set("OverrideJavaLocation", "true");
        assert_eq!(fixture.resolve(&local).java_path, Path::new("/bin/echo"));
    }

    #[test]
    fn raw_save_preserves_unrelated_keys_and_env_json() {
        let fixture = Fixture::new();
        let path = fixture.instance.root.join("instance.cfg");
        std::fs::write(
            &path,
            "[General]\nname=Example\nUnrelated=keep\n[UI]\nTab=3\n",
        )
        .unwrap();
        let mut raw = InstanceSettings::load(&fixture.instance.root).unwrap();
        assert_eq!(raw.override_memory, None);
        raw.override_memory = Some(false);
        raw.max_memory_mb = Some(6144);
        raw.env = Some(
            [(
                "QUOTED".into(),
                "a \"quote\"; C:\\folder\\file\nnext".into(),
            )]
            .into(),
        );
        raw.save(&fixture.instance.root).unwrap();
        assert_eq!(InstanceSettings::load(&fixture.instance.root).unwrap(), raw);
        let cfg = IniFile::load(&path).unwrap();
        assert_eq!(cfg.get("Unrelated"), Some("keep"));
        assert_eq!(cfg.get("Tab"), Some("3"));
        assert!(std::fs::read_to_string(&path).unwrap().contains("[UI]"));
        raw.override_memory = None;
        raw.max_memory_mb = None;
        raw.env = Some(BTreeMap::new());
        raw.save(&fixture.instance.root).unwrap();
        assert_eq!(InstanceSettings::load(&fixture.instance.root).unwrap(), raw);
        assert!(IniFile::load(&path)
            .unwrap()
            .get("OverrideMemory")
            .is_none());
        // A QSettings-escaped JSON object, as written by upstream.
        let cfg = IniFile::parse(r#"Env="{\"HELLO\":\"world\",\"WITH_SPACE\":\"a b\"}""#);
        assert_eq!(
            InstanceSettings::from_ini(&cfg).env.unwrap()["WITH_SPACE"],
            "a b"
        );
        assert!(InstanceSettings::from_ini(&IniFile::parse("Env=invalid"))
            .env
            .is_none());
    }

    #[test]
    #[cfg(unix)]
    fn saving_manual_java_override_replaces_imported_automatic_pick() {
        let fixture = Fixture::new();
        for original_path in ["/bin/sh", "/bin/echo"] {
            let mut imported = IniFile::default();
            imported.set("AutomaticJava", "true");
            imported.set("OverrideJavaLocation", "true");
            imported.set("JavaPath", original_path);
            let automatic = fixture.resolve(&imported);
            assert!(!automatic.java_path_explicit);

            let mut raw = InstanceSettings::load(&fixture.instance.root).unwrap();
            raw.java_path = Some("/bin/echo".into());
            raw.override_java_location = Some(true);
            raw.save(&fixture.instance.root).unwrap();
            let settings = LaunchSettings::for_instance(&fixture.instance, &fixture.paths);
            assert!(
                settings.java_path_explicit,
                "saved manual Java must be explicit"
            );
            assert_eq!(settings.java_path, Path::new("/bin/echo"));
            assert_eq!(fixture.command(&settings).command[0], "/bin/echo");
            let saved = IniFile::load(&fixture.instance.root.join("instance.cfg")).unwrap();
            assert_eq!(saved.get("AutomaticJava"), Some("false"));

            raw.override_java_location = Some(false);
            raw.save(&fixture.instance.root).unwrap();
            let settings = LaunchSettings::for_instance(&fixture.instance, &fixture.paths);
            assert!(!settings.java_path_explicit);
            assert_eq!(
                InstanceSettings::load(&fixture.instance.root).unwrap().java_path,
                raw.java_path
            );
        }
    }

    #[test]
    fn saving_without_manual_java_preserves_automatic_marker() {
        let fixture = Fixture::new();
        for marker in [None, Some("true")] {
            for (flag, path) in [
                (None, Some("imported-java")),
                (Some(false), Some("retained-java")),
                (Some(true), None),
                (Some(true), Some("")),
            ] {
                let mut imported = IniFile::default();
                if let Some(marker) = marker {
                    imported.set("AutomaticJava", marker);
                }
                fixture.resolve(&imported);
                let mut raw = InstanceSettings::load(&fixture.instance.root).unwrap();
                raw.override_java_location = flag;
                raw.java_path = path.map(str::to_owned);
                raw.save(&fixture.instance.root).unwrap();
                let saved = IniFile::load(&fixture.instance.root.join("instance.cfg")).unwrap();
                assert_eq!(saved.get("AutomaticJava"), marker);
            }
        }
    }

    #[test]
    #[cfg(unix)]
    fn grouped_java_overrides_preserve_automatic_selection_and_compatibility() {
        let fixture = Fixture::new();
        let mut global = IniFile::default();
        global.set("JavaPath", "/bin/sh");
        global.set("AutomaticJavaDownload", "false");
        global.set("IgnoreJavaCompatibility", "true");
        global.save(&fixture.paths.global_config()).unwrap();
        let mut local = IniFile::default();
        local.set("JavaPath", "/bin/echo");
        local.set("IgnoreJavaCompatibility", "false");
        for flag in [None, Some("true"), Some("false")] {
            if let Some(flag) = flag {
                local.set("OverrideJavaLocation", flag);
            } else {
                local.remove("OverrideJavaLocation");
            }
            local.remove("AutomaticJava");
            let settings = fixture.resolve(&local);
            assert_eq!(settings.java_path, Path::new(if flag == Some("false") {
                "/bin/sh"
            } else {
                "/bin/echo"
            }));
            assert!(settings.java_path_explicit);
            assert!(!settings.auto_java_download);
            assert!(!settings.ignore_java_compatibility);

            local.set("AutomaticJava", "true");
            let settings = fixture.resolve(&local);
            if flag == Some("true") {
                assert!(!settings.java_path_explicit);
            } else {
                assert_eq!(settings.java_path, Path::new("/bin/sh"));
                assert!(settings.java_path_explicit);
            }
        }
        local.remove("OverrideJavaLocation");
        local.remove("IgnoreJavaCompatibility");
        global.remove("JavaPath");
        global.save(&fixture.paths.global_config()).unwrap();
        let settings = fixture.resolve(&local);
        assert!(!settings.java_path_explicit);
        assert!(settings.ignore_java_compatibility);
    }

    #[test]
    fn argument_splitting_preserves_quotes_and_windows_paths() {
        use crate::launch::split_args;
        let cases = [
            (r#"-Dmessage="hello world""#, "-Dmessage=hello world"),
            (r"C:\tools\wrapper.exe", r"C:\tools\wrapper.exe"),
            (
                r#""C:\Program Files\wrapper.exe""#,
                r"C:\Program Files\wrapper.exe",
            ),
            (r"C:\tools\", r"C:\tools\"),
            (r#"'a\\b\'c'"#, r"a\b'c"),
        ];
        let fixture = Fixture::new();
        for (input, expected) in cases {
            assert_eq!(split_args(input).unwrap(), [expected], "{input}");
            let mut cfg = IniFile::default();
            cfg.set("JvmArgs", input);
            let mut settings = fixture.resolve(&cfg);
            assert_eq!(settings.extra_jvm_args, [expected]);
            assert!(fixture.command(&settings).command.iter().any(|arg| arg == expected));
            settings.wrapper_command = input.into();
            let prepared = fixture.command(&settings);
            assert_eq!(prepared.command[0], expected);
            assert!(prepared.command_error.is_none());
        }
        assert!(split_args("  \t '' \"\" ").unwrap().is_empty());
        assert_eq!(
            split_args("one\ttwo 'three four'").unwrap(),
            ["one", "two", "three four"]
        );
        for input in ["'unterminated", "\"unterminated", r#""C:\tools\""#] {
            assert!(split_args(input).unwrap_err().contains("Unterminated"));
            let mut cfg = IniFile::default();
            cfg.set("JvmArgs", input);
            let settings = fixture.resolve(&cfg);
            let prepared = fixture.command(&settings);
            assert!(prepared
                .spawn()
                .unwrap_err()
                .to_string()
                .contains("Invalid JVM arguments"));
            assert!(prepared.shell_script().is_err());
        }
    }

    #[test]
    fn quoted_global_jvm_arguments_and_invalid_inheritance() {
        let fixture = Fixture::new();
        let mut global = IniFile::default();
        let mut local = IniFile::default();
        for flag in [None, Some("false")] {
            if let Some(flag) = flag {
                local.set("OverrideJavaArgs", flag);
                local.set("JvmArgs", "-Dignored=yes");
            } else {
                local.remove("OverrideJavaArgs");
                local.remove("JvmArgs");
            }
            global.set("JvmArgs", r#"-Dmessage="hello world""#);
            global.save(&fixture.paths.global_config()).unwrap();
            let settings = fixture.resolve(&local);
            assert_eq!(settings.extra_jvm_args, ["-Dmessage=hello world"]);
            assert!(fixture.command(&settings).command.iter()
                .any(|arg| arg == "-Dmessage=hello world"));
            global.set("JvmArgs", "-Dmessage=\"unfinished");
            global.save(&fixture.paths.global_config()).unwrap();
            assert!(fixture.command(&fixture.resolve(&local))
                .spawn().unwrap_err().to_string().contains("Invalid JVM arguments"));
        }
    }

    #[test]
    fn command_settings_and_invalid_wrapper() {
        let fixture = Fixture::new();
        let mut settings = fixture.resolve(&IniFile::default());
        settings.java_path = "/bin/echo".into();
        settings.min_memory_mb = 2048;
        settings.max_memory_mb = 1024;
        settings.extra_jvm_args = vec!["-Dexample=yes".into()];
        settings.window_width = 1234;
        settings.window_height = 567;
        settings.wrapper_command = "env 'WRAPPED=hello world'".into();
        settings.env = vec![("CUSTOM".into(), "a b".into())];
        let prepared = fixture.command(&settings);
        assert_eq!(
            &prepared.command[..3],
            ["env", "WRAPPED=hello world", "/bin/echo"]
        );
        for arg in ["-Xms1024m", "-Xmx2048m", "-Dexample=yes"] {
            assert!(prepared.command.iter().any(|v| v == arg));
        }
        assert!(prepared
            .command
            .windows(4)
            .any(|v| v == ["--width", "1234", "--height", "567"]));
        assert!(prepared.env.contains(&("INST_ID".into(), "example".into())));
        assert!(prepared.env.contains(&("CUSTOM".into(), "a b".into())));
        assert!(prepared
            .shell_script()
            .unwrap()
            .contains("'WRAPPED=hello world' '/bin/echo'"));
        settings.wrapper_command = "'unterminated".into();
        let invalid = fixture.command(&settings);
        assert!(invalid.spawn().is_err());
        assert!(invalid.shell_script().is_err());
    }

    #[cfg(unix)]
    #[test]
    fn hooks_environment_working_directory_and_failures() {
        let fixture = Fixture::new();
        let mut settings = fixture.resolve(&IniFile::default());
        settings.env = vec![("CUSTOM".into(), "custom value".into())];
        settings.pre_launch_command = r#"printf '%s\n' "$INST_ID:$CUSTOM:$PWD" > pre.txt"#.into();
        settings.post_exit_command = r#"printf '%s' "$INST_NAME:$INST_MC_DIR" > post.txt"#.into();
        let mut prepared = fixture.command(&settings);
        prepared.command = vec!["/bin/sh".into(), "-c".into(), "exit 7".into()];
        assert_eq!(prepared.spawn().unwrap().wait().unwrap().code(), Some(7));
        prepared.run_post_exit().unwrap();
        assert_eq!(
            std::fs::read_to_string(fixture.instance.root.join("pre.txt")).unwrap(),
            format!("example:custom value:{}\n", fixture.instance.root.display())
        );
        assert!(
            std::fs::read_to_string(fixture.instance.root.join("post.txt"))
                .unwrap()
                .starts_with("Example:")
        );
        let mut output = Vec::new();
        prepared.pre_launch_command = "printf 'hello\\nworld\\n'; printf 'warning\\n' >&2".into();
        prepared
            .spawn_with_output(|line| output.push(line))
            .unwrap()
            .wait()
            .unwrap();
        assert_eq!(output, [
            "Pre-launch (stdout): hello",
            "Pre-launch (stdout): world",
            "Pre-launch (stderr): warning",
        ]);
        output.clear();
        prepared.post_exit_command = "echo done; echo failed >&2; exit 2".into();
        assert!(prepared
            .run_post_exit_with_output(|line| output.push(line))
            .is_err());
        assert_eq!(output, ["Post-exit (stdout): done", "Post-exit (stderr): failed"]);
        output.clear();
        prepared.post_exit_command.clear();
        prepared
            .run_post_exit_with_output(|line| output.push(line))
            .unwrap();
        assert!(output.is_empty());
        prepared.pre_launch_command = "echo aborted >&2; exit 9".into();
        prepared.command = vec!["/not/a/real/executable".into()];
        assert!(prepared
            .spawn_with_output(|line| output.push(line))
            .unwrap_err()
            .to_string()
            .contains("Hook command failed"));
        assert_eq!(output, ["Pre-launch (stderr): aborted"]);
        prepared.pre_launch_command.clear();
        assert!(prepared.spawn().is_err());
        prepared.post_exit_command = "exit 2".into();
        assert!(prepared.run_post_exit().is_err());
        prepared.post_exit_command.clear();
        assert!(prepared.run_post_exit().is_ok());

        prepared.command = vec!["/bin/sh".into(), "-c".into(), "exit 7".into()];
        prepared.pre_launch_command = "true".into();
        prepared.post_exit_command = "true".into();
        let script = fixture.instance.root.join("launch.sh");
        std::fs::write(&script, prepared.shell_script().unwrap()).unwrap();
        assert_eq!(
            std::process::Command::new("/bin/sh")
                .arg(script)
                .status()
                .unwrap()
                .code(),
            Some(7)
        );
    }
}

impl InstanceSettings {
    pub fn load(instance_root: &Path) -> std::io::Result<Self> {
        let cfg = match IniFile::load(&instance_root.join("instance.cfg")) {
            Ok(cfg) => cfg,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => IniFile::default(),
            Err(e) => return Err(e),
        };
        Ok(Self::from_ini(&cfg))
    }

    pub fn from_ini(cfg: &IniFile) -> Self {
        let flag = |key| match cfg.get(key) {
            Some("true" | "1") => Some(true),
            Some("false" | "0") => Some(false),
            _ => None,
        };
        Self {
            override_memory: flag("OverrideMemory"),
            override_java_location: flag("OverrideJavaLocation"),
            override_java_args: flag("OverrideJavaArgs"),
            override_window: flag("OverrideWindow"),
            override_commands: flag("OverrideCommands"),
            override_env: flag("OverrideEnv"),
            min_memory_mb: cfg.get("MinMemAlloc").and_then(|v| v.parse().ok()),
            max_memory_mb: cfg.get("MaxMemAlloc").and_then(|v| v.parse().ok()),
            java_path: cfg.get("JavaPath").map(str::to_owned),
            legacy_java_path: cfg.get("OverrideJavaPath").map(str::to_owned),
            jvm_args: cfg
                .get("JvmArgs")
                .or_else(|| cfg.get("jvmArgs"))
                .map(str::to_owned),
            window_width: cfg.get("MinecraftWinWidth").and_then(|v| v.parse().ok()),
            window_height: cfg.get("MinecraftWinHeight").and_then(|v| v.parse().ok()),
            pre_launch_command: cfg.get("PreLaunchCommand").map(str::to_owned),
            wrapper_command: cfg.get("WrapperCommand").map(str::to_owned),
            post_exit_command: cfg.get("PostExitCommand").map(str::to_owned),
            env: cfg.get("Env").and_then(|v| serde_json::from_str(v).ok()),
        }
    }

    /// Reload the INI before updating only launch keys; None removes the key.
    pub fn save(&self, instance_root: &Path) -> std::io::Result<()> {
        let path = instance_root.join("instance.cfg");
        let mut cfg = match IniFile::load(&path) {
            Ok(cfg) => cfg,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => IniFile::default(),
            Err(e) => return Err(e),
        };
        if let Some(value) = &self.override_memory {
            cfg.set("OverrideMemory", value.to_string());
        } else {
            cfg.remove("OverrideMemory");
        }
        if let Some(value) = &self.override_java_location {
            cfg.set("OverrideJavaLocation", value.to_string());
        } else {
            cfg.remove("OverrideJavaLocation");
        }
        if let Some(value) = &self.override_java_args {
            cfg.set("OverrideJavaArgs", value.to_string());
        } else {
            cfg.remove("OverrideJavaArgs");
        }
        if let Some(value) = &self.override_window {
            cfg.set("OverrideWindow", value.to_string());
        } else {
            cfg.remove("OverrideWindow");
        }
        if let Some(value) = &self.override_commands {
            cfg.set("OverrideCommands", value.to_string());
        } else {
            cfg.remove("OverrideCommands");
        }
        if let Some(value) = &self.override_env {
            cfg.set("OverrideEnv", value.to_string());
        } else {
            cfg.remove("OverrideEnv");
        }
        if let Some(value) = &self.min_memory_mb {
            cfg.set("MinMemAlloc", value.to_string());
        } else {
            cfg.remove("MinMemAlloc");
        }
        if let Some(value) = &self.max_memory_mb {
            cfg.set("MaxMemAlloc", value.to_string());
        } else {
            cfg.remove("MaxMemAlloc");
        }
        if let Some(value) = &self.java_path {
            cfg.set("JavaPath", value.to_string());
        } else {
            cfg.remove("JavaPath");
        }
        if self.override_java_location == Some(true)
            && self.java_path.as_deref().is_some_and(|path| !path.trim().is_empty())
        {
            cfg.set("AutomaticJava", "false");
        }
        if let Some(value) = &self.legacy_java_path {
            cfg.set("OverrideJavaPath", value.to_string());
        } else {
            cfg.remove("OverrideJavaPath");
        }
        if let Some(value) = &self.jvm_args {
            cfg.set("JvmArgs", value.to_string());
        } else {
            cfg.remove("JvmArgs");
        }
        if let Some(value) = &self.window_width {
            cfg.set("MinecraftWinWidth", value.to_string());
        } else {
            cfg.remove("MinecraftWinWidth");
        }
        if let Some(value) = &self.window_height {
            cfg.set("MinecraftWinHeight", value.to_string());
        } else {
            cfg.remove("MinecraftWinHeight");
        }
        if let Some(value) = &self.pre_launch_command {
            cfg.set("PreLaunchCommand", value.to_string());
        } else {
            cfg.remove("PreLaunchCommand");
        }
        if let Some(value) = &self.wrapper_command {
            cfg.set("WrapperCommand", value.to_string());
        } else {
            cfg.remove("WrapperCommand");
        }
        if let Some(value) = &self.post_exit_command {
            cfg.set("PostExitCommand", value.to_string());
        } else {
            cfg.remove("PostExitCommand");
        }
        cfg.remove("jvmArgs");
        if let Some(env) = &self.env {
            cfg.set("Env", serde_json::to_string(env)?);
        } else {
            cfg.remove("Env");
        }
        cfg.save(&path)
    }
}
