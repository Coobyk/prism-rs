//! Instance window: per-instance pages (Log, Version, Mods, Resource Packs,
//! Shader Packs, Notes, Settings), mirroring PrismLauncher's InstanceWindow
//! + instance pages.

use crate::state::{App, InstanceRunState};
use gpui_kit::base::Disableable;
use gpui_kit::component::button::Button;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::*;
use prism_core::modplatform::{LocalResource, ResourceType};
use prism_core::servers::{ResourcePackPolicy, ServerEntry, ServerList, ServerListSnapshot};
use prism_core::{Instance, InstanceKind};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Page {
    Log,
    Version,
    Mods,
    ResourcePacks,
    ShaderPacks,
    BrowseMods,
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
            Page::ResourcePacks => "Resource Packs",
            Page::ShaderPacks => "Shader Packs",
            Page::BrowseMods => "Browse Mods (Modrinth)",
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
}

#[cfg(test)]
mod tests {
    use super::Page;

    #[test]
    fn resource_pack_editor_is_only_available_for_java() {
        assert!(Page::ResourcePacks.available(false));
        assert!(!Page::ResourcePacks.available(true));
    }
}

pub struct InstanceWindow {
    pub app: gpui::Entity<App>,
    pub instance_id: String,
    pub page: Page,
    // Modrinth browse state.
    search_input: Option<gpui::Entity<gpui_kit::component::input::InputState>>,
    search_results: Arc<std::sync::Mutex<Vec<prism_core::modplatform::modrinth::SearchResult>>>,
    search_handle: Option<tokio::task::JoinHandle<Result<Vec<prism_core::modplatform::modrinth::SearchResult>, String>>>,
    search_status: Arc<std::sync::Mutex<String>>,
    installing: Arc<std::sync::Mutex<std::collections::BTreeSet<String>>>,
    // Loader install state.
    loader_options: Arc<std::sync::Mutex<Vec<String>>>,
    loader_handle: Option<tokio::task::JoinHandle<Result<Vec<String>, String>>>,
    servers: Option<ServerList>,
    servers_snapshot: Option<ServerListSnapshot>,
    server_selected: Option<usize>,
    server_inputs: Option<(gpui::Entity<InputState>, gpui::Entity<InputState>)>,
    server_status: String,
    servers_was_running: bool,
}

impl InstanceWindow {
    pub fn new(app: gpui::Entity<App>, instance_id: String) -> Self {
        InstanceWindow {
            app,
            instance_id,
            page: Page::Log,
            search_input: None,
            search_results: Arc::new(std::sync::Mutex::new(Vec::new())),
            search_handle: None,
            search_status: Arc::new(std::sync::Mutex::new(String::new())),
            installing: Arc::new(std::sync::Mutex::new(Default::default())),
            loader_options: Arc::new(std::sync::Mutex::new(Vec::new())),
            loader_handle: None,
            servers: None,
            servers_snapshot: None,
            server_selected: None,
            server_inputs: None,
            server_status: String::new(),
            servers_was_running: false,
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

    fn start_search(&mut self, query: &str, cx: &mut Context<Self>) {
        let app = self.app.read(cx);
        let runtime = app.runtime.clone();
        let Some(inst) = app.instances.get(&self.instance_id).cloned() else {
            return;
        };
        drop(app);
        let mc_version = prism_core::launch::LaunchSettings::for_instance(&inst, &self.app.read(cx).paths).mc_version;
        let loader = prism_core::instance::loader_of(&inst);
        let loader_name = match loader {
            prism_core::loaders::Loader::Fabric { .. } => Some("fabric"),
            prism_core::loaders::Loader::Quilt { .. } => Some("quilt"),
            prism_core::loaders::Loader::Forge { .. } => Some("forge"),
            prism_core::loaders::Loader::NeoForge { .. } => Some("neoforge"),
            prism_core::loaders::Loader::None => None,
        };

        let results = self.search_results.clone();
        let status = self.search_status.clone();
        *status.lock().unwrap_or_else(|p| p.into_inner()) = "Searching…".to_string();
        let query = query.to_string();
        self.search_handle = Some(runtime.spawn(async move {
            let api = prism_core::modplatform::modrinth::ModrinthApi::with_client(
                prism_core::net::Downloader::new().client().clone(),
            );
            match api.search(&query, prism_core::modplatform::ResourceType::Mod, Some(&mc_version), 25).await {
                Ok(resp) => Ok(resp.hits),
                Err(e) => Err(e.to_string()),
            }
            .map_err(|e: String| {
                status.lock().unwrap_or_else(|p| p.into_inner()).clone();
                e
            })
        }));
        let _ = loader_name;
        cx.notify();
    }

    fn poll_search(&mut self, cx: &mut Context<Self>) {
        if let Some(handle) = &self.search_handle {
            if handle.is_finished() {
                let handle = self.search_handle.take().unwrap();
                let results = self.search_results.clone();
                let status = self.search_status.clone();
                match self.app.read(cx).block_on(handle) {
                    Ok(Ok(hits)) => {
                        *results.lock().unwrap_or_else(|p| p.into_inner()) = hits;
                        *status.lock().unwrap_or_else(|p| p.into_inner()) = String::new();
                    }
                    Ok(Err(e)) => {
                        *status.lock().unwrap_or_else(|p| p.into_inner()) = format!("Search failed: {e}");
                    }
                    Err(e) => {
                        *status.lock().unwrap_or_else(|p| p.into_inner()) = format!("Search failed: {e}");
                    }
                }
                cx.notify();
            }
        }
    }

    fn install_mod(&mut self, project: prism_core::modplatform::modrinth::SearchResult, cx: &mut Context<Self>) {
        let app = self.app.read(cx);
        let runtime = app.runtime.clone();
        let downloader = app.runtime.downloader.clone();
        let Some(inst) = app.instances.get(&self.instance_id).cloned() else {
            return;
        };
        let paths = app.paths.clone();
        drop(app);

        let mc_version = prism_core::launch::LaunchSettings::for_instance(&inst, &paths).mc_version;
        let loader = prism_core::instance::loader_of(&inst);
        let loader_name = match loader {
            prism_core::loaders::Loader::Fabric { .. } => "fabric",
            prism_core::loaders::Loader::Quilt { .. } => "quilt",
            prism_core::loaders::Loader::Forge { .. } => "forge",
            prism_core::loaders::Loader::NeoForge { .. } => "neoforge",
            prism_core::loaders::Loader::None => "",
        };
        let loaders: Vec<&str> = if loader_name.is_empty() { vec![] } else { vec![loader_name] };

        let slug = project.slug.clone();
        let title = project.title.clone();
        {
            let mut installing = self.installing.lock().unwrap_or_else(|p| p.into_inner());
            if installing.contains(&slug) {
                return;
            }
            installing.insert(slug.clone());
        }

        let status = self.search_status.clone();
        let installing = self.installing.clone();
        runtime.spawn(async move {
            let api = prism_core::modplatform::modrinth::ModrinthApi::with_client(
                prism_core::net::Downloader::new().client().clone(),
            );
            let result = async {
                let versions = api.versions(&project.project_id, &loaders, &[&mc_version]).await?;
                let version = versions.first().ok_or_else(|| prism_core::modplatform::PlatformError::Request("no compatible version".into()))?;
                let file = api.primary_file(version).ok_or_else(|| prism_core::modplatform::PlatformError::Request("no file".into()))?.clone();
                let game_root = inst.game_root();
                api.install_file(&downloader, &game_root, &file, prism_core::modplatform::ResourceType::Mod).await?;
                Ok::<(), prism_core::modplatform::PlatformError>(())
            }.await;
            match result {
                Ok(()) => {
                    *status.lock().unwrap_or_else(|p| p.into_inner()) = format!("Installed {title}");
                }
                Err(e) => {
                    *status.lock().unwrap_or_else(|p| p.into_inner()) = format!("Install failed: {e}");
                }
            }
            installing.lock().unwrap_or_else(|p| p.into_inner()).remove(&slug);
        });
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
        let mut col = div().w(px(170.)).flex().flex_col().bg(rgb(0x2b2b2b)).border_r_1().border_color(rgb(0x1c1c1c));
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
            let active = self.page == page;
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
            Page::Mods => self.render_resources_page(cx, ResourceType::Mod, "Mods"),
            Page::ResourcePacks => self.render_resources_page(cx, ResourceType::ResourcePack, "Resource Packs"),
            Page::ShaderPacks => self.render_resources_page(cx, ResourceType::ShaderPack, "Shader Packs"),
            Page::Notes => self.render_notes_page(cx),
            Page::Settings => self.render_settings_page(cx),
            Page::Worlds => self.render_worlds_page(cx),
            Page::Servers => self.render_servers_page(cx),
            Page::Screenshots => self.render_screenshots_page(cx),
            Page::BrowseMods => self.render_browse_mods_page(cx),
        }
    }

    fn page_header(&self, cx: &Context<Self>, subtitle: String) -> impl IntoElement {
        let inst = self.instance(cx);
        div()
            .flex()
            .items_center()
            .justify_between()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(rgb(0x1c1c1c))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(div().text_size(px(15.)).font_weight(FontWeight::BOLD).child(inst.name))
                    .child(div().text_size(px(11.)).text_color(rgb(0x888888)).child(subtitle)),
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
                    .children(tail.into_iter().map(|l| div().child(l).into_any_element())),
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

    fn render_resources_page(&self, cx: &mut Context<Self>, rtype: ResourceType, title: &str) -> gpui::AnyElement {
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

        div()
            .flex_1()
            .flex()
            .flex_col()
            .min_h_0()
            .child(self.page_header(cx, folder.display().to_string()).into_any_element())
            .child(
                div()
                    .id("resources-scroll")
                    .flex_1()
                    .overflow_y_scroll()
                    .child(list),
            )
            .into_any_element()
    }

    fn render_browse_mods_page(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        self.poll_search(cx);
        let inst = self.instance(cx);
        let mc_version = prism_core::launch::LaunchSettings::for_instance(&inst, &self.app.read(cx).paths).mc_version;

        // Lazily create the search input (needs a window; done in render()).
        let Some(input) = self.search_input.clone() else {
            return div().into_any_element();
        };
        let query = input.read(cx).value().to_string();

        let status = self.search_status.lock().unwrap_or_else(|p| p.into_inner()).clone();
        let results = self.search_results.lock().unwrap_or_else(|p| p.into_inner()).clone();
        let installing = self.installing.lock().unwrap_or_else(|p| p.into_inner()).clone();

        let mut panel = div()
            .flex_1()
            .flex()
            .flex_col()
            .min_h_0()
            .child(self.page_header(cx, format!("Modrinth • Minecraft {mc_version}")).into_any_element());

        let mut search_row = div().flex().gap_2().p_2().items_center();
        search_row = search_row
            .child(div().flex_1().child(gpui_kit::component::input::Input::new(&input)))
            .child(
                Button::new("modrinth-search")
                    .label("Search")
                    .on_click(cx.listener(move |this, _ev, _window, cx| {
                        let q = this
                            .search_input
                            .as_ref()
                            .map(|i| i.read(cx).value().to_string())
                            .unwrap_or_default();
                        if !q.trim().is_empty() {
                            this.start_search(&q, cx);
                        }
                    })),
            );
        panel = panel.child(search_row);

        if !status.is_empty() {
            panel = panel.child(
                div()
                    .px_3()
                    .text_size(px(12.))
                    .text_color(rgb(0x8fb4d9))
                    .child(status),
            );
        }

        let mut list = div()
            .id("modrinth-list")
            .flex_1()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_1()
            .p_2();
        for r in &results {
            let slug = r.slug.clone();
            let is_installing = installing.contains(&slug);
            let project = r.clone();
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
                            .child(div().text_ellipsis().child(r.title.clone()))
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(rgb(0x888888))
                                    .text_ellipsis()
                                    .child(r.description.clone()),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(0x666666))
                            .child(format!("{} downloads", r.downloads)),
                    )
                    .child(
                        Button::new(SharedString::from(format!("install-{slug}")))
                            .label(if is_installing { "Installing…" } else { "Install" })
                            .disabled(is_installing)
                            .on_click(cx.listener(move |this, _ev, _window, _cx| {
                                this.install_mod(project.clone(), _cx);
                            })),
                    ),
            );
        }
        panel = panel.child(list);
        panel.into_any_element()
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
        let inst = self.instance(cx);
        let cfg = prism_core::ini::IniFile::load(&inst.root.join("instance.cfg")).unwrap_or_default();
        let min_mem = cfg.get_or("MinMemAlloc", "512");
        let max_mem = cfg.get_or("MaxMemAlloc", "4096");
        let java_path = cfg.get_or("JavaPath", "(auto)");
        let width = cfg.get_or("MinecraftWinWidth", "854");
        let height = cfg.get_or("MinecraftWinHeight", "480");

        let rows: Vec<(&str, String)> = vec![
            ("Minimum memory (MB)", min_mem),
            ("Maximum memory (MB)", max_mem),
            ("Java path", java_path),
            ("Window width", width),
            ("Window height", height),
        ];

        let mut list = div().flex().flex_col().gap_1().p_3();
        for (label, value) in rows {
            list = list.child(
                div()
                    .flex()
                    .justify_between()
                    .px_3()
                    .py_2()
                    .bg(rgb(0x2b2b2b))
                    .rounded_md()
                    .child(div().text_color(rgb(0xaaaaaa)).child(label.to_string()))
                    .child(div().child(value)),
            );
        }
        list = list.child(
            div()
                .text_size(px(11.))
                .text_color(rgb(0x777777))
                .child("Editing is read-only for now; edit instance.cfg directly to change these."),
        );

        div()
            .flex_1()
            .flex()
            .flex_col()
            .min_h_0()
            .child(self.page_header(cx, "Instance settings".to_string()).into_any_element())
            .child(div().id("settings-scroll").flex_1().overflow_y_scroll().child(list))
            .into_any_element()
    }

    fn render_worlds_page(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let inst = self.instance(cx);
        let saves = inst.game_root().join("saves");
        let mut list = div().flex().flex_col().gap_1().p_3();
        let mut found = false;
        if let Ok(entries) = std::fs::read_dir(&saves) {
            for entry in entries.flatten() {
                if !entry.path().is_dir() {
                    continue;
                }
                found = true;
                let name = entry.file_name().to_string_lossy().to_string();
                list = list.child(
                    div()
                        .px_3()
                        .py_2()
                        .bg(rgb(0x2b2b2b))
                        .rounded_md()
                        .child(name),
                );
            }
        }
        if !found {
            list = list.child(div().text_color(rgb(0x777777)).child("No worlds."));
        }

        div()
            .flex_1()
            .flex()
            .flex_col()
            .min_h_0()
            .child(self.page_header(cx, saves.display().to_string()).into_any_element())
            .child(div().id("worlds-scroll").flex_1().overflow_y_scroll().child(list))
            .into_any_element()
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
        let running = self.servers_running(cx);
        if self.servers_was_running && !running {
            self.servers = None;
            self.servers_snapshot = None;
            self.server_selected = None;
            self.server_inputs = None;
            self.server_status.clear();
        }
        self.servers_was_running = running;
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
        if self.search_input.is_none() {
            self.search_input =
                Some(cx.new(|cx| {
                    gpui_kit::component::input::InputState::new(window, cx)
                        .placeholder("Search Modrinth…")
                }));
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
