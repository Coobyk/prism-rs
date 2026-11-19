use prism_core::horizon::HorizonLauncher;
use prism_core::launch::{LaunchSettings, Launcher};
use prism_core::{AccountList, Instance, InstanceKind, InstanceList, Paths};
use std::{error::Error, io, process::{Child, Command, Stdio}};

pub fn launch(id: &str, shortcut_token: Option<&str>) -> Result<i32, Box<dyn Error>> {
    let mut paths = Paths::new();
    paths.root = std::path::absolute(&paths.root)?;
    let list = InstanceList::load(&paths)?;
    let instance = list
        .get(id)
        .ok_or_else(|| format!("Instance not found: {id:?}"))?;
    let lock = crate::instance_lock::acquire(&paths, instance)?;
    if let Some(token) = shortcut_token {
        if !instance
            .shortcut_token()
            .is_ok_and(|current| current == token)
        {
            return Err("This shortcut no longer points to the original instance. Create it again from the launcher.".into());
        }
    }

    if let InstanceKind::Horizon { id, home, token } = &instance.kind {
        println!("Launching Horizon instance {:?}", instance.name);
        let settings = prism_core::settings::Settings::load(&paths.global_config());
        let (child, current_home) =
            HorizonLauncher::from_settings(&settings).play_with(id, *token, |command| {
                spawn_attached(command, &lock)
            })?;
        if *home != current_home {
            if let Some(mut stored) = Instance::load(&instance.root) {
                if matches!(&stored.kind, InstanceKind::Horizon { id: stored_id, token: stored_token, .. } if stored_id == id && stored_token == token)
                {
                    if let Err(error) = stored.save_horizon_home(current_home) {
                        eprintln!("Could not save the moved Horizon game folder: {error}");
                    }
                }
            }
        }
        return Ok(wait_for_game(child)?);
    }

    let mut settings = LaunchSettings::for_instance(instance, &paths);
    let accounts_path = paths.accounts_file();
    let account_id = AccountList::load(&accounts_path).active_account().map(|account| account.id.clone());
    println!(
        "Launching instance {:?} (Minecraft {})",
        instance.name, settings.mc_version
    );
    let (child, prepared) = tokio::runtime::Runtime::new()?.block_on(async {
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
        let session = match account_id {
            Some(id) => AccountList::session_for_account(&accounts_path, &id).await?,
            None => None,
        };
        let prepared = launcher.build_command(&settings, session.as_ref(), &profile);
        let child = prepared.spawn_with(None, |command| spawn_attached(command, &lock))?;
        Ok::<_, Box<dyn Error>>((child, prepared))
    })?;
    let status = wait_for_game(child)?;
    if let Err(error) = prepared.run_post_exit() {
        eprintln!("Post-exit command failed: {error}");
        if shortcut_token.is_some() && status == 0 {
            return Ok(1);
        }
    }
    Ok(status)
}

fn spawn_attached(command: &mut Command, lock: &std::fs::File) -> io::Result<Child> {
    command.stdout(Stdio::inherit()).stderr(Stdio::inherit());
    crate::instance_lock::spawn(command, lock)
}

fn wait_for_game(mut child: Child) -> io::Result<i32> {
    let status = child.wait()?;
    if !status.success() {
        eprintln!("Game exited with {status}");
    }
    Ok(status.code().unwrap_or(1))
}
