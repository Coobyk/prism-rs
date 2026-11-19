//! Adapter for the official Horizon launcher. Horizon owns game installs and accounts.

use crate::settings::Settings;
use serde::Deserialize;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

pub const LAUNCHER_PATH_SETTING: &str = "HorizonLauncherPath";
pub const DOWNLOAD_URL: &str = "https://playhorizon.dev/download";
const LINK_MARKER: &str = ".prism-horizon-link";

pub(crate) fn mark_link(home: &Path) -> io::Result<uuid::Uuid> {
    let token = uuid::Uuid::new_v4();
    let mut file = std::fs::File::options()
        .write(true)
        .create_new(true)
        .open(home.join(LINK_MARKER))?;
    write!(file, "{token}")?;
    Ok(token)
}

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
    #[error("Horizon instance {0:?} was not found. Check it in the Horizon launcher.")]
    MissingInstance(String),
    #[error("Cannot verify the Prism link for Horizon instance {0:?}. The instance may have been replaced, or the link may predate identity checks.")]
    UnverifiedLink(String),
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

    pub fn play(&self, id: &str, token: Option<uuid::Uuid>) -> Result<(Child, PathBuf), HorizonError> {
        let instance = self.list()?.into_iter().find(|instance| instance.id == id)
            .ok_or_else(|| HorizonError::MissingInstance(id.to_string()))?;
        let found = std::fs::read_to_string(instance.home.join(LINK_MARKER))
            .ok()
            .and_then(|text| text.parse().ok());
        if token.is_none() || found != token {
            return Err(HorizonError::UnverifiedLink(id.to_string()));
        }
        Command::new(&self.program)
            .arg("play")
            .arg(format!("--instance={id}"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map(|child| (child, instance.home))
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

    struct FakeLauncher(PathBuf, PathBuf);

    impl FakeLauncher {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!("prism-horizon-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&dir).unwrap();
            let script = dir.join("fake horizon-launcher");
            let home = dir.join("game folder");
            fs::create_dir(&home).unwrap();
            let instance = serde_json::json!({"id":"a-name","name":"A name with spaces","home":home,"java":null});
            fs::write(dir.join("create.json"), instance.to_string()).unwrap();
            fs::write(dir.join("list.json"), serde_json::json!([
                instance, {"id":"java","name":"Java","home":"/java","java":{}}
            ]).to_string()).unwrap();
            fs::write(&script, r#"#!/bin/sh
case "$1:$2:$3:$#" in
  --json:instance:create:5)
    [ "$4" = '--' ] && [ "$5" = 'A name with spaces' ] || {
        echo 'Horizon rejected the instance name' >&2; exit 2;
    }
    cat "$(dirname "$0")/create.json"
    ;;
  --json:instance:list:3)
    cat "$(dirname "$0")/list.json"
    ;;
  play:--instance=a-name::2)
    echo 'error: Sign in with a Microsoft account' >&2
    exit 1
    ;;
  *) echo 'unexpected Horizon arguments' >&2; exit 2 ;;
esac
"#).unwrap();
            fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
            Self(script, home)
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
        assert_eq!(instance.home, fake.1);
        let listed = launcher.list().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, instance.id);

        let token = mark_link(&instance.home).unwrap();
        let (child, home) = launcher.play(&instance.id, Some(token)).unwrap();
        assert_eq!(home, instance.home);
        let output = child.wait_with_output().unwrap();
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
        let error = launcher.play("missing", Some(uuid::Uuid::new_v4())).unwrap_err();
        assert!(matches!(error, HorizonError::MissingInstance(_)));

        fs::remove_file(&fake.0).unwrap();
        let error = launcher.list().unwrap_err().to_string();
        assert!(error.contains(DOWNLOAD_URL));
        assert!(error.contains("Settings"));
    }

    #[test]
    fn reused_id_and_home_cannot_launch_from_an_old_link() {
        let fake = FakeLauncher::new();
        let launcher = fake.launcher();
        let token = mark_link(&fake.1).unwrap();
        fs::remove_dir_all(&fake.1).unwrap();
        fs::create_dir(&fake.1).unwrap();
        mark_link(&fake.1).unwrap();

        for token in [Some(token), None] {
            assert!(matches!(launcher.play("a-name", token), Err(HorizonError::UnverifiedLink(_))));
        }
    }

    #[test]
    fn moved_home_is_resolved_by_its_link_token() {
        let fake = FakeLauncher::new();
        let launcher = fake.launcher();
        let token = mark_link(&fake.1).unwrap();
        let moved = fake.1.with_file_name("moved home");
        fs::rename(&fake.1, &moved).unwrap();
        fs::write(fake.1.with_file_name("list.json"), serde_json::json!([
            {"id":"a-name","name":"A name with spaces","home":moved,"java":null}
        ]).to_string()).unwrap();

        let (child, home) = launcher.play("a-name", Some(token)).unwrap();
        assert_eq!(home, moved);
        assert_eq!(child.wait_with_output().unwrap().status.code(), Some(1));
    }
}
