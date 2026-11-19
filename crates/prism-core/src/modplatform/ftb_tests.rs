use super::*;
use serde_json::json;
use std::io::{Read, Write};
use std::net::TcpListener;

fn pack() -> Pack {
    serde_json::from_value(json!({"id":1,"name":"Fixture","plays":5,"installs":2,"authors":[{"name":"FTB"}],"versions":[{"id":2,"name":"1.0","updated":3}]})).unwrap()
}

fn manifest(base: &str) -> Manifest {
    serde_json::from_value(json!({
        "id":2,"parent":1,"name":"1.0", "targets":[
            {"name":"minecraft","type":"game","version":"1.20.1"},
            {"name":"fabric","type":"modloader","version":"0.16.0"}],
        "files":[{"path":"./mods","name":"a.jar","url":format!("{base}/file"),"sha1":"aaf4c61ddcc5e8a2dabede0f3b482cd9aea9434d"},
        {"path":"../server","name":"server.jar","serveronly":true},
        {"path":"mods","name":"manual.jar","curseforge":{"project":"10","file":"20"}}]
    })).unwrap()
}

fn server(responses: Vec<(String, String, Vec<u8>)>) -> (String, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let thread = std::thread::spawn(move || {
        for (path, status, body) in responses {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            loop {
                let mut buf = [0; 1024];
                let n = socket.read(&mut buf).unwrap();
                if n == 0 {
                    break;
                }
                request.extend_from_slice(&buf[..n]);
                if request.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            let request = String::from_utf8_lossy(&request);
            assert!(
                request.starts_with(&format!("GET {path} HTTP/1.1")),
                "{request}"
            );
            assert!(request.to_lowercase().contains("user-agent: prism-rs/"));
            write!(socket, "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).unwrap();
            socket.write_all(&body).unwrap();
        }
    });
    (base, thread)
}

#[test]
fn parses_catalog_and_filters_sorts() {
    let metadata: Pack = serde_json::from_value(json!({"id":109,"name":"Fabric fixture","art":[{"type":"square","url":"https://example.com/icon.png"}],"tags":[{"name":"Fabric"}],"versions":[{"id":1,"name":"1.0","type":"beta","targets":[{"type":"game","name":"minecraft","version":"1.20.1"}]}]})).unwrap();
    assert_eq!(metadata.art[0].kind, "square");
    assert_eq!(metadata.tags[0].name, "Fabric");
    assert_eq!(metadata.versions[0].release_type, "beta");
    assert_eq!(metadata.versions[0].targets[0].version, "1.20.1");
    assert!(pack().art.is_empty());
    assert!(pack().tags.is_empty());
    assert!(pack().versions[0].targets.is_empty());
    let catalog: Catalog = serde_json::from_str(r#"{"packs":[1,2],"status":"success"}"#).unwrap();
    assert_eq!(catalog.packs, [1, 2]);
    let mut other = pack();
    other.id = 3;
    other.name = "Another".into();
    other.plays = 1;
    other.installs = 8;
    let packs = vec![pack(), other];
    assert_eq!(filter_catalog(&packs, " fiX ", Sort::Plays)[0].id, 1);
    assert_eq!(filter_catalog(&packs, "", Sort::Installs)[0].id, 3);
    assert_eq!(filter_catalog(&packs, "", Sort::Name)[0].id, 3);
    assert!(filter_catalog(&packs, "missing", Sort::Name).is_empty());
    assert!(serde_json::from_str::<Catalog>("{}").is_err());
}

#[test]
fn maps_loaders_and_rejects_traversal_and_bad_cli_sources() {
    for (name, uid) in [
        ("forge", "net.minecraftforge"),
        ("neoforge", "net.neoforged"),
        ("fabric", "net.fabricmc.fabric-loader"),
        ("quilt", "org.quiltmc.quilt-loader"),
    ] {
        assert_eq!(loader(name, "1").unwrap().uid(), uid);
    }
    assert!(loader("unknown", "1").is_err());
    assert!(loader("fabric", "").is_err());
    for value in ["../a", "/a", "a/../b", "C:/a", "a\\b", "", "."] {
        assert!(safe_path(value).is_err(), "{value}");
    }
    assert_eq!(safe_path("./config/a").unwrap(), PathBuf::from("config/a"));
    assert_eq!(
        Source::parse("ftb:1:2").unwrap(),
        Some(Source {
            pack: 1,
            version: Some(2)
        })
    );
    assert_eq!(
        Source::parse("ftb:1").unwrap(),
        Some(Source {
            pack: 1,
            version: None
        })
    );
    assert!(Source::parse("/tmp/app").unwrap().is_none());
    for value in ["ftb:", "ftb:0", "ftb:1:", "ftb:1:0", "ftb:1:2:3", "ftb:no"] {
        assert!(Source::parse(value).is_err());
    }
}

#[tokio::test]
async fn installs_skips_server_and_reports_missing_files() {
    let (base, thread) = server(vec![("/file".into(), "200 OK".into(), b"hello".to_vec())]);
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths {
        root: dir.path().into(),
    };
    let api = FtbApi::with_endpoint(Arc::new(Downloader::new()), base.clone());
    let mut flame = FlameApi::new();
    // Avoid depending on the developer's configured CurseForge key.
    flame.clear_test_key();
    let out = api
        .install_manifest(&paths, &pack(), &manifest(&base), &flame)
        .await
        .unwrap();
    thread.join().unwrap();
    assert_eq!(
        std::fs::read(out.instance.game_root().join("mods/a.jar")).unwrap(),
        b"hello"
    );
    assert_eq!(out.missing_files.len(), 1);
    assert_eq!(out.curseforge_key_issue, Some(CurseForgeKeyIssue::Missing));
    assert!(
        std::fs::read_to_string(out.instance.root.join(CURSEFORGE_MISSING_FILES_REPORT))
            .unwrap()
            .contains("mods/manual.jar")
    );
    assert_eq!(
        crate::instance::loader_of(&out.instance),
        loader("fabric", "0.16.0").unwrap()
    );
    let cfg = IniFile::load(&out.instance.root.join("instance.cfg")).unwrap();
    assert_eq!(cfg.get("ManagedPackType"), Some("ftb"));
    assert_eq!(out.instance.icon_key, "default");
    assert!(!crate::managed_pack::ManagedPack::present(&out.instance));
}

#[tokio::test]
async fn installs_square_artwork_best_effort() {
    let png = b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDR\x00\x00\x00\x01\x00\x00\x00\x01\x08\x04\x00\x00\x00\xb5\x1c\x0c\x02\x00\x00\x00\x0bIDAT\x78\xda\x63\x64\xf8\x0f\x00\x01\x05\x01\x01\x27\x18\xe3\x66\x00\x00\x00\x00IEND\xae\x42\x60\x82";
    for (status, blocked_storage, expected_icon) in [
        ("200 OK", false, true),
        ("404 Not Found", false, false),
        ("200 OK", true, false),
    ] {
        let (base, thread) = server(vec![("/icon".into(), status.into(), png.to_vec())]);
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            root: dir.path().into(),
        };
        if blocked_storage {
            std::fs::write(paths.icons_dir(), b"not a directory").unwrap();
        }
        let mut p = pack();
        p.art = vec![
            Artwork {
                kind: "banner".into(),
                url: format!("{base}/unused-banner"),
            },
            Artwork {
                kind: "square".into(),
                url: format!("{base}/icon"),
            },
        ];
        let mut m = manifest(&base);
        m.files.clear();
        let out = FtbApi::with_endpoint(Arc::new(Downloader::new()), base)
            .install_manifest(&paths, &p, &m, &FlameApi::new())
            .await
            .unwrap();
        thread.join().unwrap();
        let cfg = IniFile::load(&out.instance.root.join("instance.cfg")).unwrap();
        let key = if expected_icon { "ftb-1" } else { "default" };
        assert_eq!(out.instance.icon_key, key);
        assert_eq!(cfg.get("iconKey"), Some(key));
        assert_eq!(cfg.get("ManagedPackType"), Some("ftb"));
        if expected_icon {
            assert_eq!(
                std::fs::read(paths.icons_dir().join("ftb-1.png")).unwrap(),
                png
            );
        }
        assert_eq!(std::fs::read_dir(&out.instance.root).unwrap().count(), 3);
    }
}

#[tokio::test]
async fn rejects_bad_hash_and_rolls_back_instance() {
    let (base, thread) = server(vec![("/file".into(), "200 OK".into(), b"wrong".to_vec())]);
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths {
        root: dir.path().into(),
    };
    let api = FtbApi::with_endpoint(Arc::new(Downloader::new()), base.clone());
    assert!(api
        .install_manifest(&paths, &pack(), &manifest(&base), &FlameApi::new())
        .await
        .is_err());
    thread.join().unwrap();
    assert_eq!(std::fs::read_dir(paths.instances_dir()).unwrap().count(), 0);
    let mut bad = manifest(&base);
    bad.files[0].path = "../escape".into();
    assert!(api
        .install_manifest(&paths, &pack(), &bad, &FlameApi::new())
        .await
        .is_err());
}

#[tokio::test]
async fn catalog_caches_empty_and_handles_errors() {
    for (status, body, ok) in [
        ("200 OK", r#"{"packs":[]}"#, true),
        ("403 Forbidden", "{}", false),
        ("200 OK", "not json", false),
    ] {
        let (base, thread) = server(vec![(
            "/modpack/all".into(),
            status.into(),
            body.as_bytes().to_vec(),
        )]);
        let api = FtbApi::with_endpoint(Arc::new(Downloader::new()), base);
        assert_eq!(api.catalog().await.is_ok(), ok);
        if ok {
            assert!(api.catalog().await.unwrap().is_empty());
        }
        thread.join().unwrap();
    }
}

#[tokio::test]
async fn fetches_pack_and_latest_manifest_over_http() {
    let p = json!({"id":1,"name":"HTTP fixture","versions":[{"id":2,"name":"old","updated":1},{"id":3,"name":"new","updated":2}]});
    let m = json!({"id":3,"parent":1,"name":"new","targets":[{"name":"minecraft","type":"game","version":"1.20.1"}],"files":[]});
    let (base, thread) = server(vec![
        (
            "/modpack/1".into(),
            "200 OK".into(),
            p.to_string().into_bytes(),
        ),
        (
            "/modpack/1/3".into(),
            "200 OK".into(),
            m.to_string().into_bytes(),
        ),
    ]);
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths {
        root: dir.path().into(),
    };
    let out = FtbApi::with_endpoint(Arc::new(Downloader::new()), base)
        .install(&paths, 1, None)
        .await
        .unwrap();
    thread.join().unwrap();
    assert!(out.missing_files.is_empty());
    assert_eq!(
        IniFile::load(&out.instance.root.join("instance.cfg"))
            .unwrap()
            .get("ManagedPackVersionID"),
        Some("3")
    );
}

#[tokio::test]
async fn resolves_url_less_files_through_curseforge() {
    let (base, thread) = server(vec![("/file".into(), "200 OK".into(), b"hello".to_vec())]);
    let body = json!({"data":{"id":20,"downloadUrl":format!("{base}/file")}});
    let (cf_base, cf_thread) = server(vec![(
        "/mods/10/files/20".into(),
        "200 OK".into(),
        body.to_string().into_bytes(),
    )]);
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths {
        root: dir.path().into(),
    };
    let api = FtbApi::with_endpoint(Arc::new(Downloader::new()), base.clone());
    let mut m = manifest(&base);
    m.files.truncate(1);
    m.files[0].url = None;
    m.files[0].curseforge = Some(CurseForgeFile {
        project: 10,
        file: 20,
    });
    let out = api
        .install_manifest(&paths, &pack(), &m, &FlameApi::with_test_endpoint(cf_base))
        .await
        .unwrap();
    thread.join().unwrap();
    cf_thread.join().unwrap();
    assert!(out.missing_files.is_empty());
    assert_eq!(
        std::fs::read(out.instance.game_root().join("mods/a.jar")).unwrap(),
        b"hello"
    );
}
