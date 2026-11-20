//! prism-rs: PrismLauncher rewritten in Rust with GPUI.

mod accounts_window;
mod atlauncher_cli;
mod atlauncher_dialog;
mod cli_launch;
mod dialogs;
mod ftb_page;
mod instance_lock;
mod instance_window;
mod instance_settings_page;
mod main_window;
mod settings_window;
#[cfg(target_os = "linux")]
mod shortcut;
mod state;
mod technic_window;

use gpui_kit::*;
use prism_core::launch::QuickPlayTarget;
use state::App;
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    // Headless CLI: verify/test the launch pipeline without a GUI.
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("atlauncher") {
        std::process::exit(atlauncher_cli::run(&args[2..]));
    }
    if args.get(1).map(String::as_str) == Some("launch") {
        let (id, shortcut_token) = match &args[2..] {
            [id] => (id, None),
            [id, flag, token] if flag == "--shortcut-token" => (id, Some(token.as_str())),
            _ => {
                eprintln!("Usage: prism-rs launch <instance-id>");
                std::process::exit(2);
            }
        };
        match cli_launch::launch(id, shortcut_token) {
            Ok(code) => std::process::exit(code),
            Err(error) => {
                eprintln!("Launch failed: {error}");
                std::process::exit(1);
            }
        }
    }
    if args.get(1).map(String::as_str) == Some("copy") {
        let result = (|| -> Result<(), Box<dyn std::error::Error>> {
            if args.len() < 4 || args.len() > 5 { return Err("Usage: prism-rs copy <instance-id> <name> [group]".into()); }
            let paths = prism_core::Paths::new();
            let list = prism_core::InstanceList::load(&paths)?;
            let source = list.get(&args[2]).ok_or("Instance not found")?;
            let group = args.get(4).map(String::as_str).or(source.group.as_deref());
            let copied = prism_core::copy::copy_instance(&paths, source, &args[3], group, &Default::default())?;
            println!("Copied: {} ({})", copied.name, copied.id);
            Ok(())
        })();
        if let Err(error) = result { eprintln!("Copy failed: {error}"); std::process::exit(1); }
        return;
    }
    if args.get(1).map(String::as_str) == Some("export") {
        if args.len() != 4 {
            eprintln!("Usage: prism-rs export <instance-id> <out.zip|out.mrpack>");
            std::process::exit(1);
        }
        let result = (|| -> Result<(), Box<dyn std::error::Error>> {
            let paths = prism_core::Paths::new();
            let list = prism_core::InstanceList::load(&paths)?;
            let instance = list.get(&args[2]).ok_or("Instance not found")?;
            let output = std::path::Path::new(&args[3]);
            match output.extension().and_then(|s| s.to_str()) {
                Some("zip") => prism_core::export::export_zip(instance, output)?,
                Some("mrpack") => {
                    let metadata = prism_core::export::MrpackMetadata {
                        name: instance.name.clone(),
                        version: "1.0.0".into(),
                        summary: instance.notes.clone(),
                    };
                    tokio::runtime::Runtime::new()?.block_on(prism_core::export::export_mrpack(
                        instance, output, metadata,
                    ))?;
                }
                _ => return Err("Output must end in .zip or .mrpack".into()),
            }
            println!("Exported: {}", output.display());
            Ok(())
        })();
        if let Err(error) = result {
            eprintln!("Export failed: {error}");
            std::process::exit(1);
        }
        return;
    }
    if args.len() >= 2 && args[1] == "import" && args.len() >= 3 {
        let code = headless_import(&args[2]);
        std::process::exit(code);
    }
    if args.len() >= 2 && args[1] == "instances" {
        let code = headless_instances();
        std::process::exit(code);
    }
    if args.len() >= 3 && args[1] == "prepare" {
        let code = headless_prepare(&args[2], &args[3..]);
        std::process::exit(code);
    }

    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(|cx| {
            gpui_kit::init(cx);

            // Dark mode for all gpui-component widgets (inputs, buttons, dialogs).
            gpui_kit::component::theme::Theme::change(
                gpui_kit::component::theme::ThemeMode::Dark,
                None,
                cx,
            );

            let app = cx.new(|_| App::new());
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| main_window::MainWindow::new(app.clone(), window, cx))
            })
            .expect("failed to open main window");
        });
}

fn headless_instances() -> i32 {
    let paths = prism_core::Paths::new();
    println!("Data root: {}", paths.root.display());
    match prism_core::InstanceList::load(&paths) {
        Ok(list) => {
            if list.instances.is_empty() {
                println!("No instances found.");
            }
            for inst in &list.instances {
                if let prism_core::InstanceKind::Horizon { id, home, .. } = &inst.kind {
                    println!(
                        "- {:?} (id: {:?}, Horizon ID: {}, group {}, game dir: {})",
                        inst.name, inst.id, id, inst.group.as_deref().unwrap_or("-"), home.display()
                    );
                    continue;
                }
                let loader = prism_core::instance::loader_of(inst);
                let mc = prism_core::launch::LaunchSettings::for_instance(inst, &paths).mc_version;
                let group = inst.group.clone().unwrap_or_else(|| "-".to_string());
                let java = prism_core::launch::LaunchSettings::for_instance(inst, &paths)
                    .java_path
                    .display()
                    .to_string();
                println!(
                    "- {:?} (id: {:?}, mc {}, {:?}, group {}, game dir: {})",
                    inst.name,
                    inst.id,
                    mc,
                    loader,
                    group,
                    inst.game_root().display()
                );
                println!("  java: {java}");
                let mods = prism_core::resources::scan_folder(
                    &inst.mods_dir(),
                    prism_core::modplatform::ResourceType::Mod,
                );
                if !mods.is_empty() {
                    println!("  mods ({}):", mods.len());
                    for m in mods.iter().take(10) {
                        println!(
                            "    {} {} {:?}",
                            m.title.clone().unwrap_or_else(|| m.file_name.clone()),
                            m.version.clone().unwrap_or_default(),
                            m.loader
                        );
                    }
                }
            }
            0
        }
        Err(e) => {
            eprintln!("Failed to scan instances: {e}");
            1
        }
    }
}

fn headless_import(path: &str) -> i32 {
    let paths = prism_core::Paths::new();
    paths.ensure().expect("failed to create data dirs");
    let downloader = std::sync::Arc::new(prism_core::net::Downloader::new());
    let runtime = tokio::runtime::Runtime::new().expect("failed to create import runtime");
    match runtime.block_on(prism_core::import::import_source(&paths, path, &downloader)) {
        Ok(outcome) => {
            let inst = outcome.instance;
            println!("Imported: {} ({})", inst.name, inst.id);
            println!(
                "loader: {:?}",
                prism_core::instance::loader_of(&inst)
            );
            if let Some(issue) = outcome.curseforge_key_issue {
                eprintln!(
                    "{}: set a valid PRISM_RS_CURSEFORGE_API_KEY for automatic downloads.",
                    issue.message()
                );
            }
            for file in outcome.missing_files {
                eprintln!("Manual download needed: {file}");
            }
            0
        }
        Err(e) => {
            eprintln!("Import failed: {e}");
            1
        }
    }
}

fn headless_prepare(version: &str, args: &[String]) -> i32 {
    let paths = prism_core::Paths::new();
    let instance = if version == "--instance" {
        let [id] = args else {
            eprintln!("Usage: prism-rs prepare --instance <id>");
            return 1;
        };
        let list = match prism_core::InstanceList::load(&paths) {
            Ok(list) => list,
            Err(error) => {
                eprintln!("Could not load instances: {error}");
                return 1;
            }
        };
        match list.get(id).filter(|instance| !instance.is_horizon()) {
            Some(instance) => Some(instance.clone()),
            None => {
                eprintln!("Java instance not found: {id}");
                return 1;
            }
        }
    } else {
        None
    };
    let args = if instance.is_some() { &[][..] } else { args };
    let quick_play = match args {
        [] => None,
        [flag, world] if flag == "--world" && !world.is_empty() => {
            Some(QuickPlayTarget::Singleplayer {
                world: world.clone(),
            })
        }
        [flag, address] if flag == "--server" => match QuickPlayTarget::parse_server(address) {
            Ok(target) => Some(target),
            Err(error) => {
                eprintln!("{error}");
                return 1;
            }
        },
        _ => {
            eprintln!("Expected [--world <name> | --server <host[:port]>]");
            return 1;
        }
    };
    paths.ensure().expect("failed to create data dirs");
    let settings = if let Some(instance) = &instance {
        prism_core::launch::LaunchSettings::for_instance(instance, &paths)
    } else {
        prism_core::launch::LaunchSettings {
            quick_play,
            instance_name: "headless-test".into(),
            instance_id: "headless-test".into(),
            instance_dir: paths.instances_dir().join("headless-test"),
            game_dir: paths.instances_dir().join("headless-test").join(".minecraft"),
            mc_version: version.into(),
            java_path: prism_core::java::find_java().unwrap_or_else(|| "java".into()),
            java_path_explicit: false,
            auto_java_download: true,
            ignore_java_compatibility: false,
            min_memory_mb: 512,
            max_memory_mb: 2048,
            extra_jvm_args: vec![],
            raw_jvm_args: String::new(),
            window_width: 854,
            window_height: 480,
            env: vec![],
            pre_launch_command: String::new(),
            wrapper_command: String::new(),
            post_exit_command: String::new(),
        }
    };
    let mc_version = settings.mc_version.clone();
    let runtime = prism_core::net::CoreRuntime::new();
    let launcher = prism_core::launch::Launcher::new(paths.clone(), runtime.downloader.clone());

    runtime.handle().block_on(async {
        let version = mc_version.as_str();
        println!("Fetching version metadata for {version}…");
        let vd = launcher.fetch_version_data(version).await.expect("version fetch failed");
        let mut profile = prism_core::profile::LaunchProfile::from_mojang(&vd);
        if let Some(instance) = &instance {
            let loader = prism_core::instance::loader_of(instance);
            match &loader {
                prism_core::loaders::Loader::None => {}
                prism_core::loaders::Loader::Forge { .. }
                    if prism_core::modplatform::technic::apply_forge_patch(
                        &mut profile, &settings.instance_dir, &settings.game_dir, &loader,
                    ).expect("Technic Forge profile could not be read") => {}
                prism_core::loaders::Loader::Forge { .. }
                | prism_core::loaders::Loader::NeoForge { .. } => {
                    launcher.prepare_files(&profile, &settings)
                        .await.expect("vanilla prepare failed");
                    let versions = settings.game_dir.join("versions").join(version);
                    std::fs::create_dir_all(&versions).unwrap();
                    std::fs::copy(
                        launcher.client_jar_path(version),
                        versions.join(format!("{version}.jar")),
                    ).unwrap();
                    let overlay = prism_core::loaders::ensure_forge_installed(
                        &settings.java_path,
                        &loader,
                        version,
                        &settings.game_dir,
                        &runtime.downloader,
                    )
                    .await
                    .expect("loader installation failed");
                    profile.apply_loader(&overlay);
                }
                _ => {
                    let overlay = prism_core::loaders::fetch_loader_profile(
                        &runtime.downloader.client(), &loader, version,
                    )
                    .await
                    .expect("loader profile fetch failed");
                    profile.apply_loader(&overlay);
                }
            }
        }
        // Version-only preparation can select Fabric via PRISM_RS_FABRIC.
        if let Some(loader_version) = std::env::var("PRISM_RS_FABRIC")
            .ok()
            .filter(|_| instance.is_none())
        {
            if !loader_version.is_empty() {
                let loader = prism_core::loaders::Loader::Fabric { version: loader_version };
                let lvd = prism_core::loaders::fetch_loader_profile(
                    &runtime.downloader.client(),
                    &loader,
                    version,
                )
                .await
                .expect("loader profile fetch failed");
                profile.apply_loader(&lvd);
            }
        }
        if instance.is_some() {
            prism_core::modplatform::atlauncher::apply_launch(&settings.instance_dir, &mut profile)
                .expect("ATLauncher launch metadata failed");
        }
        println!(
            "mainClass: {}\nlibraries: {} (+{} natives)\nassets: {}\njava: {:?}",
            profile.main_class,
            profile.libraries.len(),
            profile.natives.len(),
            profile.assets_id,
            profile.java_major
        );

        std::fs::create_dir_all(&settings.game_dir).unwrap();

        let mut settings = settings;
        match launcher.resolve_java(&mut settings, &profile).await {
            Ok(message) => println!("{message}"),
            Err(e) => panic!("java selection failed: {e}"),
        }

        println!("Downloading game files…");
        launcher.prepare_files(&profile, &settings).await.expect("prepare failed");

        let mut accounts = prism_core::AccountList::load(&paths.accounts_file());
        let session = match accounts.session_for_launch(&paths.accounts_file()).await {
            Ok(session) => {
                session.unwrap_or_else(|| prism_core::launch::AuthSession::offline("Player"))
            }
            Err(error) => {
                eprintln!("Launch preparation failed: {error}");
                return 1;
            }
        };
        let prepared = launcher.build_command(&settings, Some(&session), &profile);
        println!("\nCommand line:");
        for arg in &prepared.command {
            println!("  {}", redact_launch_arg(arg, &session));
        }

        println!("Environment: {:?}", prepared.env);
        println!("Pre-launch: {}", prepared.pre_launch_command);
        println!("Post-exit: {}", prepared.post_exit_command);
        let script = prepared.shell_script().expect("invalid launch command");
        let script_path = paths.root.join("launch-headless.sh");
        write_launch_script(&script_path, &script).expect("failed to write private launch script");
        println!("\nLaunch script written to {}", script_path.display());
        0
    })
}

fn redact_launch_arg(arg: &str, session: &prism_core::launch::AuthSession) -> String {
    if session.user_type == "msa" && !session.access_token.is_empty() {
        arg.replace(&session.access_token, "<access token>")
    } else {
        arg.to_string()
    }
}

fn write_launch_script(path: &std::path::Path, script: &str) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(path)?;
    #[cfg(unix)]
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    file.set_len(0)?;
    file.write_all(script.as_bytes())
}

#[cfg(test)]
mod headless_tests {
    use super::{redact_launch_arg, write_launch_script};
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn printed_command_redacts_msa_tokens() {
        let mut session = prism_core::launch::AuthSession::offline("Player");
        assert_eq!(redact_launch_arg("token:0:0", &session), "token:0:0");
        session.user_type = "msa".into();
        session.access_token = "secret-token".into();
        assert_eq!(
            redact_launch_arg("secret-token", &session),
            "<access token>"
        );
        assert_eq!(
            redact_launch_arg("token:secret-token:uuid", &session),
            "token:<access token>:uuid"
        );
        assert_eq!(redact_launch_arg("--username", &session), "--username");
        session.access_token.clear();
        assert_eq!(redact_launch_arg("--username", &session), "--username");
    }

    #[test]
    fn script_retains_token_and_is_private_on_creation_and_rewrite() {
        let path = std::env::temp_dir().join(format!(
            "prism-script-{}-{}.sh",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let script = "#!/bin/sh\nexec java --accessToken 'secret-token'\n";
        for existing in [false, true] {
            if existing {
                #[cfg(unix)]
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
            }
            write_launch_script(&path, script).unwrap();
            assert_eq!(std::fs::read_to_string(&path).unwrap(), script);
            #[cfg(unix)]
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        std::fs::remove_file(path).unwrap();
        assert!(write_launch_script(&std::env::temp_dir(), script).is_err());
    }
}
