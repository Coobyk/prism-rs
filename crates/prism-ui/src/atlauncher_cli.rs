use prism_core::{modplatform::atlauncher as atl, net::Downloader, Paths};

pub fn run(args: &[String]) -> i32 {
    match tokio::runtime::Runtime::new()
        .map_err(|e| e.to_string())
        .and_then(|rt| rt.block_on(execute(args)))
    {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("ATLauncher: {e}");
            1
        }
    }
}
async fn execute(args: &[String]) -> Result<(), String> {
    let dl = Downloader::new();
    let command = args.first().map(String::as_str).unwrap_or("");
    if command == "list" && args.len() <= 2 {
        let query = args.get(1).map(|s| s.to_lowercase()).unwrap_or_default();
        for p in atl::list(&dl)
            .await
            .map_err(|e| e.to_string())?
            .iter()
            .filter(|p| p.name.to_lowercase().contains(&query))
        {
            println!(
                "{}: {}",
                p.name,
                p.versions
                    .iter()
                    .map(|v| format!("{} (MC {})", v.version, v.minecraft))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        return Ok(());
    }
    if command != "install" || args.len() < 3 {
        return Err("Usage: atlauncher list [query] | atlauncher install <pack> <version> [--name NAME] [--optional a,b | --recommended]".into());
    }
    let (pack, version) = (&args[1], &args[2]);
    let mut name = pack.as_str();
    let mut optional = None;
    let mut recommended = false;
    let mut i = 3;
    while i < args.len() {
        match args[i].as_str() {
            "--name" => {
                i += 1;
                name = args.get(i).ok_or("Missing --name value")?;
            }
            "--optional" => {
                i += 1;
                optional = Some(args.get(i).ok_or("Missing --optional value")?.as_str());
            }
            "--recommended" => recommended = true,
            _ => return Err(format!("Unknown option: {}", args[i])),
        }
        i += 1;
    }
    if recommended && optional.is_some() {
        return Err("Choose --optional or --recommended, not both".into());
    }
    let packs = atl::list(&dl).await.map_err(|e| e.to_string())?;
    let p = packs
        .iter()
        .find(|p| {
            p.name.eq_ignore_ascii_case(pack) || atl::safe_name(&p.name).eq_ignore_ascii_case(pack)
        })
        .ok_or("Unknown public ATLauncher pack")?;
    if !p.versions.iter().any(|v| &v.version == version) {
        return Err("Unknown pack version".into());
    }
    let m = atl::manifest(&dl, &p.name, version)
        .await
        .map_err(|e| e.to_string())?;
    if let Some(message) = m.messages.get("install") {
        println!("Install message: {message}");
    }
    let mut selection = if recommended {
        atl::Selection::recommended(&m)
    } else if optional.is_some() {
        atl::Selection::clear(&m)
    } else {
        atl::Selection::defaults(&m)
    }
    .map_err(|e| e.to_string())?;
    if let Some(optional) = optional {
        for name in optional.split(',').filter(|s| !s.is_empty()) {
            selection.set(&m, name, true).map_err(|e| e.to_string())?;
        }
    }
    for entry in m
        .mods
        .iter()
        .filter(|e| selection.enabled.contains(&e.name) && !e.warning.is_empty())
    {
        if let Some(warning) = m.warnings.get(&entry.warning) {
            eprintln!("Warning ({}): {warning}", entry.name);
        }
    }
    let paths = Paths::new();
    paths.ensure().map_err(|e| e.to_string())?;
    println!(
        "Installing {} {} (Minecraft {})",
        p.name, version, m.minecraft
    );
    let instance = atl::install(&paths, &dl, &p.name, version, name, &m, &selection)
        .await
        .map_err(|e| e.to_string())?;
    println!("Installed: {} ({})", instance.name, instance.id);
    Ok(())
}
