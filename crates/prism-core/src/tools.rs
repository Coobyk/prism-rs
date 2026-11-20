//! External editors and PID-attached profilers.
use crate::settings::Settings;
use std::{
    io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    JProfiler,
    JVisualVM,
    MCEdit,
}

impl Tool {
    pub const ALL: [Self; 3] = [Self::JProfiler, Self::JVisualVM, Self::MCEdit];
    pub fn name(self) -> &'static str {
        match self {
            Self::JProfiler => "JProfiler",
            Self::JVisualVM => "JVisualVM",
            Self::MCEdit => "MCEdit",
        }
    }
    pub fn key(self) -> &'static str {
        match self {
            Self::JProfiler => "JProfilerPath",
            Self::JVisualVM => "JVisualVMPath",
            Self::MCEdit => "MCEditPath",
        }
    }
    pub fn path(self, settings: &Settings) -> PathBuf {
        settings
            .get(self.key())
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                if self == Self::JVisualVM {
                    detect_visualvm().unwrap_or_default()
                } else {
                    PathBuf::new()
                }
            })
    }
    pub fn check(self, path: &Path) -> Result<(), &'static str> {
        if path.as_os_str().is_empty() {
            return Err("Empty path");
        }
        if !path.exists() {
            return Err("Path does not exist");
        }
        match self {
            Self::JProfiler
                if path.is_dir()
                    && path.join("bin/agent.jar").is_file()
                    && ["bin/jprofiler", "bin/jprofiler.exe"]
                        .iter()
                        .any(|p| path.join(p).is_file())
                    && path
                        .join(if cfg!(windows) {
                            "bin/jpenable.exe"
                        } else {
                            "bin/jpenable"
                        })
                        .is_file() =>
            {
                Ok(())
            }
            Self::JVisualVM
                if executable(path)
                    && path
                        .file_name()
                        .is_some_and(|n| n.to_string_lossy().contains("visualvm")) =>
            {
                Ok(())
            }
            Self::MCEdit if self.editor_program(path).is_some() => Ok(()),
            Self::JProfiler => Err("Invalid JProfiler install"),
            Self::JVisualVM => Err("Invalid path to VisualVM"),
            Self::MCEdit => Err("Path does not seem to be a MCEdit path"),
        }
    }
    fn editor_program(self, path: &Path) -> Option<PathBuf> {
        if cfg!(target_os = "macos") {
            return path.join("Contents").is_dir().then(|| path.to_path_buf());
        }
        let names = if cfg!(windows) {
            ["mcedit.exe", "mcedit2.exe"]
        } else {
            ["mcedit.sh", "mcedit.py"]
        };
        names
            .iter()
            .map(|name| path.join(name))
            .find(|p| p.is_file())
    }
    pub fn profiler_command(self, settings: &Settings, pid: u32) -> io::Result<Command> {
        let path = self.path(settings);
        self.check(&path).map_err(io::Error::other)?;
        let mut command = match self {
            Self::JProfiler => Command::new(path.join(if cfg!(windows) {
                "bin/jpenable.exe"
            } else {
                "bin/jpenable"
            })),
            Self::JVisualVM => Command::new(path),
            Self::MCEdit => return Err(io::Error::other("MCEdit is not a profiler")),
        };
        match self {
            Self::JProfiler => {
                command.args([
                    "-d",
                    &pid.to_string(),
                    "--gui",
                    "-p",
                    &settings.get_int("JProfilerPort", 42042).to_string(),
                ]);
            }
            _ => {
                command.args(["--openpid", &pid.to_string()]);
            }
        }
        Ok(command)
    }
    pub fn attach(self, settings: &Settings, pid: u32, log: impl Fn(String) + Send + 'static) {
        let result = self
            .profiler_command(settings, pid)
            .and_then(|mut c| c.stdout(Stdio::null()).stderr(Stdio::null()).spawn());
        match result {
            Ok(mut child) => {
                log(if self == Self::JProfiler {
                    format!(
                        "Listening on port: {}",
                        settings.get_int("JProfilerPort", 42042)
                    )
                } else {
                    "VisualVM started".into()
                });
                std::thread::spawn(move || match child.wait() {
                    Ok(status) if status.success() => {}
                    Ok(status) => log(format!("Profiler aborted: {status}")),
                    Err(error) => log(format!("Profiler aborted: {error}")),
                });
            }
            Err(error) => log(format!("Could not start {}: {error}", self.name())),
        }
    }
    pub fn open_world(self, settings: &Settings, world: &Path) -> io::Result<()> {
        if self != Self::MCEdit {
            return Err(io::Error::other("Only MCEdit can open worlds"));
        }
        let path = std::path::absolute(self.path(settings))?;
        let world = std::path::absolute(world)?;
        self.check(&path).map_err(io::Error::other)?;
        let mut command = if cfg!(target_os = "macos") {
            let mut c = Command::new("open");
            c.arg("-a").arg(&path).arg("--args");
            c
        } else {
            Command::new(
                self.editor_program(&path)
                    .ok_or_else(|| io::Error::other("MCEdit executable not found"))?,
            )
        };
        let mut child = command
            .arg(world)
            .current_dir(path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(())
    }
}

fn executable(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        true
    }
}

pub fn detect_visualvm() -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    for name in ["jvisualvm", "visualvm"] {
        for dir in std::env::split_paths(&paths) {
            let path = dir.join(if cfg!(windows) {
                format!("{name}.exe")
            } else {
                name.into()
            });
            if executable(&path) {
                return Some(path);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn visualvm_path_falls_back_for_missing_and_empty_settings() {
        let mut settings = Settings::default();
        assert_eq!(settings.get("JVisualVMPath"), Some(""));
        let detected = detect_visualvm().unwrap_or_default();
        assert_eq!(Tool::JVisualVM.path(&settings), detected);
        settings.ini.remove("JVisualVMPath");
        assert_eq!(Tool::JVisualVM.path(&settings), detected);
        settings.set("JVisualVMPath", "custom/visualvm");
        assert_eq!(
            Tool::JVisualVM.path(&settings),
            PathBuf::from("custom/visualvm")
        );
    }

    #[test]
    fn external_tools_validation_and_commands() {
        let dir = tempfile::tempdir().unwrap();
        for tool in Tool::ALL {
            assert!(tool.check(Path::new("")).is_err());
            assert!(tool.check(&dir.path().join("missing")).is_err());
            assert!(tool.check(dir.path()).is_err());
        }
        std::fs::create_dir(dir.path().join("bin")).unwrap();
        for name in [
            "agent.jar",
            "jprofiler",
            if cfg!(windows) {
                "jpenable.exe"
            } else {
                "jpenable"
            },
        ] {
            std::fs::write(dir.path().join("bin").join(name), "").unwrap();
        }
        let mut settings = Settings::default();
        settings.set("JProfilerPath", dir.path().to_string_lossy());
        settings.set("JProfilerPort", "12345");
        assert!(Tool::JProfiler.check(dir.path()).is_ok());
        let command = Tool::JProfiler.profiler_command(&settings, 123).unwrap();
        assert_eq!(
            command.get_program(),
            dir.path().join("bin").join(if cfg!(windows) {
                "jpenable.exe"
            } else {
                "jpenable"
            })
        );
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            ["-d", "123", "--gui", "-p", "12345"]
        );
        settings.ini.remove("JProfilerPort");
        assert_eq!(
            Tool::JProfiler
                .profiler_command(&settings, 123)
                .unwrap()
                .get_args()
                .last()
                .unwrap(),
            "42042"
        );
        std::fs::remove_file(command.get_program()).unwrap();
        assert!(Tool::JProfiler.check(dir.path()).is_err());
        let vm = dir.path().join("visualvm");
        std::fs::write(&vm, "").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert!(Tool::JVisualVM.check(&vm).is_err());
            std::fs::set_permissions(&vm, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        settings.set("JVisualVMPath", vm.to_string_lossy());
        assert_eq!(
            Tool::JVisualVM
                .profiler_command(&settings, 456)
                .unwrap()
                .get_program(),
            vm
        );
        assert_eq!(
            Tool::JVisualVM
                .profiler_command(&settings, 456)
                .unwrap()
                .get_args()
                .collect::<Vec<_>>(),
            ["--openpid", "456"]
        );
        assert!(Tool::MCEdit.profiler_command(&settings, 1).is_err());
        if cfg!(target_os = "macos") {
            std::fs::create_dir(dir.path().join("Contents")).unwrap();
        } else {
            std::fs::write(
                dir.path().join(if cfg!(windows) {
                    "mcedit.exe"
                } else {
                    "mcedit.sh"
                }),
                "",
            )
            .unwrap();
        }
        assert!(Tool::MCEdit.check(dir.path()).is_ok());
        let config = dir.path().join("prismlauncher.cfg");
        settings.set("MCEditPath", dir.path().to_string_lossy());
        settings.save(&config).unwrap();
        let loaded = Settings::load(&config);
        for tool in Tool::ALL {
            assert_eq!(tool.path(&loaded), tool.path(&settings));
        }
    }

    #[test]
    fn external_tools_attach_reports_invalid_configuration() {
        let mut settings = Settings::default();
        settings.set("JVisualVMPath", "");
        let (tx, rx) = std::sync::mpsc::channel();
        Tool::JVisualVM.attach(&settings, 123, move |message| {
            tx.send(message).unwrap();
        });
        assert_eq!(rx.recv().unwrap(), "Could not start JVisualVM: Empty path");
    }
}
