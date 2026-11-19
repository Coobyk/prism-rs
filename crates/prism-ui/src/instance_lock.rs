use prism_core::{Instance, Paths};
use std::{
    fs, io,
    process::{Child, Command},
};

/// Hold the returned file until the game exits. Leave it on disk so every process locks the same file.
pub fn acquire(paths: &Paths, instance: &Instance) -> io::Result<fs::File> {
    let dir = paths.logs_dir().join("launch-locks");
    fs::create_dir_all(&dir)?;
    let file = fs::File::options()
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join(&instance.id))?;
    fs2::FileExt::try_lock_exclusive(&file).map_err(|error| {
        if error.raw_os_error() == fs2::lock_contended_error().raw_os_error() {
            io::Error::new(
                io::ErrorKind::WouldBlock,
                format!(
                    "Instance {:?} is already launching or running",
                    instance.name
                ),
            )
        } else {
            error
        }
    })?;
    Ok(file)
}

pub fn spawn(command: &mut Command, lock: &fs::File) -> io::Result<Child> {
    #[cfg(unix)]
    {
        use std::{os::fd::AsRawFd, os::unix::process::CommandExt};
        let fd = lock.as_raw_fd();
        // Keep the same flock in the game if the launcher exits. Clear
        // close-on-exec only in this child, never in the multithreaded parent.
        // SAFETY: The lock stays open throughout spawn; fcntl is async-signal-safe.
        unsafe {
            command.pre_exec(move || {
                if libc::fcntl(fd, libc::F_SETFD, 0) == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    #[cfg(not(unix))]
    let _ = lock;
    command.spawn()
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use prism_core::InstanceList;
    use std::process::Stdio;

    #[test]
    fn the_game_keeps_the_lock_after_the_launcher_closes_its_file() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            root: dir.path().into(),
        };
        let instance = InstanceList::create(&paths, "World", "default", "1.20.1").unwrap();
        let lock = acquire(&paths, &instance).unwrap();
        let mut child = spawn(
            Command::new("/bin/sh")
                .args(["-c", "read -r stop"])
                .stdin(Stdio::piped()),
            &lock,
        )
        .unwrap();
        drop(lock);
        let blocked = acquire(&paths, &instance);
        child.stdin.take();
        child.wait().unwrap();
        assert_eq!(blocked.unwrap_err().kind(), io::ErrorKind::WouldBlock);
        assert!(acquire(&paths, &instance).is_ok());
    }
}
