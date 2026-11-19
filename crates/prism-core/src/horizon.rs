//! Adapter for the official Horizon launcher. Horizon owns game installs and accounts.

use crate::settings::Settings;
use serde::Deserialize;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

pub const LAUNCHER_PATH_SETTING: &str = "HorizonLauncherPath";
pub const DOWNLOAD_URL: &str = "https://playhorizon.dev/download";

#[derive(Debug, Deserialize)]
pub struct HorizonInstance {
    pub id: String,
    pub name: String,
    pub home: PathBuf,
    java: Option<serde_json::Value>,
}

impl HorizonInstance {
    fn is_horizon(&self) -> bool {
        self.java.is_none() && !self.id.is_empty() && self.home.is_absolute()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum HorizonError {
    #[error("Could not start {program:?}: {source}. Install Horizon from {DOWNLOAD_URL} and add horizon-launcher to PATH or set its path in Settings.")]
    Start { program: PathBuf, source: io::Error },
    #[error("Horizon launcher exited with {status}: {message}")]
    Failed { status: std::process::ExitStatus, message: String },
    #[error("Invalid response from Horizon launcher: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Horizon launcher did not return a valid Horizon instance")]
    InvalidInstance,
    #[error("Horizon instance {0:?} was not found at its linked game folder. Check it in the Horizon launcher.")]
    MissingInstance(String),
}

pub struct HorizonLauncher {
    program: PathBuf,
}

impl HorizonLauncher {
    pub fn from_settings(settings: &Settings) -> Self {
        let program = settings
            .get(LAUNCHER_PATH_SETTING)
            .filter(|path| !path.is_empty())
            .unwrap_or("horizon-launcher");
        Self { program: program.into() }
    }

    pub fn create(&self, name: &str) -> Result<HorizonInstance, HorizonError> {
        let instance: HorizonInstance = self.json(&["instance", "create", "--", name])?;
        if !instance.is_horizon() {
            return Err(HorizonError::InvalidInstance);
        }
        Ok(instance)
    }

    pub fn list(&self) -> Result<Vec<HorizonInstance>, HorizonError> {
        let mut instances: Vec<HorizonInstance> = self.json(&["instance", "list"])?;
        instances.retain(HorizonInstance::is_horizon);
        Ok(instances)
    }

    pub fn play(&self, id: &str, home: &Path) -> Result<Child, HorizonError> {
        if !self.list()?.iter().any(|instance| instance.id == id && instance.home == home) {
            return Err(HorizonError::MissingInstance(id.to_string()));
        }
        Command::new(&self.program)
            .arg("play")
            .arg(format!("--instance={id}"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|source| self.start_error(source))
    }

    fn json<T: serde::de::DeserializeOwned>(&self, args: &[&str]) -> Result<T, HorizonError> {
        let output = Command::new(&self.program)
            .arg("--json")
            .args(args)
            .output()
            .map_err(|source| self.start_error(source))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let message = if stderr.trim().is_empty() {
                String::from_utf8_lossy(&output.stdout).trim().to_string()
            } else {
                stderr.trim().to_string()
            };
            return Err(HorizonError::Failed { status: output.status, message });
        }
        Ok(serde_json::from_slice(&output.stdout)?)
    }

    fn start_error(&self, source: io::Error) -> HorizonError {
        HorizonError::Start { program: self.program.clone(), source }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    struct FakeLauncher(PathBuf);

    impl FakeLauncher {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!("prism-horizon-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&dir).unwrap();
            let script = dir.join("fake horizon-launcher");
            fs::write(&script, r#"#!/bin/sh
case "$1:$2:$3:$#" in
  --json:instance:create:5)
    [ "$4" = '--' ] && [ "$5" = 'A name with spaces' ] || {
        echo 'Horizon rejected the instance name' >&2; exit 2;
    }
    printf '%s\n' '{"id":"a-name","name":"A name with spaces","home":"/game folder","java":null}'
    ;;
  --json:instance:list:3)
    printf '%s\n' '[{"id":"a-name","name":"A name with spaces","home":"/game folder","java":null},{"id":"java","name":"Java","home":"/java","java":{}}]'
    ;;
  play:--instance=a-name::2)
    echo 'error: Sign in with a Microsoft account' >&2
    exit 1
    ;;
  *) echo 'unexpected Horizon arguments' >&2; exit 2 ;;
esac
"#).unwrap();
            fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
            Self(script)
        }

        fn launcher(&self) -> HorizonLauncher {
            let mut settings = Settings::default();
            settings.set(LAUNCHER_PATH_SETTING, self.0.to_str().unwrap());
            HorizonLauncher::from_settings(&settings)
        }
    }

    impl Drop for FakeLauncher {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(self.0.parent().unwrap());
        }
    }

    #[test]
    fn create_list_and_play_use_horizon_cli_arguments() {
        let fake = FakeLauncher::new();
        let launcher = fake.launcher();
        let instance = launcher.create("A name with spaces").unwrap();
        assert_eq!(instance.id, "a-name");
        assert_eq!(instance.name, "A name with spaces");
        assert_eq!(instance.home, PathBuf::from("/game folder"));
        let listed = launcher.list().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, instance.id);

        let output = launcher.play(&instance.id, &instance.home).unwrap().wait_with_output().unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert_eq!(String::from_utf8(output.stderr).unwrap().trim(), "error: Sign in with a Microsoft account");
    }

    #[test]
    fn reports_failed_commands_and_missing_launcher_or_link() {
        let fake = FakeLauncher::new();
        let launcher = fake.launcher();
        let error = launcher.create("wrong argument").unwrap_err().to_string();
        assert!(error.contains("exit status: 2"));
        assert!(error.contains("Horizon rejected the instance name"));
        let error = launcher.play("a-name", Path::new("/different-home")).unwrap_err();
        assert!(matches!(error, HorizonError::MissingInstance(_)));

        fs::remove_file(&fake.0).unwrap();
        let error = launcher.list().unwrap_err().to_string();
        assert!(error.contains(DOWNLOAD_URL));
        assert!(error.contains("Settings"));
    }
}
