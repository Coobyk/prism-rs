//! prism-rs: PrismLauncher rewritten in Rust with GPUI.

mod accounts_window;
mod dialogs;
mod instance_window;
mod main_window;
mod settings_window;
mod state;

use gpui_kit::*;
use state::App;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    // Headless CLI: verify/test the launch pipeline without a GUI.
    // Usage: prism-rs prepare <mc-version> | import <pack-file>
    let args: Vec<String> = std::env::args().collect();
    if args.len() >= 2 && args[1] == "import" && args.len() >= 3 {
        let code = headless_import(&args[2]);
        std::process::exit(code);
    }
    if args.len() >= 2 && args[1] == "instances" {
        let code = headless_instances();
        std::process::exit(code);
    }
    if args.len() >= 3 && args[1] == "prepare" {
        let code = headless_prepare(&args[2]);
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
                let loader = prism_core::instance::loader_of(inst);
                let mc = prism_core::launch::LaunchSettings::for_instance(inst, &paths).mc_version;
                let group = inst.group.clone().unwrap_or_else(|| "-".to_string());
                let java = prism_core::launch::LaunchSettings::for_instance(inst, &paths)
                    .java_path
                    .display()
                    .to_string();
                println!(
                    "- {:?} (mc {}, {:?}, group {}, game dir: {})",
                    inst.name,
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
    match prism_core::import::import_sync(&paths, std::path::Path::new(path), Some(&downloader)) {
        Ok(inst) => {
            println!("Imported: {} ({})", inst.name, inst.id);
            println!(
                "loader: {:?}",
                prism_core::instance::loader_of(&inst)
            );
            0
        }
        Err(e) => {
            eprintln!("Import failed: {e}");
            1
        }
    }
}

fn headless_prepare(version: &str) -> i32 {
    let paths = prism_core::Paths::new();
    paths.ensure().expect("failed to create data dirs");
    let runtime = prism_core::net::CoreRuntime::new();
    let launcher = prism_core::launch::Launcher::new(paths.clone(), runtime.downloader.clone());

    runtime.handle().block_on(async {
        println!("Fetching version metadata for {version}…");
        let vd = launcher.fetch_version_data(version).await.expect("version fetch failed");
        let mut profile = prism_core::profile::LaunchProfile::from_mojang(&vd);
        // Loader overlay if the caller set PRISM_RS_LOADER (e.g. "0.16.14").
        if let Ok(loader_version) = std::env::var("PRISM_RS_FABRIC") {
            if !loader_version.is_empty() {
                let loader = prism_core::loaders::Loader::Fabric { version: loader_version };
                let lvd = prism_core::loaders::fetch_loader_profile(
                    runtime.downloader.client(),
                    &loader,
                    version,
                )
                .await
                .expect("loader profile fetch failed");
                profile.apply_loader(&lvd);
            }
        }
        println!(
            "mainClass: {}\nlibraries: {} (+{} natives)\nassets: {}\njava: {:?}",
            profile.main_class,
            profile.libraries.len(),
            profile.natives.len(),
            profile.assets_id,
            profile.java_major
        );

        let game_dir = paths.instances_dir().join("headless-test").join(".minecraft");
        std::fs::create_dir_all(&game_dir).unwrap();
        let settings = prism_core::launch::LaunchSettings {
            instance_name: "headless-test".into(),
            instance_id: "headless-test".into(),
            game_dir: game_dir.clone(),
            mc_version: version.into(),
            java_path: prism_core::java::find_java().unwrap_or_else(|| "java".into()),
            min_memory_mb: 512,
            max_memory_mb: 2048,
            extra_jvm_args: vec![],
            window_width: 854,
            window_height: 480,
            env: vec![],
        };

        println!("Downloading game files…");
        launcher.prepare_files(&profile, &settings).await.expect("prepare failed");

        let session = prism_core::launch::AuthSession::offline("Player");
        let prepared = launcher.build_command(&settings, Some(&session), &profile);
        println!("\nCommand line:");
        for arg in &prepared.command {
            println!("  {arg}");
        }

        // Emit a runnable script (run paths through the same substitution).
        let script = format!(
            "#!/bin/sh\ncd '{}'\nexec {}\n",
            prepared.working_dir.display(),
            prepared
                .command
                .iter()
                .map(|a| format!("'{}'", a.replace('\'', "'\\''")))
                .collect::<Vec<_>>()
                .join(" ")
        );
        let script_path = paths.root.join("launch-headless.sh");
        std::fs::write(&script_path, script).unwrap();
        println!("\nLaunch script written to {}", script_path.display());
    });
    0
}
