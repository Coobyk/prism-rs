use prism_core::{Instance, Paths};
use std::{fs, io};

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
