#![cfg(target_os = "linux")]

use prism_core::{AccountList, InstanceList, Paths};
use std::{fs, os::unix::fs::PermissionsExt, process::Command};

fn cli(paths: &Paths) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_prism-rs"));
    cmd.env("PRISM_RS_DATA", &paths.root)
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY");
    cmd
}

#[test]
fn prepare_self_contained_minecraft_patch_without_metadata_or_network() {
    let root = tempfile::tempdir().unwrap();
    let paths = Paths { root: root.path().into() };
    let instance = InstanceList::create(&paths, "Offline", "default", "1.20.1").unwrap();
    fs::create_dir_all(instance.root.join("libraries")).unwrap();
    fs::write(instance.root.join("libraries/client-1.jar"), b"local client").unwrap();
    prism_core::version_components::Pack::load(&instance.root).unwrap().customize(
        prism_core::version_components::MINECRAFT,
        &serde_json::json!({"uid":"net.minecraft","formatVersion":1,"version":"1.20.1",
            "mainClass":"offline.Main", "minecraftArguments":"--username ${auth_player_name}",
            "mainJar":{"name":"custom:client:1","MMC-hint":"local"}}),
    ).unwrap();
    let output = cli(&paths).args(["prepare", "--instance", &instance.id])
        .env("HTTP_PROXY", "http://127.0.0.1:1")
        .env("HTTPS_PROXY", "http://127.0.0.1:1")
        .env("ALL_PROXY", "http://127.0.0.1:1")
        .env("NO_PROXY", "").output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}\n{}", String::from_utf8_lossy(&output.stderr));
    assert!(stdout.contains("offline.Main"));
    assert!(!stdout.contains("Fetching version metadata"));
    assert!(!paths.metadata_dir().exists());
    assert!(paths.root.join("launch-headless.sh").exists());
}

#[test]
fn launch_requires_an_installed_instance_id() {
    let root = tempfile::tempdir().unwrap();
    let paths = Paths {
        root: root.path().into(),
    };
    let missing_id = cli(&paths).arg("launch").output().unwrap();
    assert_eq!(missing_id.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&missing_id.stderr)
        .contains("Usage: prism-rs launch <instance-id>"));

    let unknown_id = cli(&paths).args(["launch", "missing"]).output().unwrap();
    assert_eq!(unknown_id.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&unknown_id.stderr).contains("Instance not found"));
}

#[test]
fn shortcut_rejects_a_replacement_instance_with_the_same_id() {
    let root = tempfile::tempdir().unwrap();
    let paths = Paths {
        root: root.path().into(),
    };
    let original = InstanceList::create(&paths, "World", "default", "1.20.1").unwrap();
    let original_token = original.ensure_shortcut_token().unwrap();
    assert_eq!(original.ensure_shortcut_token().unwrap(), original_token);
    fs::remove_dir_all(&original.root).unwrap();
    let replacement = InstanceList::create(&paths, "World", "default", "1.20.1").unwrap();
    assert_eq!(original.id, replacement.id);
    assert_ne!(original_token, replacement.ensure_shortcut_token().unwrap());
    let output = cli(&paths)
        .args(["launch", &original.id, "--shortcut-token", &original_token])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr)
        .contains("no longer points to the original instance"));
}

#[test]
fn launch_saves_a_horizon_folders_verified_new_location() {
    use prism_core::horizon::{HorizonLauncher, LAUNCHER_PATH_SETTING};
    use prism_core::settings::Settings;

    let root = tempfile::tempdir().unwrap();
    let paths = Paths {
        root: root.path().into(),
    };
    let original = root.path().join("original");
    let moved = root.path().join("moved");
    fs::create_dir(&original).unwrap();
    let list = root.path().join("instances.json");
    let write_list = |home: &std::path::Path| {
        fs::write(
            &list,
            format!(
                r#"[{{"id":"world","name":"World","home":"{}","java":null}}]"#,
                home.display()
            ),
        )
        .unwrap();
    };
    write_list(&original);
    let exe = root.path().join("horizon-launcher");
    fs::write(&exe, "#!/bin/sh\ncase \"$1\" in --json) cat \"$(dirname \"$0\")/instances.json\";; play) exit 0;; *) exit 1;; esac\n").unwrap();
    fs::set_permissions(&exe, fs::Permissions::from_mode(0o700)).unwrap();
    let mut settings = Settings::load(&paths.global_config());
    settings.set(LAUNCHER_PATH_SETTING, exe.to_str().unwrap());
    settings.save(&paths.global_config()).unwrap();
    let horizon = HorizonLauncher::from_settings(&settings)
        .list()
        .unwrap()
        .remove(0);
    let instance = InstanceList::link_horizon(&paths, horizon).unwrap();
    fs::rename(original, &moved).unwrap();
    write_list(&moved);

    assert!(cli(&paths)
        .args(["launch", &instance.id])
        .status()
        .unwrap()
        .success());
    let list = InstanceList::load(&paths).unwrap();
    assert_eq!(list.get(&instance.id).unwrap().game_root(), moved);
}

#[test]
fn launch_uses_instance_settings_and_account_and_returns_the_games_exit_code() {
    let root = tempfile::tempdir().unwrap();
    let paths = Paths {
        root: root.path().join("Prism test data"),
    };
    let instance = InstanceList::create(&paths, "Test World", "default", "1.20.1").unwrap();
    let meta_dir = paths.metadata_dir().join("net.minecraft");
    let jar_dir = paths.cache_dir().join("versions/1.20.1");
    fs::create_dir_all(&meta_dir).unwrap();
    fs::create_dir_all(&jar_dir).unwrap();
    fs::write(
        meta_dir.join("1.20.1.json"),
        r#"{"id":"1.20.1","mainClass":"test.Game","javaVersion":{"majorVersion":17,"component":"java-runtime-gamma"}}"#,
    ).unwrap();
    fs::write(jar_dir.join("1.20.1.jar"), []).unwrap();
    let java = root.path().join("java stub");
    fs::write(
        &java,
        r#"#!/bin/sh
if [ "$1" = "-version" ]; then
    if [ -f "$PRISM_RS_DATA/accounts-switched.json" ]; then
        mv "$PRISM_RS_DATA/accounts-switched.json" "$PRISM_RS_DATA/accounts.json"
    fi
    echo 'openjdk version "17.0.10"' >&2
    exit
fi
printf '%s\n' "$PWD" "$@"
echo 'game stderr' >&2
exit "${PRISM_TEST_GAME_STATUS:-42}"
"#,
    ).unwrap();
    fs::set_permissions(&java, fs::Permissions::from_mode(0o700)).unwrap();
    let config = instance.root.join("instance.cfg");
    let mut cfg = prism_core::ini::IniFile::load(&config).unwrap();
    cfg.set("JavaPath", java.to_str().unwrap());
    cfg.set("MaxMemAlloc", "2048");
    cfg.set("OverrideCommands", "true");
    cfg.set("PreLaunchCommand", "echo pre-launch-ran");
    cfg.set("PostExitCommand", "echo post-exit-ran");
    cfg.save(&config).unwrap();
    let mut accounts = AccountList::default();
    accounts.add_offline("DemoPlayer");
    accounts.save(&paths.accounts_file()).unwrap();

    let lock_dir = paths.logs_dir().join("launch-locks");
    fs::create_dir_all(&lock_dir).unwrap();
    let lock = fs::File::create(lock_dir.join(&instance.id)).unwrap();
    fs2::FileExt::try_lock_exclusive(&lock).unwrap();
    let blocked = cli(&paths).args(["launch", &instance.id]).output().unwrap();
    assert_eq!(blocked.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&blocked.stderr).contains("already launching or running"));
    drop(lock);

    accounts.active = Some(accounts.add_offline("OtherPlayer").id.clone());
    accounts.save(&paths.root.join("accounts-switched.json")).unwrap();

    let output = cli(&paths).args(["launch", &instance.id]).output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(42), "{stdout}\n{stderr}");
    assert!(stdout.lines().any(|line| line == "-Xmx2048m"));
    assert!(stdout.lines().any(|line| line == "test.Game"));
    assert!(stdout.lines().any(|line| line == "DemoPlayer"));
    assert_eq!(
        AccountList::load(&paths.accounts_file()).active_account().unwrap().player_name,
        "OtherPlayer"
    );
    assert!(stdout.lines().any(|line| line == "pre-launch-ran"));
    assert!(stdout.lines().any(|line| line == "post-exit-ran"));
    assert!(stdout
        .lines()
        .any(|line| line == instance.game_root().to_str().unwrap()));
    assert!(stderr.contains("game stderr"));

    cfg.set("PostExitCommand", "exit 7");
    cfg.save(&config).unwrap();
    let output = cli(&paths).args(["launch", &instance.id]).output().unwrap();
    assert_eq!(output.status.code(), Some(42));
    assert!(String::from_utf8_lossy(&output.stderr).contains("Post-exit command failed"));

    let output = cli(&paths)
        .env("PRISM_TEST_GAME_STATUS", "0")
        .args(["launch", &instance.id])
        .output()
        .unwrap();
    assert!(output.status.success());
    let token = instance.ensure_shortcut_token().unwrap();
    let output = cli(&paths)
        .env("PRISM_TEST_GAME_STATUS", "0")
        .args(["launch", &instance.id, "--shortcut-token", &token])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("Post-exit command failed"));
}
