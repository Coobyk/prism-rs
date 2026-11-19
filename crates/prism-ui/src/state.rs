//! Application state shared across the UI.

use prism_core::horizon::HorizonLauncher;
use prism_core::launch::LaunchSettings;
use prism_core::meta::VersionManifest;
use prism_core::net::CoreRuntime;
use prism_core::{AccountList, InstanceKind, InstanceList, Paths};
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
    pub news: Vec<prism_core::news::NewsEntry>,
    pub news_handle: Option<tokio::task::JoinHandle<Vec<prism_core::news::NewsEntry>>>,
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
            news: Vec::new(),
            news_handle: None,
            toast: None,
        }
    }

    pub fn start_news_fetch(&mut self) {
        if self.news_handle.is_some() {
            return;
        }
        let downloader = self.runtime.downloader.clone();
        self.news_handle = Some(self.runtime.spawn(async move {
            prism_core::news::fetch_news(&downloader).await
        }));
    }

    pub fn poll_news(&mut self) {
        let finished = self.news_handle.as_ref().map(|h| h.is_finished()).unwrap_or(false);
        if !finished {
            return;
        }
        let handle = self.news_handle.take().unwrap();
        match self.block_on(handle) {
            Ok(news) => self.news = news,
            Err(e) => tracing::error!("news join failed: {e}"),
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
    pub fn start_launch(&mut self, id: &str, cx: &mut gpui_kit::Context<Self>) {
        self.start_launch_with(id, None, cx);
    }

    pub fn start_launch_with(
        &mut self,
        id: &str,
        quick_play: Option<prism_core::launch::QuickPlayTarget>,
        cx: &mut gpui_kit::Context<Self>,
    ) {
        if self.is_running(id) {
            return;
        }
        let Some(inst) = self.instances.get(id).cloned() else {
            return;
        };
        let running = RunningInstance::new();
        self.running.insert(id.to_string(), running.clone());

        if let InstanceKind::Horizon { id, home, token } = inst.kind {
            let config = self.paths.global_config();
            log_lock(&running.log).push(format!("Launching Horizon instance {:?}", inst.name));
            let (tx, rx) = tokio::sync::oneshot::channel();
            let log = running.log.clone();
            self.runtime.handle().spawn_blocking(move || {
                let settings = prism_core::settings::Settings::load(&config);
                let launcher = HorizonLauncher::from_settings(&settings);
                let result = launcher.play(&id, token).map(|(child, current_home)| {
                    if home != current_home {
                        let _ = tx.send(current_home);
                    }
                    child
                });
                monitor_launch(running, "Horizon", result);
            });
            cx.spawn(async move |this, cx| {
                if let Ok(current_home) = rx.await {
                    let _ = this.update(cx, |app, cx| {
                        let Some(instance) = app.instances.get_mut(&inst.id) else { return };
                        let InstanceKind::Horizon { token: current_token, .. } = &instance.kind else { return };
                        if *current_token != token {
                            return;
                        }
                        if let Err(error) = instance.save_horizon_home(current_home) {
                            log_lock(&log).push(format!("Could not save the moved Horizon game folder: {error}"));
                        }
                        cx.notify();
                    });
                }
            }).detach();
            return;
        }

        let mut settings = LaunchSettings::for_instance(&inst, &self.paths);
        settings.quick_play = quick_play;
        let accounts_path = self.paths.accounts_file();
        let mut accounts = AccountList::load(&accounts_path);
        let (accounts_tx, accounts_rx) = tokio::sync::oneshot::channel();
        cx.spawn(async move |this, cx| {
            if let Ok(accounts) = accounts_rx.await {
                let _ = this.update(cx, |app, cx| {
                    app.accounts = accounts;
                    cx.notify();
                });
            }
        }).detach();
        let launcher = prism_core::launch::Launcher::new(self.paths.clone(), self.runtime.downloader.clone());
        let downloader = self.runtime.downloader.clone();
        let log = running.log.clone();
        let state = running.state.clone();

        log_lock(&log).push(format!(
            "Launching instance \"{}\" (Minecraft {})",
            inst.name, settings.mc_version
        ));

        if let Some(target) = &settings.quick_play {
            let destination = match target {
                prism_core::launch::QuickPlayTarget::Singleplayer { world } => {
                    format!("world {world:?}")
                }
                _ => format!("server {}", target.address()),
            };
            log_lock(&log).push(format!("Quick Play: joining {destination}"));
        }
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
            let mut profile = prism_core::profile::LaunchProfile::from_mojang(&version_data);

            // 1b. Loader component overlay.
            let loader = prism_core::instance::loader_of(&inst);
            match &loader {
                prism_core::loaders::Loader::None => {}
                prism_core::loaders::Loader::Forge { .. } | prism_core::loaders::Loader::NeoForge { .. } => {
                    log_lock(&log).push(format!("Preparing loader: {loader:?}"));
                    // The installer expects a vanilla layout: game_dir/versions/<mc>/<mc>.jar.
                    let mc_dir = settings.game_dir.clone();
                    let versions_dir = mc_dir.join("versions").join(&settings.mc_version);
                    if let Err(e) = tokio::fs::create_dir_all(&versions_dir).await {
                        log_lock(&log).push(format!("Fatal: {e}"));
                        state_lock(&state, |s| *s = InstanceRunState::Crashed(e.to_string()));
                        return;
                    }
                    let client_jar_src = launcher.client_jar_path(&settings.mc_version);
                    let client_jar_dst = versions_dir.join(format!("{}.jar", settings.mc_version));
                    if !client_jar_dst.exists() {
                        if let Err(e) = tokio::fs::copy(&client_jar_src, &client_jar_dst).await {
                            log_lock(&log).push(format!("Fatal: vanilla jar copy: {e}"));
                            state_lock(&state, |s| *s = InstanceRunState::Crashed(e.to_string()));
                            return;
                        }
                    }
                    match prism_core::loaders::ensure_forge_installed(
                        &settings.java_path,
                        &loader,
                        &settings.mc_version,
                        &mc_dir,
                        &downloader,
                    )
                    .await
                    {
                        Ok(lvd) => profile.apply_loader(&lvd),
                        Err(e) => {
                            log_lock(&log).push(format!("Fatal: forge install failed: {e}"));
                            state_lock(&state, |s| *s = InstanceRunState::Crashed(format!("forge: {e}")));
                            return;
                        }
                    }
                }
                _ => {
                    log_lock(&log).push(format!("Fetching loader profile: {loader:?}"));
                    match prism_core::loaders::fetch_loader_profile(downloader.client(), &loader, &settings.mc_version).await {
                        Ok(lvd) => profile.apply_loader(&lvd),
                        Err(e) => {
                            log_lock(&log).push(format!("Fatal: loader profile fetch failed: {e}"));
                            state_lock(&state, |s| *s = InstanceRunState::Crashed(format!("loader: {e}")));
                            return;
                        }
                    }
                }
            }
            log_lock(&log).push(format!(
                "Resolved profile: mainClass={}, libraries={}, natives={}",
                profile.main_class,
                profile.libraries.len(),
                profile.natives.len()
            ));

            // 2. Files.
            if matches!(settings.quick_play, Some(prism_core::launch::QuickPlayTarget::Singleplayer { .. }))
                && !profile.supports_quick_play_singleplayer()
            {
                log_lock(&log).push(format!("Minecraft {} does not support Quick Play singleplayer; launching to the title screen", settings.mc_version));
            }
            if let Err(e) = launcher.prepare_files(&profile, &settings).await {
                log_lock(&log).push(format!("Fatal: {e}"));
                state_lock(&state, |s| *s = InstanceRunState::Crashed(e.to_string()));
                return;
            }
            log_lock(&log).push("Downloads complete".to_string());

            // 3. Command + spawn.
            let session = match accounts.session_for_launch(&accounts_path).await {
                Ok(session) => session,
                Err(error) => {
                    log_lock(&log).push(format!("Fatal: {error}"));
                    state_lock(&state, |s| *s = InstanceRunState::Crashed(error.to_string()));
                    return;
                }
            };
            let _ = accounts_tx.send(accounts);
            let prepared = launcher.build_command(&settings, session.as_ref(), &profile);
            log_lock(&log).push(format!("Command: {}", prepared.command.join(" ")));
            monitor_launch(running, "Minecraft", prepared.spawn());
        });
    }

    pub fn kill_instance(&mut self, id: &str) {
        if !self.instances.get(id).is_some_and(|inst| !inst.is_horizon()) {
            return;
        }
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

fn monitor_launch(
    running: RunningInstance,
    game: &str,
    result: Result<std::process::Child, impl std::fmt::Display>,
) {
    let mut child = match result {
        Ok(child) => child,
        Err(e) => {
            let message = format!("Could not launch {game}: {e}");
            log_lock(&running.log).push(message.clone());
            state_lock(&running.state, |state| *state = InstanceRunState::Crashed(message));
            return;
        }
    };
    *pid_lock(&running.pid) = Some(child.id());
    state_lock(&running.state, |state| *state = InstanceRunState::Running);
    log_lock(&running.log).push(format!("{game} process ID: {}", child.id()));

    if let Some(pipe) = child.stdout.take() {
        read_output(pipe, running.log.clone());
    }
    if let Some(pipe) = child.stderr.take() {
        read_output(pipe, running.log.clone());
    }
    std::thread::spawn(move || {
        let status = child.wait();
        *pid_lock(&running.pid) = None;
        match status {
            Ok(code) if code.success() => {
                log_lock(&running.log).push("Game exited normally".to_string());
                state_lock(&running.state, |state| *state = InstanceRunState::Finished);
            }
            Ok(code) => {
                let message = code.to_string();
                log_lock(&running.log).push(format!("Game exited with {message}"));
                state_lock(&running.state, |state| *state = InstanceRunState::Crashed(message));
            }
            Err(e) => {
                log_lock(&running.log).push(format!("Process error: {e}"));
                state_lock(&running.state, |state| *state = InstanceRunState::Crashed(e.to_string()));
            }
        }
    });
}

fn read_output(
    pipe: impl std::io::Read + Send + 'static,
    log: Arc<Mutex<Vec<String>>>,
) {
    use std::io::BufRead;
    std::thread::spawn(move || {
        for line in std::io::BufReader::new(pipe).lines().map_while(Result::ok) {
            log_lock(&log).push(line);
        }
    });
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
