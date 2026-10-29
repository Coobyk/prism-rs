//! Instance window: per-instance pages (Log, Version, Mods, Resource Packs,
//! Shader Packs, Notes, Settings), mirroring PrismLauncher's InstanceWindow
//! + instance pages.

use crate::state::{App, InstanceRunState};
use gpui_kit::base::Disableable;
use gpui_kit::component::button::Button;
use gpui_kit::*;
use prism_core::modplatform::{LocalResource, ResourceType};
use prism_core::Instance;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Page {
    Log,
    Version,
    Mods,
    ResourcePacks,
    ShaderPacks,
    Notes,
    Settings,
    Worlds,
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
            Page::Notes => "Notes",
            Page::Settings => "Settings",
            Page::Worlds => "Worlds",
            Page::Screenshots => "Screenshots",
        }
    }

    pub fn all() -> Vec<Page> {
        vec![
            Page::Log,
            Page::Version,
            Page::Mods,
            Page::ResourcePacks,
            Page::ShaderPacks,
            Page::Worlds,
            Page::Screenshots,
            Page::Notes,
            Page::Settings,
        ]
    }
}

pub struct InstanceWindow {
    pub app: gpui::Entity<App>,
    pub instance_id: String,
    pub page: Page,
}

impl InstanceWindow {
    pub fn new(app: gpui::Entity<App>, instance_id: String) -> Self {
        InstanceWindow {
            app,
            instance_id,
            page: Page::Log,
        }
    }

    fn instance(&self, cx: &Context<Self>) -> Instance {
        self.app
            .read(cx)
            .instances
            .get(&self.instance_id)
            .cloned()
            .unwrap_or(Instance {
                id: self.instance_id.clone(),
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

        for page in Page::all() {
            let active = self.page == page;
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
                    .child(page.label().to_string()),
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
                            this.app.update(cx, |app, _cx| app.start_launch(&id));
                            cx.notify();
                        })),
                ),
        )
    }

    fn render_page(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        match self.page {
            Page::Log => self.render_log_page(cx),
            Page::Version => self.render_version_page(cx),
            Page::Mods => self.render_resources_page(cx, ResourceType::Mod, "Mods"),
            Page::ResourcePacks => self.render_resources_page(cx, ResourceType::ResourcePack, "Resource Packs"),
            Page::ShaderPacks => self.render_resources_page(cx, ResourceType::ShaderPack, "Shader Packs"),
            Page::Notes => self.render_notes_page(cx),
            Page::Settings => self.render_settings_page(cx),
            Page::Worlds => self.render_worlds_page(cx),
            Page::Screenshots => self.render_screenshots_page(cx),
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

    fn render_version_page(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let inst = self.instance(cx);
        let loader = prism_core::instance::loader_of(&inst);
        let mc_version = prism_core::launch::LaunchSettings::for_instance(&inst, &self.app.read(cx).paths)
            .mc_version;

        let mut rows: Vec<(String, String)> = vec![
            ("net.minecraft".to_string(), mc_version),
        ];
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

        div()
            .flex_1()
            .flex()
            .flex_col()
            .min_h_0()
            .child(self.page_header(cx, "Installed components".to_string()).into_any_element())
            .child(
                div()
                    .id("version-scroll")
                    .flex_1()
                    .overflow_y_scroll()
                    .child(list),
            )
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
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let running = matches!(
            self.app.read(cx).run_state(&self.instance_id),
            InstanceRunState::Running | InstanceRunState::Preparing
        );
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
