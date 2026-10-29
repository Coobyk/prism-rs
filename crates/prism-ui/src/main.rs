//! prism-rs: PrismLauncher rewritten in Rust with GPUI.

mod dialogs;
mod main_window;
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
    // Usage: prism-rs prepare <mc-version> [--offline-name NAME]
    let args: Vec<String> = std::env::args().collect();
    if args.len() >= 3 && args[1] == "prepare" {
        let code = headless_prepare(&args[2]);
        std::process::exit(code);
    }

    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(|cx| {
            gpui_kit::init(cx);

            let app = cx.new(|_| App::new());
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| main_window::MainWindow::new(app.clone(), window, cx))
            })
            .expect("failed to open main window");
        });
}

fn headless_prepare(version: &str) -> i32 {
    let paths = prism_core::Paths::new();
    paths.ensure().expect("failed to create data dirs");
    let runtime = prism_core::net::CoreRuntime::new();
    let launcher = prism_core::launch::Launcher::new(paths.clone(), runtime.downloader.clone());

    runtime.handle().block_on(async {
        println!("Fetching version metadata for {version}…");
        let vd = launcher.fetch_version_data(version).await.expect("version fetch failed");
        let profile = prism_core::profile::LaunchProfile::from_mojang(&vd);
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
