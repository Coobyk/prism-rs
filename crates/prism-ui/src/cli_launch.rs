use prism_core::horizon::HorizonLauncher;
use prism_core::launch::{LaunchSettings, Launcher};
use prism_core::{AccountList, InstanceKind, InstanceList, Paths};
use std::{error::Error, io, process::Child};

pub fn launch(id: &str, shortcut_token: Option<&str>) -> Result<i32, Box<dyn Error>> {
    let mut paths = Paths::new();
    paths.root = std::path::absolute(&paths.root)?;
    let list = InstanceList::load(&paths)?;
    let instance = list
        .get(id)
        .ok_or_else(|| format!("Instance not found: {id:?}"))?;
    let _lock = crate::instance_lock::acquire(&paths, instance)?;
    if let Some(token) = shortcut_token {
        if !instance.shortcut_token().is_ok_and(|current| current == token) {
            return Err("This shortcut no longer points to the original instance. Create it again from the launcher.".into());
        }
    }

    if let InstanceKind::Horizon { id, token, .. } = &instance.kind {
        println!("Launching Horizon instance {:?}", instance.name);
        let settings = prism_core::settings::Settings::load(&paths.global_config());
        let (child, _) = HorizonLauncher::from_settings(&settings).play(id, *token)?;
        return Ok(wait_for_game(child)?);
    }

    let mut settings = LaunchSettings::for_instance(instance, &paths);
    println!(
        "Launching instance {:?} (Minecraft {})",
        instance.name, settings.mc_version
    );
    let child = tokio::runtime::Runtime::new()?.block_on(async {
        let launcher = Launcher::new(paths.clone(), Default::default());
        let profile = launcher
            .prepare_instance(
                &mut settings,
                prism_core::instance::loader_of(instance),
                |message| {
                    println!("{message}");
                },
            )
            .await?;
        let accounts_path = paths.accounts_file();
        let session = AccountList::load(&accounts_path)
            .session_for_launch(&accounts_path)
            .await?;
        let prepared = launcher.build_command(&settings, session.as_ref(), &profile);
        Ok::<_, Box<dyn Error>>(prepared.spawn()?)
    })?;
    Ok(wait_for_game(child)?)
}

fn wait_for_game(mut child: Child) -> io::Result<i32> {
    std::thread::scope(|scope| {
        if let Some(mut pipe) = child.stdout.take() {
            scope.spawn(move || {
                let _ = io::copy(&mut pipe, &mut io::stdout());
            });
        }
        if let Some(mut pipe) = child.stderr.take() {
            scope.spawn(move || {
                let _ = io::copy(&mut pipe, &mut io::stderr());
            });
        }
        let status = child.wait()?;
        if !status.success() {
            eprintln!("Game exited with {status}");
        }
        Ok(status.code().unwrap_or(1))
    })
}
