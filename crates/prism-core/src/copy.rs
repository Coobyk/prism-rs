//! Independent instance copies with PrismLauncher-compatible content filters.
use crate::{Instance, InstanceGroups, Paths};
use std::{fs, io, path::Path};

#[derive(Clone, Debug)]
pub struct CopyOptions {
    pub saves: bool,
    pub game_options: bool,
    pub resource_packs: bool,
    pub shader_packs: bool,
    pub servers: bool,
    pub mods: bool,
    pub screenshots: bool,
    pub keep_playtime: bool,
}

impl Default for CopyOptions {
    fn default() -> Self {
        Self {
            saves: true,
            game_options: true,
            resource_packs: true,
            shader_packs: true,
            servers: true,
            mods: true,
            screenshots: true,
            keep_playtime: true,
        }
    }
}

pub fn copy_instance(
    paths: &Paths,
    source: &Instance,
    name: &str,
    group: Option<&str>,
    options: &CopyOptions,
) -> io::Result<Instance> {
    let mut instance = copy_instance_files(paths, source, name, options)?;
    instance.group = group.filter(|g| !g.trim().is_empty()).map(str::to_string);
    let mut groups = InstanceGroups::load(&paths.instances_dir());
    groups.set_group(&instance.id, instance.group.as_deref());
    if let Err(error) = groups.save(&paths.instances_dir()) {
        let _ = fs::remove_dir_all(&instance.root);
        return Err(error);
    }
    Ok(instance)
}

/// Copy files without touching shared group metadata. GUI callers must assign
/// the returned instance's group through their current App state on the UI thread.
pub fn copy_instance_files(
    paths: &Paths,
    source: &Instance,
    name: &str,
    options: &CopyOptions,
) -> io::Result<Instance> {
    if name.trim().is_empty() || source.is_horizon() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "A nonempty name and Java instance are required",
        ));
    }
    fs::create_dir_all(paths.instances_dir())?;
    let base: String = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let mut n = 1;
    let root = loop {
        let candidate = paths.instances_dir().join(if n == 1 {
            base.clone()
        } else {
            format!("{base}{n}")
        });
        match fs::create_dir(&candidate) {
            Ok(()) => break candidate,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => n += 1,
            Err(e) => return Err(e),
        }
    };
    let result = (|| {
        copy_tree(&source.root, &root, Path::new(""), options)?;
        let mut instance = Instance::load(&root).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "Source instance has no valid instance.cfg",
            )
        })?;
        instance.name = name.trim().to_string();
        if !options.keep_playtime {
            instance.total_time_played = 0;
            instance.last_launch = 0;
        }
        instance.save()?;
        let mut cfg = crate::ini::IniFile::load(&root.join("instance.cfg"))?;
        cfg.set("lastLaunchTime", instance.last_launch.to_string());
        if !options.keep_playtime {
            cfg.set("lastTimePlayed", "0");
        }
        cfg.set("InstanceUUID", uuid::Uuid::new_v4().to_string());
        cfg.save(&root.join("instance.cfg"))?;
        Ok(instance)
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&root);
    }
    result
}

fn copy_tree(source: &Path, dest: &Path, relative: &Path, options: &CopyOptions) -> io::Result<()> {
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let name = entry.file_name();
        if relative.as_os_str().is_empty() && Instance::is_launch_state_file(&name) {
            continue;
        }
        let lower = name.to_string_lossy().to_ascii_lowercase();
        let game = relative == Path::new("minecraft") || relative == Path::new(".minecraft");
        if (relative.as_os_str().is_empty() || game)
            && (matches!(
                lower.as_str(),
                "logs"
                    | "crash-reports"
                    | "session.lock"
                    | "instance.lock"
                    | "prismlauncher.log"
                    | "mmc-launcher.jar"
            ) || lower.ends_with(".log")
                || lower.starts_with("hs_err_pid"))
        {
            continue;
        }
        if game
            && match lower.as_str() {
                "saves" => !options.saves,
                "options.txt" => !options.game_options,
                "resourcepacks" | "texturepacks" => !options.resource_packs,
                "shaderpacks" => !options.shader_packs,
                "servers.dat" | "servers.dat_old" | "server-resource-packs" => !options.servers,
                "mods" | "coremods" | "config" => !options.mods,
                "screenshots" => !options.screenshots,
                _ => false,
            }
        {
            continue;
        }
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Copying symbolic links is not supported",
            ));
        }
        let target = dest.join(&name);
        if kind.is_dir() {
            fs::create_dir(&target)?;
            copy_tree(&entry.path(), &target, &relative.join(&name), options)?;
        } else if kind.is_file() {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn background_copies_do_not_overwrite_group_edits() {
        let paths = Paths {
            root: std::env::temp_dir().join(format!("prism-copy-{}", uuid::Uuid::new_v4())),
        };
        let source = crate::InstanceList::create(&paths, "Source", "default", "1.21").unwrap();
        let mut groups = InstanceGroups::default();
        groups.set_group(&source.id, Some("Original"));
        groups.save(&paths.instances_dir()).unwrap();
        let (first, second) = std::thread::scope(|scope| {
            let first = scope.spawn(|| {
                copy_instance_files(&paths, &source, "Copy", &CopyOptions::default()).unwrap()
            });
            let second = scope.spawn(|| {
                copy_instance_files(&paths, &source, "Copy", &CopyOptions::default()).unwrap()
            });
            groups.set_group(&source.id, Some("Edited while copying"));
            groups.save(&paths.instances_dir()).unwrap();
            (first.join().unwrap(), second.join().unwrap())
        });
        assert_ne!(first.id, second.id);
        assert!(first.group.is_none());
        assert!(second.group.is_none());
        let saved = InstanceGroups::load(&paths.instances_dir());
        assert_eq!(
            saved.group_of(&source.id).as_deref(),
            Some("Edited while copying")
        );
        assert!(saved.group_of(&first.id).is_none());
        assert!(saved.group_of(&second.id).is_none());
        for instance in [&first, &second] {
            groups.set_group(&instance.id, Some("Copies"));
            groups.save(&paths.instances_dir()).unwrap();
        }
        let saved = InstanceGroups::load(&paths.instances_dir());
        assert_eq!(
            saved.group_of(&source.id).as_deref(),
            Some("Edited while copying")
        );
        for instance in [&first, &second] {
            assert_eq!(saved.group_of(&instance.id).as_deref(), Some("Copies"));
        }
        fs::remove_dir_all(paths.root).unwrap();
    }

    #[test]
    fn nested_world_content_is_not_filtered_as_junk() {
        let paths = Paths {
            root: std::env::temp_dir().join(format!("prism-copy-{}", uuid::Uuid::new_v4())),
        };
        let source = crate::InstanceList::create(&paths, "Source", "default", "1.21").unwrap();
        for world in ["Logs", "x.log"] {
            let dir = source.game_root().join("saves").join(world);
            fs::create_dir_all(&dir).unwrap();
            for file in ["level.dat", "session.lock", "hs_err_pid123.log"] {
                fs::write(dir.join(file), "world content").unwrap();
            }
        }
        for root in [&source.root, &source.game_root()] {
            fs::write(root.join("launcher.log"), "junk").unwrap();
            fs::create_dir(root.join("logs")).unwrap();
        }
        let copied = copy_instance(&paths, &source, "Copy", None, &CopyOptions::default()).unwrap();
        for world in ["Logs", "x.log"] {
            for file in ["level.dat", "session.lock", "hs_err_pid123.log"] {
                assert_eq!(
                    fs::read_to_string(copied.game_root().join("saves").join(world).join(file))
                        .unwrap(),
                    "world content"
                );
            }
        }
        for root in [&copied.root, &copied.game_root()] {
            assert!(!root.join("launcher.log").exists());
            assert!(!root.join("logs").exists());
        }
        fs::remove_dir_all(paths.root).unwrap();
    }

    #[test]
    fn copy_options_identity_and_errors() {
        let paths = Paths {
            root: std::env::temp_dir().join(format!("prism-copy-{}", uuid::Uuid::new_v4())),
        };
        let mut source = crate::InstanceList::create(&paths, "Source", "default", "1.21").unwrap();
        source.total_time_played = 100;
        source.last_launch = 123;
        source.save().unwrap();
        let names = [
            "saves",
            "options.txt",
            "resourcepacks",
            "texturepacks",
            "shaderpacks",
            "servers.dat",
            "servers.dat_old",
            "server-resource-packs",
            "mods",
            "coremods",
            "config",
            "screenshots",
            "logs",
        ];
        for name in names {
            fs::write(source.game_root().join(name), "data").unwrap();
        }
        let all = copy_instance(
            &paths,
            &source,
            "Source",
            Some("Copies"),
            &CopyOptions::default(),
        )
        .unwrap();
        assert_ne!(source.id, all.id);
        assert_eq!(all.name, "Source");
        assert_eq!(all.total_time_played, 100);
        assert_eq!(all.last_launch, 123);
        assert_eq!(
            InstanceGroups::load(&paths.instances_dir())
                .group_of(&all.id)
                .as_deref(),
            Some("Copies")
        );
        for name in names {
            assert_eq!(
                all.game_root().join(name).exists(),
                name != "logs",
                "{name}"
            );
        }
        let none = CopyOptions {
            saves: false,
            game_options: false,
            resource_packs: false,
            shader_packs: false,
            servers: false,
            mods: false,
            screenshots: false,
            keep_playtime: false,
        };
        let empty = copy_instance(&paths, &source, "Source", None, &none).unwrap();
        assert_ne!(all.id, empty.id);
        assert_eq!(empty.total_time_played, 0);
        assert_eq!(Instance::load(&empty.root).unwrap().last_launch, 0);
        for name in names {
            assert!(!empty.game_root().join(name).exists(), "{name}");
        }
        assert!(copy_instance(&paths, &source, "  ", None, &none).is_err());
        fs::remove_file(source.root.join("instance.cfg")).unwrap();
        assert!(copy_instance(&paths, &source, "Broken", None, &none).is_err());
        assert!(!paths.instances_dir().join("Broken").exists());
        fs::remove_dir_all(paths.root).unwrap();
    }
}
