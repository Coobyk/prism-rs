//! Application state shared across the UI.

use prism_core::launch::{AuthSession, LaunchSettings, PreparedLaunch};
use prism_core::meta::VersionManifest;
use prism_core::net::CoreRuntime;
use prism_core::{AccountList, InstanceList, Paths};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, PartialEq)]
pub enum InstanceRunState {
    Idle,
    Preparing,
    Running,
    Crashed(String),
    Finished,
}

#[derive(Clone)]
pub struct RunningInstance {
    pub log: Arc<Mutex<Vec<String>>>,
    pub state: Arc<Mutex<InstanceRunState>>,
    pub pid: Arc<Mutex<Option<u32>>>,
}

impl RunningInstance {
    fn new() -> RunningInstance {
        RunningInstance {
            log: Arc::new(Mutex::new(Vec::new())),
            state: Arc::new(Mutex::new(InstanceRunState::Preparing)),
            pid: Arc::new(Mutex::new(None)),
        }
    }
}

pub struct App {
    pub paths: Paths,
    pub runtime: Arc<CoreRuntime>,
    pub instances: InstanceList,
    pub accounts: AccountList,
    pub selected: Option<String>,
    pub running: BTreeMap<String, RunningInstance>,
    pub manifest: Option<Arc<VersionManifest>>,
    pub manifest_loading: bool,
    pub manifest_handle: Option<tokio::task::JoinHandle<Result<VersionManifest, prism_core::net::NetError>>>,
    pub toast: Option<(String, std::time::Instant)>,
}

impl App {
    pub fn new() -> App {
        let paths = Paths::new();
        if let Err(e) = paths.ensure() {
            tracing::warn!("failed to create data dirs: {e}");
        }
        let accounts = AccountList::load(&paths.accounts_file());
        let instances = InstanceList::load(&paths).unwrap_or_else(|e| {
            tracing::error!("failed to load instances: {e}");
            InstanceList::default()
        });
        App {
            runtime: Arc::new(CoreRuntime::new()),
            paths,
            instances,
            accounts,
            selected: None,
            running: BTreeMap::new(),
            manifest: None,
            manifest_loading: false,
            manifest_handle: None,
            toast: None,
        }
    }

    pub fn reload_instances(&mut self) {
        self.instances = InstanceList::load(&self.paths).unwrap_or_else(|e| {
            tracing::error!("failed to load instances: {e}");
            InstanceList::default()
        });
    }

    pub fn save_accounts(&self) {
        if let Err(e) = self.accounts.save(&self.paths.accounts_file()) {
            tracing::error!("failed to save accounts: {e}");
        }
    }

    pub fn selected_instance(&self) -> Option<&prism_core::Instance> {
        self.selected.as_deref().and_then(|id| self.instances.get(id))
    }

    /// Status bar description of the selected instance, mirroring
    /// `MainWindow::getStatusbarDescription()`.
    pub fn status_description(&self) -> String {
        match self.selected_instance() {
            Some(inst) => {
                let played = format_play_time(inst.total_time_played);
                match self.run_state(&inst.id) {
                    InstanceRunState::Preparing => format!("{} • Preparing…", inst.name),
                    InstanceRunState::Running => format!("{} • Running", inst.name),
                    InstanceRunState::Crashed(e) => format!("{} • Crashed: {e}", inst.name),
                    InstanceRunState::Finished => format!("{} • Finished", inst.name),
                    InstanceRunState::Idle => format!("Name: {} • Play time: {played}", inst.name),
                }
            }
            None => "No instance selected".to_string(),
        }
    }

    pub fn run_state(&self, id: &str) -> InstanceRunState {
        self.running
            .get(id)
            .map(|r| r.state.lock().map(|s| s.clone()).unwrap_or(InstanceRunState::Idle))
            .unwrap_or(InstanceRunState::Idle)
    }

    pub fn is_running(&self, id: &str) -> bool {
        matches!(self.run_state(id), InstanceRunState::Preparing | InstanceRunState::Running)
    }

    pub fn instance_icon_path(&self, inst: &prism_core::Instance) -> Option<PathBuf> {
        let dir = self.paths.icons_dir();
        for ext in ["png", "svg", "jpg", "jpeg", "webp", "ico"] {
            let p = dir.join(format!("{}.{ext}", inst.icon_key));
            if p.exists() {
                return Some(p);
            }
        }
        None
    }

    /// Kick off launching the instance. The heavy lifting runs on the core
    /// tokio runtime; progress lands in `running[id]`.
    pub fn start_launch(&mut self, id: &str) {
        if self.is_running(id) {
            return;
        }
        let Some(inst) = self.instances.get(id).cloned() else {
            return;
        };
        let running = RunningInstance::new();
        self.running.insert(id.to_string(), running.clone());

        let settings = LaunchSettings::for_instance(&inst, &self.paths);
        let session: Option<AuthSession> = self
            .accounts
            .active_account()
            .map(|acc| self.accounts.to_session(acc));
        let launcher = prism_core::launch::Launcher::new(self.paths.clone(), self.runtime.downloader.clone());
        let downloader = self.runtime.downloader.clone();
        let log = running.log.clone();
        let state = running.state.clone();

        log_lock(&log).push(format!(
            "Launching instance \"{}\" (Minecraft {})",
            inst.name, settings.mc_version
        ));

        let id_owned = id.to_string();
        self.runtime.spawn(async move {
            // 1. Version metadata.
            state_lock(&state, |s| *s = InstanceRunState::Preparing);
            let version_data = match launcher.fetch_version_data(&settings.mc_version).await {
                Ok(vd) => vd,
                Err(e) => {
                    log_lock(&log).push(format!("Fatal: {e}"));
                    state_lock(&state, |s| *s = InstanceRunState::Crashed(e.to_string()));
                    return;
                }
            };
            let profile = prism_core::profile::LaunchProfile::from_mojang(&version_data);
            log_lock(&log).push(format!(
                "Resolved profile: mainClass={}, libraries={}, natives={}",
                profile.main_class,
                profile.libraries.len(),
                profile.natives.len()
            ));

            // 2. Files.
            if let Err(e) = launcher.prepare_files(&profile, &settings).await {
                log_lock(&log).push(format!("Fatal: {e}"));
                state_lock(&state, |s| *s = InstanceRunState::Crashed(e.to_string()));
                return;
            }
            log_lock(&log).push("Downloads complete".to_string());

            // 3. Command + spawn.
            let prepared = launcher.build_command(&settings, session.as_ref(), &profile);
            log_lock(&log).push(format!("Command: {}", prepared.command.join(" ")));
            match prepared.spawn() {
                Ok(mut child) => {
                    *pid_lock(&running.pid) = Some(child.id());
                    state_lock(&state, |s| *s = InstanceRunState::Running);
                    log_lock(&log).push(format!("Minecraft process ID: {}", child.id()));

                    // Pump stdout/stderr into the log (std threads: the child
                    // handles are plain std::process).
                    if let Some(stdout) = child.stdout.take() {
                        let log2 = log.clone();
                        std::thread::spawn(move || {
                            use std::io::BufRead;
                            let reader = std::io::BufReader::new(stdout);
                            for line in reader.lines().map_while(Result::ok) {
                                log_lock(&log2).push(line);
                            }
                        });
                    }
                    if let Some(stderr) = child.stderr.take() {
                        let log2 = log.clone();
                        std::thread::spawn(move || {
                            use std::io::BufRead;
                            let reader = std::io::BufReader::new(stderr);
                            for line in reader.lines().map_while(Result::ok) {
                                log_lock(&log2).push(line);
                            }
                        });
                    }

                    // Wait on a dedicated thread so the async task returns.
                    let log2 = log.clone();
                    let state2 = state.clone();
                    let pid2 = running.pid.clone();
                    std::thread::spawn(move || {
                        let status = child.wait();
                        match status {
                            Ok(code) if code.success() => {
                                state_lock(&state2, |s| *s = InstanceRunState::Finished);
                                log_lock(&log2).push("Game exited normally".to_string());
                            }
                            Ok(code) => {
                                let e = format!("exit code {code}");
                                state_lock(&state2, |s| *s = InstanceRunState::Crashed(e.clone()));
                                log_lock(&log2).push(format!("Game crashed: {e}"));
                            }
                            Err(e) => {
                                state_lock(&state2, |s| *s = InstanceRunState::Crashed(e.to_string()));
                                log_lock(&log2).push(format!("Process error: {e}"));
                            }
                        }
                        *pid_lock(&pid2) = None;
                    });
                }
                Err(e) => {
                    let msg = format!("Could not launch Minecraft: {e}");
                    log_lock(&log).push(msg.clone());
                    state_lock(&state, |s| *s = InstanceRunState::Crashed(msg));
                }
            }
            let _ = downloader;
            let _ = id_owned;
        });
    }

    pub fn kill_instance(&mut self, id: &str) {
        if let Some(running) = self.running.get(id) {
            if let Ok(Some(pid)) = running.pid.lock().map(|p| *p) {
                // Best-effort kill through the OS.
                #[cfg(unix)]
                {
                    let _ = std::process::Command::new("kill")
                        .arg(pid.to_string())
                        .status();
                }
            }
        }
    }

    pub fn start_manifest_fetch(&mut self) {
        if self.manifest.is_some() || self.manifest_handle.is_some() {
            return;
        }
        self.manifest_loading = true;
        let downloader = self.runtime.downloader.clone();
        self.manifest_handle = Some(self.runtime.spawn(async move {
            prism_core::meta::VersionManifest::fetch(downloader.client()).await
        }));
    }

    /// Non-blocking poll of the in-flight manifest fetch (safe to call every
    /// render).
    pub fn poll_manifest(&mut self) {
        let finished = self
            .manifest_handle
            .as_ref()
            .map(|h| h.is_finished())
            .unwrap_or(false);
        if !finished {
            return;
        }
        let handle = self.manifest_handle.take().unwrap();
        match self.block_on(handle) {
            Ok(Ok(m)) => {
                self.manifest = Some(Arc::new(m));
                self.manifest_loading = false;
            }
            Ok(Err(e)) => {
                tracing::error!("manifest fetch failed: {e}");
                self.manifest_loading = false;
            }
            Err(e) => {
                tracing::error!("manifest join failed: {e}");
                self.manifest_loading = false;
            }
        }
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

pub fn format_play_time(seconds: u64) -> String {
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    if hours > 0 {
        format!("{hours}h {minutes}m")
    } else {
        format!("{minutes}m")
    }
}

fn log_lock(log: &Arc<Mutex<Vec<String>>>) -> std::sync::MutexGuard<'_, Vec<String>> {
    log.lock().unwrap_or_else(|p| p.into_inner())
}

fn state_lock(state: &Arc<Mutex<InstanceRunState>>, f: impl FnOnce(&mut InstanceRunState)) {
    let mut guard = state.lock().unwrap_or_else(|p| p.into_inner());
    f(&mut guard);
}

fn pid_lock(pid: &Arc<Mutex<Option<u32>>>) -> std::sync::MutexGuard<'_, Option<u32>> {
    pid.lock().unwrap_or_else(|p| p.into_inner())
}

impl App {
    /// Block on a future using the core runtime (for small sync calls).
    pub fn block_on<F: std::future::Future>(&self, fut: F) -> F::Output {
        self.runtime.handle().block_on(fut)
    }
}
