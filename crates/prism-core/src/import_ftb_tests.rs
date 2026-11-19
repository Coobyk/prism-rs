use super::*;
use serde_json::json;

fn fixture(root: &Path) -> PathBuf {
    let app = root.join("source/uuid");
    std::fs::create_dir_all(app.join(".ftbapp")).unwrap();
    std::fs::create_dir_all(app.join("saves/world")).unwrap();
    std::fs::write(app.join("saves/world/level.dat"), b"world").unwrap();
    std::fs::write(app.join("instance.json"), json!({"uuid":"test","id":10,"versionId":20,"name":"Local fixture","version":"1.0","mcVersion":"1.20.1","totalPlayTime":123456,"jvmArgs":"-Dfixture=true"}).to_string()).unwrap();
    std::fs::write(
        app.join(".ftbapp/version.json"),
        json!({"targets":[{"name":"quilt","type":"modloader","version":"0.27.0"}]}).to_string(),
    )
    .unwrap();
    app
}

#[tokio::test]
async fn shared_dispatch_imports_local_paths_and_rejects_invalid_sources() {
    let dir = tempfile::tempdir().unwrap();
    let app = fixture(dir.path());
    let paths = Paths {
        root: dir.path().join("launcher"),
    };
    let downloader = std::sync::Arc::new(crate::net::Downloader::new());
    let out = crate::import::import_source(&paths, app.to_str().unwrap(), &downloader)
        .await
        .unwrap();
    assert_eq!(out.instance.name, "Local fixture");
    assert_eq!(out.instance.total_time_played, 123);
    for source in ["", "ftb:0", "ftb:invalid"] {
        assert!(crate::import::import_source(&paths, source, &downloader)
            .await
            .is_err());
    }
    let missing = dir.path().join("missing");
    assert!(
        crate::import::import_source(&paths, missing.to_str().unwrap(), &downloader)
            .await
            .is_err()
    );
}

#[test]
fn scans_and_imports_settings_worlds_and_legacy_loader() {
    let dir = tempfile::tempdir().unwrap();
    let app = fixture(dir.path());
    let source_bytes = std::fs::read(app.join("instance.json")).unwrap();
    let paths = Paths {
        root: dir.path().join("launcher"),
    };
    assert_eq!(crate::import::detect_format(&app), "ftb-app");
    let instances = scan(app.parent().unwrap()).unwrap();
    assert_eq!(instances.len(), 1);
    let out = crate::import::import_sync(&paths, &app, None).unwrap();
    assert_eq!(out.instance.total_time_played, 123);
    assert_eq!(
        crate::instance::loader_of(&out.instance),
        loader("quilt", "0.27.0").unwrap()
    );
    let cfg = IniFile::load(&out.instance.root.join("instance.cfg")).unwrap();
    assert_eq!(cfg.get("JvmArgs"), Some("-Dfixture=true"));
    assert_eq!(cfg.get("OverrideJavaArgs"), Some("true"));
    assert_eq!(cfg.get("ManagedPackID"), Some("10"));
    assert_eq!(
        std::fs::read(out.instance.game_root().join("saves/world/level.dat")).unwrap(),
        b"world"
    );
    assert_eq!(
        std::fs::read(app.join("instance.json")).unwrap(),
        source_bytes
    );
    let logo = hex::decode("89504e470d0a1a0a0000000d49484452000000010000000108060000001f15c4890000000b49444154789c636000020000050001a5f645400000000049454e44ae426082").unwrap();
    let mut wrapped = vec![0];
    wrapped.extend_from_slice(&logo);
    std::fs::write(app.join(".ftbapp/logo"), wrapped).unwrap();
    let with_icon = import(&paths, &app).unwrap();
    assert_eq!(
        std::fs::read(
            paths
                .icons_dir()
                .join(format!("{}.png", with_icon.instance.icon_key))
        )
        .unwrap(),
        logo
    );
    std::fs::rename(app.join(".ftbapp/version.json"), app.join("version.json")).unwrap();
    assert_eq!(
        AppInstance::read(&app).unwrap().loader().unwrap(),
        loader("quilt", "0.27.0").unwrap()
    );
}

#[test]
fn discovers_fallback_and_handles_empty_and_invalid_instances() {
    let dir = tempfile::tempdir().unwrap();
    assert!(discover_in(dir.path()).is_none());
    assert!(scan(dir.path()).unwrap().is_empty());
    std::fs::create_dir_all(dir.path().join("bin")).unwrap();
    std::fs::write(
        dir.path().join("bin/settings.json"),
        r#"{"instanceLocation":"/example/instances"}"#,
    )
    .unwrap();
    assert_eq!(
        discover_in(dir.path()).unwrap(),
        PathBuf::from("/example/instances")
    );
    assert!(AppInstance::read(dir.path()).is_err());
    assert!(scan(&dir.path().join("missing")).is_err());
    std::fs::write(dir.path().join("instance.json"), "{}").unwrap();
    assert!(AppInstance::read(dir.path()).is_err());
}

#[cfg(unix)]
#[test]
fn rejects_symlinks_and_rolls_back() {
    let dir = tempfile::tempdir().unwrap();
    let app = fixture(dir.path());
    std::os::unix::fs::symlink("/etc/passwd", app.join("escape")).unwrap();
    let paths = Paths {
        root: dir.path().join("launcher"),
    };
    assert!(import(&paths, &app).is_err());
    assert_eq!(std::fs::read_dir(paths.instances_dir()).unwrap().count(), 0);
}
