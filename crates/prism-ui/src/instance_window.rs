//! Instance window: per-instance pages (Log, Version, Mods, Resource Packs,
//! Shader Packs, Data Packs, Notes, Settings), mirroring PrismLauncher's InstanceWindow
//! + instance pages.

use crate::state::{App, InstanceRunState};
use gpui_kit::base::Disableable;
use gpui_kit::component::button::Button;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::*;
use prism_core::loaders::Loader;
use prism_core::modplatform::flame::{FlameApi, FlameSearchHit, API_KEY_ENV};
use prism_core::modplatform::modrinth::{ModrinthApi, SearchResult};
use prism_core::modplatform::updates::{self, UpdateReport};
use prism_core::modplatform::{LocalResource, ResourceType};
use prism_core::servers::{ResourcePackPolicy, ServerEntry, ServerList, ServerListSnapshot};
use prism_core::{Instance, InstanceKind};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

#[path = "datapacks_page.rs"]
mod datapacks_page;
use datapacks_page::DataPacksPage;
#[path = "worlds_page.rs"]
mod worlds_page;
use worlds_page::WorldsPage;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Page {
    Log,
    Version,
    Mods,
    ModUpdates,
    ResourcePacks,
    ShaderPacks,
    DataPacks,
    BrowseMods,
    BrowseCurseForge,
    BrowseResourcePacks,
    BrowseShaderPacks,
    BrowseDataPacks,
    Notes,
    Settings,
    Worlds,
    Servers,
    Screenshots,
}

impl Page {
    pub fn label(&self) -> &'static str {
        match self {
            Page::Log => "Minecraft Log",
            Page::Version => "Version",
            Page::Mods => "Mods",
            Page::ModUpdates => "Mod Updates",
            Page::ResourcePacks => "Resource Packs",
            Page::ShaderPacks => "Shader Packs",
            Page::DataPacks => "Data Packs",
            Page::BrowseMods => "Browse Mods (Modrinth)",
            Page::BrowseCurseForge => "Browse Mods (CurseForge)",
            Page::BrowseResourcePacks => "Browse Resource Packs",
            Page::BrowseShaderPacks => "Browse Shader Packs",
            Page::BrowseDataPacks => "Browse Data Packs",
            Page::Notes => "Notes",
            Page::Settings => "Settings",
            Page::Worlds => "Worlds",
            Page::Servers => "Servers",
            Page::Screenshots => "Screenshots",
        }
    }

    pub fn all() -> Vec<Page> {
        vec![
            Page::Log,
            Page::Version,
            Page::Mods,
            Page::BrowseMods,
            Page::ResourcePacks,
            Page::ShaderPacks,
            Page::DataPacks,
            Page::Worlds,
            Page::Servers,
            Page::Screenshots,
            Page::Notes,
            Page::Settings,
        ]
    }

    fn available(self, horizon: bool) -> bool {
        !horizon || matches!(self, Self::Log | Self::Worlds | Self::Screenshots | Self::Notes)
    }

    fn browser(self) -> Option<Browser> {
        match self {
            Self::BrowseMods => Some(Browser::Modrinth(ResourceType::Mod)),
            Self::BrowseCurseForge => Some(Browser::CurseForge),
            Self::BrowseResourcePacks => Some(Browser::Modrinth(ResourceType::ResourcePack)),
            Self::BrowseShaderPacks => Some(Browser::Modrinth(ResourceType::ShaderPack)),
            Self::BrowseDataPacks => Some(Browser::Modrinth(ResourceType::DataPack)),
            _ => None,
        }
    }

    fn sidebar_page(self) -> Self {
        match self {
            Self::ModUpdates | Self::BrowseCurseForge => Self::Mods,
            Self::BrowseResourcePacks => Self::ResourcePacks,
            Self::BrowseShaderPacks => Self::ShaderPacks,
            Self::BrowseDataPacks => Self::DataPacks,
            _ => self,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Browser, Page, ResourceType};

    #[test]
    fn resource_pack_editor_is_only_available_for_java() {
        assert!(Page::ResourcePacks.available(false));
        assert!(!Page::ResourcePacks.available(true));
        assert!(Page::Settings.available(false));
        assert!(!Page::Settings.available(true));
    }

    #[test]
    fn pack_browsers_belong_to_their_java_resource_pages() {
        for (browser, installed, resource_type) in [
            (Page::BrowseResourcePacks, Page::ResourcePacks, ResourceType::ResourcePack),
            (Page::BrowseShaderPacks, Page::ShaderPacks, ResourceType::ShaderPack),
            (Page::BrowseDataPacks, Page::DataPacks, ResourceType::DataPack),
        ] {
            assert!(browser.available(false));
            assert!(!browser.available(true));
            assert!(installed.available(false));
            assert!(!installed.available(true));
            assert!(!Page::all().contains(&browser));
            assert_eq!(browser.sidebar_page(), installed);
            assert_eq!(browser.browser(), Some(Browser::Modrinth(resource_type)));
        }
    }

    #[test]
    fn curseforge_is_nested_under_java_mods() {
        let page = Page::BrowseCurseForge;
        assert!(page.available(false));
        assert!(!page.available(true));
        assert!(!Page::all().contains(&page));
        assert_eq!(page.sidebar_page(), Page::Mods);
        assert_eq!(page.browser(), Some(Browser::CurseForge));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Browser {
    Modrinth(ResourceType),
    CurseForge,
}

impl Browser {
    fn prerequisite(self, loader: &Loader) -> Option<String> {
        if self != Self::CurseForge {
            return None;
        }
        if *loader == Loader::None {
            return Some("Install a mod loader before browsing CurseForge mods.".into());
        }
        (!FlameApi::is_configured()).then(|| format!("Set {API_KEY_ENV} before opening the launcher to browse CurseForge."))
    }
}

#[derive(Clone)]
enum BrowserProject {
    Modrinth(SearchResult),
    CurseForge(FlameSearchHit),
}

impl BrowserProject {
    fn details(&self) -> (String, &str, &str, u64) {
        match self {
            Self::Modrinth(project) => (project.project_id.clone(), &project.title, &project.description, project.downloads),
            Self::CurseForge(project) => (project.id.to_string(), &project.name, &project.summary, project.downloads),
        }
    }
}

#[derive(Clone, PartialEq)]
struct BrowserTarget {
    game_root: std::path::PathBuf,
    mc_version: String,
    loader: Loader,
}

impl BrowserTarget {
    fn new(instance: &Instance, app: &App) -> Self {
        Self {
            game_root: instance.game_root(),
            mc_version: prism_core::launch::LaunchSettings::for_instance(instance, &app.paths).mc_version,
            loader: prism_core::instance::loader_of(instance),
        }
    }
}

struct BrowserState {
    input: Entity<InputState>,
    target: Option<BrowserTarget>,
    results: Vec<BrowserProject>,
    searching: bool,
    search_status: String,
    installing: HashSet<String>,
    install_status: String,
}

#[derive(Default)]
struct ModUpdates {
    target: Option<(std::path::PathBuf, String, prism_core::loaders::Loader)>,
    report: UpdateReport,
    busy: bool,
    status: String,
}

pub struct InstanceWindow {
    pub app: gpui::Entity<App>,
    pub instance_id: String,
    pub page: Page,
    browsers: HashMap<Browser, BrowserState>,
    mod_updates: ModUpdates,
    // Loader install state.
    loader_options: Arc<std::sync::Mutex<Vec<String>>>,
    loader_handle: Option<tokio::task::JoinHandle<Result<Vec<String>, String>>>,
    servers: Option<ServerList>,
    servers_snapshot: Option<ServerListSnapshot>,
    server_selected: Option<usize>,
    server_inputs: Option<(gpui::Entity<InputState>, gpui::Entity<InputState>)>,
    server_status: String,
    servers_was_running: bool,
    datapacks: DataPacksPage,
    worlds: WorldsPage,
    pub(crate) settings_editor: Option<crate::instance_settings_page::SettingsEditor>,
}

impl InstanceWindow {
    pub fn new(app: gpui::Entity<App>, instance_id: String) -> Self {
        InstanceWindow {
            app,
            instance_id,
            page: Page::Log,
            browsers: HashMap::new(),
            mod_updates: ModUpdates::default(),
            loader_options: Arc::new(std::sync::Mutex::new(Vec::new())),
            loader_handle: None,
            servers: None,
            servers_snapshot: None,
            server_selected: None,
            server_inputs: None,
            server_status: String::new(),
            servers_was_running: false,
            datapacks: DataPacksPage::default(),
            worlds: WorldsPage::default(),
            settings_editor: None,
        }
    }

    fn fetch_loader_versions(&mut self, kind: &'static str, cx: &mut Context<Self>) {
        let app = self.app.read(cx);
        let runtime = app.runtime.clone();
        let client = app.runtime.downloader.client().clone();
        let Some(inst) = app.instances.get(&self.instance_id).cloned() else {
            return;
        };
        drop(app);
        let mc_version =
            prism_core::launch::LaunchSettings::for_instance(&inst, &self.app.read(cx).paths).mc_version;
        let loader = match kind {
            "fabric" => prism_core::loaders::Loader::Fabric { version: String::new() },
            _ => prism_core::loaders::Loader::Quilt { version: String::new() },
        };
        let options = self.loader_options.clone();
        *options.lock().unwrap_or_else(|p| p.into_inner()) = Vec::new();
        self.loader_handle = Some(runtime.spawn(async move {
            prism_core::loaders::loader_versions(&client, &loader, &mc_version)
                .await
                .map(|v| v.into_iter().take(12).collect())
        }));
        cx.notify();
    }

    fn poll_loader_versions(&mut self, cx: &mut Context<Self>) {
        if let Some(handle) = &self.loader_handle {
            if handle.is_finished() {
                let handle = self.loader_handle.take().unwrap();
                let options = self.loader_options.clone();
                let result = self.app.read(cx).block_on(handle);
                let list = match result {
                    Ok(Ok(v)) => v,
                    Ok(Err(e)) => {
                        tracing::warn!("loader versions failed: {e}");
                        Vec::new()
                    }
                    Err(e) => {
                        tracing::warn!("loader versions failed: {e}");
                        Vec::new()
                    }
                };
                *options.lock().unwrap_or_else(|p| p.into_inner()) = list;
                cx.notify();
            }
        }
    }

    fn set_instance_loader(&mut self, kind: &'static str, version: String, cx: &mut Context<Self>) {
        let loader = if version.is_empty() {
            prism_core::loaders::Loader::None
        } else {
            match kind {
                "fabric" => prism_core::loaders::Loader::Fabric { version },
                _ => prism_core::loaders::Loader::Quilt { version },
            }
        };
        let instance_id = self.instance_id.clone();
        self.app.update(cx, |app, _cx| {
            if let Some(inst) = app.instances.get(&instance_id).cloned() {
                let _ = prism_core::instance::set_loader(&inst, &loader);
            }
        });
        *self.loader_options.lock().unwrap_or_else(|p| p.into_inner()) = Vec::new();
        self.app.update(cx, |app, _cx| app.reload_instances());
        cx.notify();
    }

    fn start_search(&mut self, provider: Browser, cx: &mut Context<Self>) {
        let app = self.app.read(cx);
        let Some(inst) = app.instances.get(&self.instance_id).filter(|inst| !inst.is_horizon()) else {
            return;
        };
        let target = BrowserTarget::new(inst, app);
        if provider.prerequisite(&target.loader).is_some() {
            return;
        }
        let runtime = app.runtime.clone();
        let client = runtime.downloader.client().clone();
        let Some(browser) = self.browsers.get_mut(&provider) else {
            return;
        };
        if browser.searching {
            return;
        }
        let query = browser.input.read(cx).value().trim().to_string();
        browser.searching = true;
        browser.target = Some(target.clone());
        browser.results.clear();
        browser.search_status = "Searching…".into();
        let task = runtime.spawn(async move {
            let results = match provider {
                Browser::Modrinth(resource_type) => ModrinthApi::with_client(client)
                    .search(&query, resource_type, Some(&target.mc_version), 25)
                    .await?
                    .hits
                    .into_iter()
                    .map(BrowserProject::Modrinth)
                    .collect::<Vec<_>>(),
                Browser::CurseForge => FlameApi::new()
                    .search_mods(&query, &target.mc_version, &target.loader)
                    .await?
                    .into_iter()
                    .map(BrowserProject::CurseForge)
                    .collect(),
            };
            Ok::<_, prism_core::modplatform::PlatformError>(results)
        });
        cx.spawn(async move |this, cx| {
            let result = task.await.map_err(|e| e.to_string()).and_then(|r| r.map_err(|e| e.to_string()));
            let _ = this.update(cx, |this, cx| {
                if let Some(browser) = this.browsers.get_mut(&provider) {
                    browser.searching = false;
                    browser.search_status = match result {
                        Ok(results) if results.is_empty() => "No compatible projects found.".into(),
                        Ok(results) => {
                            browser.results = results;
                            String::new()
                        }
                        Err(error) => format!("Search failed: {error}"),
                    };
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    fn install_project(&mut self, provider: Browser, project: BrowserProject, cx: &mut Context<Self>) {
        let app = self.app.read(cx);
        let Some(inst) = app.instances.get(&self.instance_id).filter(|inst| !inst.is_horizon()) else {
            return;
        };
        let target = BrowserTarget::new(inst, app);
        let runtime = app.runtime.clone();
        let downloader = runtime.downloader.clone();
        let Some(browser) = self.browsers.get_mut(&provider) else {
            return;
        };
        if browser.target.as_ref() != Some(&target) {
            browser.install_status = "This instance's version or loader changed. Search again before installing.".into();
            cx.notify();
            return;
        }
        let (project_id, title, _, _) = project.details();
        if browser.installing.contains(&project_id) {
            return;
        }
        let lock = match crate::instance_lock::acquire(&app.paths, inst) {
            Ok(lock) => lock,
            Err(error) => {
                browser.install_status = format!("Could not install {title}: {error}");
                cx.notify();
                return;
            }
        };
        browser.installing.insert(project_id.clone());
        browser.install_status.clear();
        let title = title.to_owned();
        let task = runtime.spawn(async move {
            let _lock = lock;
            match (provider, project) {
                (Browser::Modrinth(resource_type), BrowserProject::Modrinth(project)) => ModrinthApi::with_client(downloader.client().clone())
                    .install_compatible(&downloader, &target.game_root, &project, resource_type, &target.mc_version, &target.loader)
                    .await,
                (Browser::CurseForge, BrowserProject::CurseForge(project)) => FlameApi::new()
                    .install_mod(&downloader, &target.game_root, project.id, &target.mc_version, &target.loader)
                    .await,
                _ => unreachable!("browser project belongs to another provider"),
            }
        });
        cx.spawn(async move |this, cx| {
            let result = task.await.map_err(|e| e.to_string()).and_then(|r| r.map_err(|e| e.to_string()));
            let _ = this.update(cx, |this, cx| {
                if let Some(browser) = this.browsers.get_mut(&provider) {
                    browser.installing.remove(&project_id);
                    browser.install_status = match result {
                        Ok(_) => format!("Installed {title}"),
                        Err(error) => format!("Could not install {title}: {error}"),
                    };
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    fn check_mod_updates(&mut self, cx: &mut Context<Self>) {
        if self.mod_updates.busy {
            return;
        }
        let app = self.app.read(cx);
        let Some(instance) = app.instances.get(&self.instance_id).filter(|inst| !inst.is_horizon()) else {
            return;
        };
        let root = instance.game_root();
        let mc = prism_core::launch::LaunchSettings::for_instance(instance, &app.paths).mc_version;
        let loader = prism_core::instance::loader_of(instance);
        let runtime = app.runtime.clone();
        let downloader = runtime.downloader.clone();
        self.page = Page::ModUpdates;
        self.mod_updates = ModUpdates {
            target: Some((root.clone(), mc.clone(), loader.clone())),
            busy: true,
            status: "Checking installed mods…".into(),
            ..ModUpdates::default()
        };
        let task = runtime.spawn(async move { updates::check(&root, &mc, &loader, &downloader).await });
        cx.spawn(async move |this, cx| {
            let result = task.await.map_err(|e| e.to_string()).and_then(|r| r.map_err(|e| e.to_string()));
            let _ = this.update(cx, |this, cx| {
                let state = &mut this.mod_updates;
                state.busy = false;
                state.status = match result {
                    Ok(report) => {
                        let count = report.updates.len();
                        let status = if count > 0 {
                            format!("{count} compatible update{} available.", if count == 1 { "" } else { "s" })
                        } else if report.warnings.is_empty() && report.unmatched == 0 {
                            "No compatible updates found.".into()
                        } else {
                            "No updates found for mods that could be checked.".into()
                        };
                        state.report = report;
                        status
                    }
                    Err(error) => format!("Could not check for updates: {error}"),
                };
                cx.notify();
            });
        }).detach();
        cx.notify();
    }

    fn apply_mod_update(&mut self, update: updates::ModUpdate, cx: &mut Context<Self>) {
        if self.mod_updates.busy {
            return;
        }
        let app = self.app.read(cx);
        let Some(instance) = app.instances.get(&self.instance_id).filter(|inst| !inst.is_horizon()) else {
            return;
        };
        let target = (
            instance.game_root(),
            prism_core::launch::LaunchSettings::for_instance(instance, &app.paths).mc_version,
            prism_core::instance::loader_of(instance),
        );
        if self.mod_updates.target.as_ref() != Some(&target) {
            self.mod_updates.status = "Instance settings changed. Check for updates again.".into();
            self.mod_updates.report.updates.clear();
            cx.notify();
            return;
        }
        let lock = match crate::instance_lock::acquire(&app.paths, instance) {
            Ok(lock) => lock,
            Err(error) => {
                self.mod_updates.status = format!("Could not update {}: {error}", update.title);
                cx.notify();
                return;
            }
        };
        let runtime = app.runtime.clone();
        let downloader = runtime.downloader.clone();
        let filename = update.installed_filename().to_owned();
        let title = update.title.clone();
        self.mod_updates.busy = true;
        self.mod_updates.status = format!("Updating {title}…");
        let task = runtime.spawn(async move {
            let _lock = lock;
            update.apply(&downloader).await
        });
        cx.spawn(async move |this, cx| {
            let result = task.await.map_err(|e| e.to_string()).and_then(|r| r.map_err(|e| e.to_string()));
            let _ = this.update(cx, |this, cx| {
                let state = &mut this.mod_updates;
                state.busy = false;
                state.status = match result {
                    Ok(()) => {
                        state.report.updates.retain(|row| row.installed_filename() != filename);
                        format!("Updated {title}.")
                    }
                    Err(error) => format!("Could not update {title}: {error}"),
                };
                cx.notify();
            });
        }).detach();
        cx.notify();
    }

    fn instance(&self, cx: &Context<Self>) -> Instance {
        self.app
            .read(cx)
            .instances
            .get(&self.instance_id)
            .cloned()
            .unwrap_or(Instance {
                id: self.instance_id.clone(),
                kind: InstanceKind::Java,
                root: Default::default(),
                name: self.instance_id.clone(),
                icon_key: "default".into(),
                notes: String::new(),
                total_time_played: 0,
                last_launch: 0,
                group: None,
            })
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let inst = self.instance(cx);
        let running = self.app.read(cx).is_running(&self.instance_id);
        let mut col = div().w(px(170.)).flex_shrink_0().flex().flex_col().bg(rgb(0x2b2b2b)).border_r_1().border_color(rgb(0x1c1c1c));
        col = col
            .child(
                div()
                    .px_3()
                    .py_2()
                    .text_size(px(14.))
                    .font_weight(FontWeight::BOLD)
                    .text_ellipsis()
                    .child(inst.name.clone()),
            )
            .child(div().h(px(1.)).bg(rgb(0x1c1c1c)));

        for page in Page::all().into_iter().filter(|page| page.available(inst.is_horizon())) {
            let active = self.page.sidebar_page() == page;
            let label = if page == Page::Log && inst.is_horizon() { "Horizon Log" } else { page.label() };
            col = col.child(
                div()
                    .id(SharedString::from(format!("page-{:?}", page)))
                    .px_3()
                    .py_2()
                    .text_size(px(13.))
                    .bg(if active { rgb(0x3d5a80) } else { rgba(0x00000000) })
                    .hover(|s| s.bg(rgb(0x333333)))
                    .on_click(cx.listener(move |this, _ev, _window, cx| {
                        if this.page != page {
                            this.worlds.on_navigation();
                        }
                        this.page = page;
                        cx.notify();
                    }))
                    .child(label),
            );
        }

        col.child(div().flex_1()).child(
            div()
                .px_3()
                .py_2()
                .child(
                    Button::new("iw-launch")
                        .label(if running { "Running…" } else { "Launch" })
                        .disabled(running)
                        .on_click(cx.listener(|this, _ev, _window, cx| {
                            let id = this.instance_id.clone();
                            this.app.update(cx, |app, cx| app.start_launch(&id, cx));
                            cx.notify();
                        })),
                ),
        )
    }

    fn render_page(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        match self.page {
            Page::Log => self.render_log_page(cx),
            Page::Version => self.render_version_page(cx),
            Page::Mods => self.render_resources_page(cx, ResourceType::Mod),
            Page::ModUpdates => self.render_mod_updates(cx),
            Page::ResourcePacks => self.render_resources_page(cx, ResourceType::ResourcePack),
            Page::ShaderPacks => self.render_resources_page(cx, ResourceType::ShaderPack),
            Page::DataPacks => self.render_datapacks(cx),
            Page::Notes => self.render_notes_page(cx),
            Page::Settings => self.render_settings_page(cx),
            Page::Worlds => self.render_worlds_page(cx),
            Page::Servers => self.render_servers_page(cx),
            Page::Screenshots => self.render_screenshots_page(cx),
            Page::BrowseMods | Page::BrowseCurseForge | Page::BrowseResourcePacks | Page::BrowseShaderPacks | Page::BrowseDataPacks => self
                .render_mod_browser(cx, self.page.browser().expect("browser page")),
        }
    }

    fn page_header(&self, cx: &Context<Self>, subtitle: String) -> Div {
        let inst = self.instance(cx);
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap_2()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(rgb(0x1c1c1c))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .child(div().text_ellipsis().text_size(px(15.)).font_weight(FontWeight::BOLD).child(inst.name))
                    .child(div().text_ellipsis().text_size(px(11.)).text_color(rgb(0x888888)).child(subtitle)),
            )
    }

    fn render_log_page(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let mut lines: Vec<String> = Vec::new();
        if let Some(running) = self.app.read(cx).running.get(&self.instance_id) {
            if let Ok(log) = running.log.lock() {
                lines = log.clone();
            }
        } else {
            lines.push("No log: instance is not running.".to_string());
        }
        let tail: Vec<String> = lines.iter().rev().take(1000).rev().cloned().collect();

        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .min_h_0()
            .child(self.page_header(cx, "Console output".to_string()).into_any_element())
            .child(
                div()
                    .id("log-scroll")
                    .flex_1()
                    .overflow_y_scroll()
                    .p_2()
                    .bg(rgb(0x1a1a1a))
                    .font_family("monospace")
                    .text_size(px(11.))
                    .text_color(rgb(0xcccccc))
                    .children(tail.into_iter().map(|l| div().w_full().whitespace_normal().child(l).into_any_element())),
            )
            .into_any_element()
    }

    fn render_version_page(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        self.poll_loader_versions(cx);
        let inst = self.instance(cx);
        let loader = prism_core::instance::loader_of(&inst);
        let mc_version = prism_core::launch::LaunchSettings::for_instance(&inst, &self.app.read(cx).paths)
            .mc_version;

        let mut rows: Vec<(String, String)> = vec![("net.minecraft".to_string(), mc_version.clone())];
        match &loader {
            prism_core::loaders::Loader::None => {}
            l => rows.push((l.uid().to_string(), l.version().unwrap_or("?").to_string())),
        }

        let mut list = div().flex().flex_col().gap_1().p_3();
        for (uid, version) in &rows {
            list = list.child(
                div()
                    .flex()
                    .justify_between()
                    .px_3()
                    .py_2()
                    .bg(rgb(0x2b2b2b))
                    .rounded_md()
                    .child(div().child(uid.clone()))
                    .child(div().text_color(rgb(0x8fb4d9)).child(version.clone())),
            );
        }

        // Loader install / remove.
        let loader_options = self.loader_options.lock().unwrap_or_else(|p| p.into_inner()).clone();
        list = list.child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .p_3()
                .pt_4()
                .child(
                    Button::new("install-fabric")
                        .label("Install Fabric")
                        .on_click(cx.listener(|this, _ev, _window, cx| {
                            this.fetch_loader_versions("fabric", cx);
                        })),
                )
                .child(
                    Button::new("install-quilt")
                        .label("Install Quilt")
                        .on_click(cx.listener(|this, _ev, _window, cx| {
                            this.fetch_loader_versions("quilt", cx);
                        })),
                )
                .children((!matches!(loader, prism_core::loaders::Loader::None)).then(|| {
                    Button::new("remove-loader")
                        .label("Remove Loader")
                        .on_click(cx.listener(|this, _ev, _window, cx| {
                            this.set_instance_loader("fabric", String::new(), cx);
                        }))
                })),
        );

        for version in &loader_options {
            let v = version.clone();
            list = list.child(
                div()
                    .id(SharedString::from(format!("loader-{version}")))
                    .flex()
                    .justify_between()
                    .px_3()
                    .py_2()
                    .bg(rgb(0x243244))
                    .rounded_md()
                    .hover(|s| s.bg(rgb(0x2c3d52)))
                    .on_click(cx.listener(move |this, _ev, _window, cx| {
                        this.set_instance_loader("fabric", v.clone(), cx);
                    }))
                    .child(format!("fabric-loader {version}"))
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(0x8fb4d9))
                            .child("click to install"),
                    ),
            );
        }

        div()
            .flex_1()
            .flex()
            .flex_col()
            .min_h_0()
            .child(self.page_header(cx, "Installed components".to_string()).into_any_element())
            .child(div().id("version-scroll").flex_1().overflow_y_scroll().child(list))
            .into_any_element()
    }

    fn render_resources_page(&self, cx: &mut Context<Self>, rtype: ResourceType) -> gpui::AnyElement {
        let inst = self.instance(cx);
        let folder = inst.game_root().join(rtype.folder());
        let resources: Vec<LocalResource> = prism_core::resources::scan_folder(&folder, rtype);

        let mut list = div().flex().flex_col().gap_1().p_3();
        if resources.is_empty() {
            list = list.child(
                div()
                    .text_color(rgb(0x777777))
                    .child(format!("No files in {}", folder.display())),
            );
        }
        for r in &resources {
            let path = r.path.clone();
            let enabled = !r.file_name.ends_with(".disabled");
            let display_title = r.title.clone().unwrap_or_else(|| r.file_name.clone());
            let version = r.version.clone().unwrap_or_default();
            let loader = r.loader.clone().unwrap_or_default();
            list = list.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .bg(rgb(0x2b2b2b))
                    .rounded_md()
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_ellipsis()
                                    .child(if enabled { display_title } else { format!("{display_title} (disabled)") }),
                            )
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(rgb(0x888888))
                                    .child(format!("{} {} {}", r.file_name, version, loader)),
                            ),
                    )
                    .child(
                        Button::new(SharedString::from(format!("toggle-{}", r.file_name)))
                            .label(if enabled { "Disable" } else { "Enable" })
                            .on_click(move |_, _, _| {
                                let _ = prism_core::resources::toggle_enabled(&path);
                            }),
                    )
                    .child(
                        Button::new(SharedString::from(format!("del-{}", r.file_name)))
                            .label("Delete")
                            .on_click({
                                let p = r.path.clone();
                                move |_, _, _| {
                                    let _ = std::fs::remove_file(&p);
                                }
                            }),
                    ),
            );
        }

        let browser_page = match rtype {
            ResourceType::ResourcePack => Some(Page::BrowseResourcePacks),
            ResourceType::ShaderPack => Some(Page::BrowseShaderPacks),
            _ => None,
        };
        let mut header = self.page_header(cx, folder.display().to_string());
        if rtype == ResourceType::Mod {
            header = header.child(div().flex_shrink_0().flex().gap_2()
                .child(Button::new("browse-curseforge")
                    .label("Browse CurseForge")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.page = Page::BrowseCurseForge;
                        cx.notify();
                    })))
                .child(Button::new("check-mod-updates")
                    .label("Check for Updates")
                    .disabled(self.mod_updates.busy)
                    .on_click(cx.listener(|this, _, _, cx| this.check_mod_updates(cx)))),
            );
        }
        if let Some(page) = browser_page {
            header = header.child(
                div().flex_shrink_0().child(Button::new("browse-packs")
                    .label("Browse Modrinth")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.page = page;
                        cx.notify();
                    }))),
            );
        }
        div()
            .flex_1()
            .flex()
            .flex_col()
            .min_w_0()
            .min_h_0()
            .child(header)
            .child(
                div()
                    .id("resources-scroll")
                    .flex_1()
                    .overflow_y_scroll()
                    .child(list),
            )
            .into_any_element()
    }

    fn render_mod_browser(&self, cx: &mut Context<Self>, provider: Browser) -> gpui::AnyElement {
        let inst = self.instance(cx);
        let app = self.app.read(cx);
        let target = BrowserTarget::new(&inst, app);
        let running = app.is_running(&self.instance_id);
        let is_datapack = provider == Browser::Modrinth(ResourceType::DataPack);
        let pack_target = is_datapack.then(|| self.datapack_target(cx)).flatten();

        let Some(browser) = self.browsers.get(&provider) else {
            return div().into_any_element();
        };
        let mut subtitle = format!("{} · Minecraft {}", self.page.label(), target.mc_version);
        if let Some(scope) = &pack_target {
            subtitle.push_str(&format!(" · World: {}", scope.world.name()));
        }
        let mut header = self.page_header(cx, subtitle);
        let installed_page = self.page.sidebar_page();
        if installed_page != self.page {
            header = header.child(
                div().flex_shrink_0().child(Button::new("installed-packs")
                    .label("Back to installed")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.page = installed_page;
                        cx.notify();
                    }))),
            );
        }

        let mut panel = div()
            .flex_1()
            .flex()
            .flex_col()
            .min_w_0()
            .min_h_0()
            .child(header);
        if is_datapack && pack_target.is_none() {
            return panel.child(div().p_3().child("Select an existing world on the Data Packs page.")).into_any_element();
        }
        if let Some(message) = provider.prerequisite(&target.loader) {
            return panel.child(div().p_3().text_color(rgb(0x8fb4d9)).child(message)).into_any_element();
        }

        let search_row = div().flex().gap_2().p_2().items_center()
            .child(div().flex_1().min_w_0().child(Input::new(&browser.input)))
            .child(
                Button::new("mod-browser-search")
                    .label("Search")
                    .disabled(browser.searching)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.start_search(provider, cx);
                    })),
            );
        panel = panel.child(search_row);

        for status in [&browser.search_status, &browser.install_status] {
            if status.is_empty() {
                continue;
            }
            panel = panel.child(
                div()
                    .px_3()
                    .text_size(px(12.))
                    .text_ellipsis()
                    .text_color(rgb(0x8fb4d9))
                    .child(status.clone()),
            );
        }

        let mut list = div()
            .id("mod-browser-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_1()
            .p_2();
        for project in &browser.results {
            let (id, title, description, downloads) = project.details();
            let is_installing = browser.installing.contains(&id);
            let project = project.clone();
            let pack_target = pack_target.clone();
            list = list.child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .bg(rgb(0x2b2b2b))
                    .rounded_md()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(div().text_ellipsis().child(title.to_owned()))
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(rgb(0x888888))
                                    .text_ellipsis()
                                    .child(description.to_owned()),
                            ),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(10.))
                            .text_color(rgb(0x666666))
                            .child(format!("{downloads} downloads")),
                    )
                    .child(
                        div().flex_shrink_0().child(Button::new(SharedString::from(format!("install-{id}")))
                            .label(if is_installing { "Installing…" } else { "Install" })
                            .disabled(running || is_installing)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(target) = &pack_target {
                                    if let BrowserProject::Modrinth(project) = &project {
                                        this.install_datapack(target.clone(), project.clone(), cx);
                                    }
                                } else if !is_datapack {
                                    this.install_project(provider, project.clone(), cx);
                                }
                            }))),
                    ),
            );
        }
        panel = panel.child(list);
        panel.into_any_element()
    }

    fn render_mod_updates(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let updates = &self.mod_updates;
        let running = self.app.read(cx).is_running(&self.instance_id);
        let subtitle = updates.target.as_ref()
            .map(|(_, mc, _)| format!("Mod Updates · Minecraft {mc}"))
            .unwrap_or_else(|| "Mod Updates".into());
        let header = self.page_header(cx, subtitle)
            .child(div().flex_shrink_0().flex().gap_2()
                .child(Button::new("recheck-mod-updates")
                    .label("Check again")
                    .disabled(updates.busy)
                    .on_click(cx.listener(|this, _, _, cx| this.check_mod_updates(cx))))
                .child(Button::new("back-to-mods")
                    .label("Back to installed")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.page = Page::Mods;
                        cx.notify();
                    }))));
        let mut list = div().id("mod-updates-list").min_h_0().flex_1().overflow_y_scroll()
            .flex().flex_col().gap_2().p_3();
        for update in &updates.report.updates {
            let row = update.clone();
            list = list.child(
                div().flex().flex_shrink_0().items_center().gap_3().px_3().py_2()
                    .rounded_md().bg(rgb(0x2b2b2b))
                    .child(div().min_w_0().flex_1().flex().flex_col()
                        .child(div().text_ellipsis().child(update.title.clone()))
                        .child(div().text_ellipsis().text_size(px(11.)).text_color(rgb(0xaaaaaa))
                            .child(format!("{} → {} · {}", update.current_version, update.new_version, update.provider))))
                    .child(div().flex_shrink_0().child(Button::new(SharedString::from(format!("update-{}", update.installed_filename())))
                        .label("Update")
                        .disabled(updates.busy || running)
                        .on_click(cx.listener(move |this, _, _, cx| this.apply_mod_update(row.clone(), cx))))),
            );
        }
        if updates.report.locked > 0 || updates.report.unmatched > 0 {
            list = list.child(div().text_size(px(12.)).text_color(rgb(0xaaaaaa))
                .child(format!("Skipped: {} locked, {} without a provider match.", updates.report.locked, updates.report.unmatched)));
        }
        for warning in &updates.report.warnings {
            list = list.child(div().text_size(px(12.)).text_color(rgb(0xd8b17c)).child(warning.clone()));
        }
        div().min_w_0().min_h_0().flex_1().flex().flex_col()
            .child(header)
            .child(div().px_3().pt_3().text_size(px(13.)).text_color(rgb(0x8fb4d9)).child(updates.status.clone()))
            .child(list)
            .into_any_element()
    }

    fn render_notes_page(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let notes = self.instance(cx).notes;
        div()
            .flex_1()
            .flex()
            .flex_col()
            .min_h_0()
            .child(self.page_header(cx, "Free-form notes".to_string()).into_any_element())
            .child(
                div()
                    .id("notes-area")
                    .flex_1()
                    .m_3()
                    .p_3()
                    .bg(rgb(0x2b2b2b))
                    .rounded_md()
                    .text_size(px(13.))
                    .child(if notes.is_empty() { "No notes yet.".to_string() } else { notes }),
            )
            .into_any_element()
    }

    fn render_settings_page(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        match &self.settings_editor {
            Some(editor) => editor.render(cx),
            None => div().p_4().child("Could not load instance settings. Reopen this window to retry.").into_any_element(),
        }
    }

    fn servers_running(&self, cx: &Context<Self>) -> bool {
        matches!(
            self.app.read(cx).run_state(&self.instance_id),
            InstanceRunState::Running | InstanceRunState::Preparing
        )
    }

    fn save_servers(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.servers_running(cx) {
            return;
        }
        let Some(instance) = self.app.read(cx).instances.get(&self.instance_id) else {
            self.server_status = "Save failed: instance no longer exists".into();
            cx.notify();
            return;
        };
        if !Page::Servers.available(instance.is_horizon()) {
            return;
        }
        let game_root = instance.game_root();
        if let (Some(list), Some(snapshot)) = (&mut self.servers, &mut self.servers_snapshot) {
            match snapshot.save_or_reload(&game_root, list) {
                Ok(true) => {
                    let selected = (!list.entries.is_empty()).then_some(0);
                    self.select_server(selected, window, cx);
                    self.server_status = "servers.dat changed on disk; reloaded".into();
                }
                Ok(false) => self.server_status.clear(),
                Err(e) => self.server_status = format!("Save failed: {e}"),
            }
        }
        cx.notify();
    }

    fn select_server(&mut self, index: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        self.server_selected = index;
        self.server_inputs = None;
        if let Some(entry) = index
            .and_then(|i| self.servers.as_ref()?.entries.get(i))
            .cloned()
        {
            let name = cx.new(|cx| InputState::new(window, cx).placeholder("Name"));
            let address = cx.new(|cx| InputState::new(window, cx).placeholder("Address"));
            name.update(cx, |s, cx| s.set_value(entry.name, window, cx));
            address.update(cx, |s, cx| s.set_value(entry.address, window, cx));
            for (input, is_name) in [(&name, true), (&address, false)] {
                cx.subscribe_in(
                    input,
                    window,
                    move |this, input, event: &InputEvent, window, cx| {
                        if !matches!(event, InputEvent::Change) || this.servers_running(cx) {
                            return;
                        }
                        let Some((name, address)) = &this.server_inputs else {
                            return;
                        };
                        if input != (if is_name { name } else { address }) {
                            return;
                        }
                        let value = input.read(cx).value().to_string();
                        if let Some(entry) = this
                            .server_selected
                            .and_then(|i| this.servers.as_mut()?.entries.get_mut(i))
                        {
                            if is_name {
                                entry.name = value;
                            } else {
                                entry.address = value;
                            }
                            this.save_servers(window, cx);
                        }
                    },
                )
                .detach();
            }
            self.server_inputs = Some((name, address));
        }
        cx.notify();
    }

    fn render_servers_page(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let running = self.servers_running(cx);
        let mut panel = div().flex_1().flex().flex_col().min_h_0().child(
            self.page_header(
                cx,
                if running {
                    let mut status =
                        "Minecraft is running; servers.dat is read-only until it exits".to_string();
                    if !self.server_status.is_empty() {
                        status.push_str(&format!(" • {}", self.server_status));
                    }
                    status
                } else if self.server_status.is_empty() {
                    "Servers • changes save immediately".into()
                } else {
                    self.server_status.clone()
                },
            )
            .into_any_element(),
        );
        let Some(servers) = &self.servers else {
            return panel.into_any_element();
        };
        let selected = self.server_selected;
        let len = servers.entries.len();
        let mut rows = div().flex().flex_col().gap_1().p_2();
        if len == 0 {
            rows = rows.child("No servers. Add a server to get started.");
        }
        for (i, entry) in servers.entries.iter().enumerate() {
            rows = rows.child(
                div()
                    .id(("server", i))
                    .px_3()
                    .py_2()
                    .bg(if selected == Some(i) {
                        rgb(0x3d5a80)
                    } else {
                        rgb(0x2b2b2b)
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.select_server(Some(i), window, cx)
                    }))
                    .child(format!("{} — {}", entry.name, entry.address)),
            );
        }
        panel = panel.child(
            div()
                .id("servers-scroll")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .child(rows),
        );
        let mut buttons = div().flex().gap_2().p_2();
        buttons = buttons.child(
            Button::new("server-add")
                .label("Add")
                .disabled(running)
                .on_click(cx.listener(|this, _, window, cx| {
                    if this.servers_running(cx) {
                        return;
                    }
                    if let Some(list) = &mut this.servers {
                        list.entries.push(ServerEntry {
                            name: "Minecraft Server".into(),
                            ..Default::default()
                        });
                        let index = list.entries.len() - 1;
                        this.select_server(Some(index), window, cx);
                        this.save_servers(window, cx);
                    }
                })),
        );
        buttons = buttons.child(
            Button::new("server-remove")
                .label("Remove")
                .disabled(running || selected.is_none())
                .on_click(cx.listener(|this, _, window, cx| {
                    if this.servers_running(cx) {
                        return;
                    }
                    if let (Some(i), Some(list)) = (this.server_selected, &mut this.servers) {
                        list.entries.remove(i);
                        let next = if list.entries.is_empty() {
                            None
                        } else {
                            Some(i.min(list.entries.len() - 1))
                        };
                        this.select_server(next, window, cx);
                        this.save_servers(window, cx);
                    }
                })),
        );
        for (up, label) in [(true, "Move Up"), (false, "Move Down")] {
            let disabled =
                running || selected.is_none_or(|i| if up { i == 0 } else { i + 1 >= len });
            buttons = buttons.child(
                Button::new(if up { "server-up" } else { "server-down" })
                    .label(label)
                    .disabled(disabled)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if this.servers_running(cx) {
                            return;
                        }
                        if let (Some(i), Some(list)) = (this.server_selected, &mut this.servers) {
                            let next = if up {
                                i.checked_sub(1)
                            } else {
                                (i + 1 < list.entries.len()).then_some(i + 1)
                            };
                            if let Some(next) = next {
                                list.entries.swap(i, next);
                                this.select_server(Some(next), window, cx);
                                this.save_servers(window, cx);
                            }
                        }
                    })),
            );
        }
        panel = panel.child(buttons);
        if let Some((name, address)) = &self.server_inputs {
            let policy = selected
                .and_then(|i| servers.entries.get(i))
                .map(|e| e.accept_textures)
                .unwrap_or_default();
            let mut form = div()
                .flex()
                .flex_col()
                .gap_2()
                .p_3()
                .child("Name")
                .child(Input::new(name).disabled(running))
                .child("Address")
                .child(Input::new(address).disabled(running))
                .child("Server resource packs");
            let mut policies = div().flex().gap_2();
            for (value, label) in [
                (ResourcePackPolicy::Prompt, "Prompt"),
                (ResourcePackPolicy::Enabled, "Enabled"),
                (ResourcePackPolicy::Disabled, "Disabled"),
            ] {
                policies = policies.child(
                    Button::new(label)
                        .disabled(running)
                        .label(if policy == value {
                            format!("• {label}")
                        } else {
                            label.into()
                        })
                        .on_click(cx.listener(move |this, _, window, cx| {
                            if this.servers_running(cx) {
                                return;
                            }
                            if let Some(entry) = this
                                .server_selected
                                .and_then(|i| this.servers.as_mut()?.entries.get_mut(i))
                            {
                                entry.accept_textures = value;
                                this.save_servers(window, cx);
                            }
                        })),
                );
            }
            form = form.child(policies);
            panel = panel.child(form);
        }
        panel.into_any_element()
    }

    fn render_screenshots_page(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let inst = self.instance(cx);
        let dir = inst.game_root().join("screenshots");
        let mut list = div().flex().flex_col().gap_1().p_3();
        let mut found = false;
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
                if !name.to_lowercase().ends_with(".png") {
                    continue;
                }
                found = true;
                list = list.child(
                    div().px_3().py_2().bg(rgb(0x2b2b2b)).rounded_md().child(name.to_string()),
                );
            }
        }
        if !found {
            list = list.child(div().text_color(rgb(0x777777)).child("No screenshots."));
        }

        div()
            .flex_1()
            .flex()
            .flex_col()
            .min_h_0()
            .child(self.page_header(cx, dir.display().to_string()).into_any_element())
            .child(div().id("shots-scroll").flex_1().overflow_y_scroll().child(list))
            .into_any_element()
    }
}

impl Render for InstanceWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.page.available(self.instance(cx).is_horizon()) {
            self.page = Page::Log;
        }
        if self.page == Page::Settings && self.settings_editor.is_none() {
            if let Some(instance) = self.app.read(cx).instances.get(&self.instance_id).cloned() {
                let paths = self.app.read(cx).paths.clone();
                self.settings_editor = crate::instance_settings_page::SettingsEditor::load(
                    &instance, &paths, window, cx,
                ).ok();
            }
        }
        let running = self.servers_running(cx);
        if self.servers_was_running && !running {
            self.servers = None;
            self.servers_snapshot = None;
            self.server_selected = None;
            self.server_inputs = None;
            self.server_status.clear();
        }
        self.servers_was_running = running;
        self.sync_worlds_page(running, cx);
        if self.page == Page::Servers && self.servers.is_none() && self.server_status.is_empty() {
            match ServerListSnapshot::load(&self.instance(cx).game_root()) {
                Ok((list, snapshot)) => {
                    let selected = (!list.entries.is_empty()).then_some(0);
                    self.servers = Some(list);
                    self.servers_snapshot = Some(snapshot);
                    self.select_server(selected, window, cx);
                }
                Err(e) => self.server_status = format!("Load failed: {e}"),
            }
        }
        if let Some(provider) = self.page.browser() {
            let placeholder = match provider {
                Browser::Modrinth(_) => "Search Modrinth…",
                Browser::CurseForge => "Search CurseForge…",
            };
            self.browsers.entry(provider).or_insert_with(|| BrowserState {
                input: cx.new(|cx| InputState::new(window, cx).placeholder(placeholder)),
                target: None,
                results: Vec::new(),
                searching: false,
                search_status: "Search by name or leave blank to browse.".into(),
                installing: HashSet::new(),
                install_status: String::new(),
            });
        }
        if running {
            cx.spawn(async move |this, cx| {
                let _ = cx.background_executor().timer(std::time::Duration::from_millis(500)).await;
                let _ = this.update(cx, |_, cx| cx.notify());
            })
            .detach();
        }

        div()
            .size_full()
            .flex()
            .bg(rgb(0x232323))
            .text_color(rgb(0xdddddd))
            .child(self.render_sidebar(cx))
            .child(self.render_page(cx))
    }
}
