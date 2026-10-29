//! Main window: the launcher shell.
//!
//! Layout mirrors PrismLauncher's `MainWindow.ui`:
//! - top main toolbar (Add Instance | Folders, Settings, Help, Check Updates
//!   | Accounts)
//! - central instance view (grouped icon grid) with a right-side instance
//!   toolbar (name, Launch, Kill, Edit, ...)
//! - bottom news toolbar, status bar and a live console panel.

use crate::dialogs::CreateInstanceState;
use crate::state::{format_play_time, App, InstanceRunState};
use gpui_kit::base::{Disableable, InteractiveElementExt};
use gpui_kit::component::button::Button;
use gpui_kit::*;

pub struct MainWindow {
    pub app: gpui::Entity<App>,
    pub create_dialog: Option<CreateInstanceState>,
    console_open: bool,
    console_filter: String,
}

impl MainWindow {
    pub fn new(app: gpui::Entity<App>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Kick off the version manifest fetch for the create dialog.
        app.update(cx, |app, _| app.start_manifest_fetch());
        MainWindow {
            app,
            create_dialog: None,
            console_open: false,
            console_filter: String::new(),
        }
    }

    fn poll_manifest(&mut self, cx: &mut Context<Self>) {
        self.app.update(cx, |app, _| app.poll_manifest());
    }


    fn open_create_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.create_dialog = Some(CreateInstanceState::new(window, cx));
        cx.notify();
    }

    fn close_create_dialog(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.create_dialog = None;
        cx.notify();
    }

    // -- toolbar ------------------------------------------------------------

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("main-toolbar")
            .flex()
            .items_center()
            .gap_1()
            .px_2()
            .h(px(40.))
            .bg(rgb(0x2b2b2b))
            .border_b_1()
            .border_color(rgb(0x1c1c1c))
            .child(
                Button::new("add-instance")
                    .label("+ Add Instance")
                    .on_click(cx.listener(Self::on_add_instance)),
            )
            .child(toolbar_separator())
            .child(Button::new("folders").label("Folders").on_click(cx.listener(Self::on_view_instances_folder)))
            .child(Button::new("settings").label("Settings").on_click(cx.listener(Self::on_open_settings)))
            .child(Button::new("help").label("Help").on_click(|_, _, _| {}))
            .child(Button::new("check-updates").label("Check for Updates").on_click(|_, _, _| {}))
            .child(
                Button::new("accounts")
                    .label(self.accounts_button_label(cx))
                    .on_click(cx.listener(Self::on_open_accounts)),
            )
    }

    fn on_open_accounts(&mut self, _ev: &ClickEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let app = self.app.clone();
        open_window(WindowOptions::default(), cx, |_window, cx| {
            cx.new(|_| crate::accounts_window::AccountsWindow::new(app.clone()))
        })
        .ok();
    }

    fn on_open_settings(&mut self, _ev: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        let app = self.app.clone();
        let options = WindowOptions {
            window_min_size: Some(gpui::Size {
                width: px(560.),
                height: px(420.),
            }),
            ..WindowOptions::default()
        };
        open_window(options, cx, |window, cx| {
            cx.new(|cx| crate::settings_window::SettingsWindow::new(app.clone(), window, cx))
        })
        .ok();
    }

    fn accounts_button_label(&self, cx: &Context<Self>) -> String {
        let app = self.app.read(cx);
        match app.accounts.active_account() {
            Some(acc) => format!("👤 {}", acc.player_name),
            None => "Accounts".to_string(),
        }
    }

    fn on_add_instance(&mut self, _ev: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.open_create_dialog(window, cx);
    }

    fn on_view_instances_folder(&mut self, _ev: &ClickEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let dir = self.app.read(cx).paths.instances_dir();
        let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
    }

    fn on_cycle_account(&mut self, _ev: &ClickEvent, _window: &mut Window, cx: &mut Context<Self>) {
        self.app.update(cx, |app, _cx| {
            if app.accounts.accounts.is_empty() {
                app.accounts.add_offline("Player");
            } else {
                // Cycle to the next account.
                let idx = app
                    .accounts
                    .active
                    .as_deref()
                    .and_then(|id| app.accounts.accounts.iter().position(|a| a.id == id))
                    .map(|i| (i + 1) % app.accounts.accounts.len())
                    .unwrap_or(0);
                app.accounts.active = Some(app.accounts.accounts[idx].id.clone());
            }
            app.save_accounts();
        });
        cx.notify();
    }

    // -- instance grid ------------------------------------------------------

    fn render_instance_grid(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let instances: Vec<(String, String, Option<String>)> = {
            let app = self.app.read(cx);
            app.instances
                .instances
                .iter()
                .map(|i| (i.id.clone(), i.name.clone(), i.group.clone()))
                .collect()
        };

        if instances.is_empty() {
            return div()
                .flex_1()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(rgb(0x888888))
                .child("Add an instance to get started")
                .into_any_element();
        }

        // Group instances, preserving group order then ungrouped.
        let mut ordered: Vec<(String, Vec<usize>)> = Vec::new();
        for (idx, (_, _, group)) in instances.iter().enumerate() {
            let key = group.clone().unwrap_or_default();
            match ordered.iter_mut().find(|(g, _)| *g == key) {
                Some((_, list)) => list.push(idx),
                None => ordered.push((key, vec![idx])),
            }
        }

        let mut container = div()
            .id("instance-list")
            .flex_1()
            .flex()
            .flex_col()
            .gap_3()
            .p_3()
            .overflow_y_scroll();
        for (group, indices) in ordered {
            if !group.is_empty() {
                container = container.child(
                    div()
                        .text_color(rgb(0xaaaaaa))
                        .text_size(px(13.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(group.clone()),
                );
            }
            let mut grid = div().flex().flex_wrap().gap_2().w_full();
            for idx in indices {
                let (id, name, _) = instances[idx].clone();
                grid = grid.child(self.render_instance_tile(id, name, cx));
            }
            container = container.child(grid);
        }
        container.into_any_element()
    }

    fn render_instance_tile(&self, id: String, name: String, cx: &mut Context<Self>) -> impl IntoElement {
        let app = self.app.clone();
        let selected = {
            let app = app.read(cx);
            app.selected.as_deref() == Some(id.as_str())
        };
        let running = app.read(cx).is_running(&id);
        let tile_bg = if selected { rgb(0x3d5a80) } else { rgb(0x2b2b2b) };
        let id2 = id.clone();

        div()
            .id(SharedString::from(format!("tile-{id}")))
            .flex()
            .flex_col()
            .items_center()
            .gap_1()
            .p_2()
            .w(px(110.))
            .rounded_md()
            .bg(tile_bg)
            .hover(|s| s.bg(rgb(0x353535)))
            .on_click(cx.listener(move |this, _event, _window, cx| {
                let id = id.clone();
                this.app.update(cx, |app, _| {
                    app.selected = if app.selected.as_deref() == Some(id.as_str()) {
                        None
                    } else {
                        Some(id.clone())
                    };
                });
                cx.notify();
            }))
            .on_double_click(cx.listener(move |this, _event, _window, cx| {
                let id = id2.clone();
                this.app.update(cx, |app, _cx| {
                    app.selected = Some(id.clone());
                    app.start_launch(&id);
                });
                cx.notify();
            }))
            .child(
                div()
                    .size(px(64.))
                    .rounded_md()
                    .bg(rgb(0x1e4d2b))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(28.))
                    .font_weight(FontWeight::BOLD)
                    .text_color(rgb(0x9fdcb8))
                    .child(if running {
                        "▶".to_string()
                    } else {
                        name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_else(|| "?".to_string())
                    }),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .max_w(px(100.))
                    .text_ellipsis()
                    .child(name),
            )
    }

    // -- sidebar ------------------------------------------------------------

    fn render_instance_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let app = self.app.read(cx);
        let selected = app.selected.clone();
        let name = selected
            .as_deref()
            .and_then(|id| app.instances.get(id))
            .map(|i| i.name.clone())
            .unwrap_or_default();
        let enabled = selected.is_some();
        let is_running = selected
            .as_deref()
            .map(|id| app.is_running(id))
            .unwrap_or(false);
        let group = selected
            .as_deref()
            .and_then(|id| app.instances.get(id))
            .and_then(|i| i.group.clone())
            .unwrap_or_default();

        let app_entity = self.app.clone();
        div()
            .w(px(160.))
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .bg(rgb(0x2b2b2b))
            .border_l_1()
            .border_color(rgb(0x1c1c1c))
            .child(
                div()
                    .text_size(px(14.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_ellipsis()
                    .child(if name.is_empty() { "No selection" } else { name.as_str() }.to_string()),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(rgb(0x888888))
                    .child(if group.is_empty() { "No group" } else { group.as_str() }.to_string()),
            )
            .child(toolbar_separator())
            .child(self.sidebar_button("rename", "Rename", enabled).on_click(cx.listener(Self::on_rename)))
            .child(self.sidebar_button("change-icon", "Change Icon", enabled))
            .child(toolbar_separator())
            .child(
                self.sidebar_button("launch", "Launch", enabled && !is_running)
                    .on_click(cx.listener(Self::on_launch)),
            )
            .child(
                self.sidebar_button("kill", "Kill", is_running)
                    .on_click(cx.listener(Self::on_kill)),
            )
            .child(toolbar_separator())
            .child(
                self.sidebar_button("edit", "Edit", enabled)
                    .on_click(cx.listener(Self::on_edit)),
            )
            .child(self.sidebar_button("change-group", "Change Group", enabled))
            .child(self.sidebar_button("view-folder", "View Folder", enabled).on_click(cx.listener(Self::on_view_folder)))
            .child(self.sidebar_button("export", "Export", enabled))
            .child(self.sidebar_button("copy", "Copy", enabled))
            .child(
                self.sidebar_button("delete", "Delete", enabled)
                    .on_click(cx.listener(Self::on_delete)),
            )
            .child(self.sidebar_button("shortcut", "Create Shortcut", enabled))
    }

    fn sidebar_button(&self, id: &str, label: &str, enabled: bool) -> Button {
        let mut btn = Button::new(SharedString::from(format!("sb-{id}"))).label(label);
        if !enabled {
            btn = btn.disabled(true);
        }
        btn
    }

    fn on_rename(&mut self, _ev: &ClickEvent, _window: &mut Window, cx: &mut Context<Self>) {
        // Cycle name with a " (renamed)" suffix as placeholder for the rename
        // dialog; a proper inline-rename editor follows.
        self.app.update(cx, |app, _cx| {
            if let Some(id) = app.selected.clone() {
                if let Some(inst) = app.instances.get_mut(&id) {
                    inst.name = format!("{} (renamed)", inst.name);
                    let inst = inst.clone();
                    let _ = inst.save();
                }
            }
        });
        cx.notify();
    }

    fn on_launch(&mut self, _ev: &ClickEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let id = self.app.read(cx).selected.clone();
        if let Some(id) = id {
            self.console_open = true;
            self.app.update(cx, |app, _cx| app.start_launch(&id));
        }
        cx.notify();
    }

    fn on_kill(&mut self, _ev: &ClickEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let id = self.app.read(cx).selected.clone();
        if let Some(id) = id {
            self.app.update(cx, |app, _cx| app.kill_instance(&id));
        }
        cx.notify();
    }

    fn on_edit(&mut self, _ev: &ClickEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.app.read(cx).selected.clone() else {
            return;
        };
        let app = self.app.clone();
        let options = WindowOptions {
            window_min_size: Some(gpui::Size {
                width: px(760.),
                height: px(520.),
            }),
            ..WindowOptions::default()
        };
        open_window(options, cx, |_window, cx| {
            cx.new(|_| crate::instance_window::InstanceWindow::new(app, id))
        })
        .ok();
    }

    fn on_view_folder(&mut self, _ev: &ClickEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let dir = self
            .app
            .read(cx)
            .selected_instance()
            .map(|i| i.root.clone());
        if let Some(dir) = dir {
            let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
        }
    }

    fn on_delete(&mut self, _ev: &ClickEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let id = self.app.read(cx).selected.clone();
        self.app.update(cx, |app, _cx| {
            if let Some(id) = id {
                let _ = prism_core::InstanceList::remove(&app.paths, &id);
                if app.selected.as_deref() == Some(id.as_str()) {
                    app.selected = None;
                }
                app.reload_instances();
            }
        });
        cx.notify();
    }

    // -- console ------------------------------------------------------------

    fn render_console(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let selected = self.app.read(cx).selected.clone();
        let mut lines: Vec<String> = Vec::new();
        let run_state = InstanceRunState::Idle;
        if let Some(id) = &selected {
            if let Some(running) = self.app.read(cx).running.get(id) {
                if let Ok(log) = running.log.lock() {
                    lines = log.clone();
                }
                let s = running.state.lock().map(|s| s.clone()).unwrap_or(InstanceRunState::Idle);
                lines.push(format!("--- {s:?} ---"));
            }
        }
        let _ = run_state;

        div()
            .flex()
            .flex_col()
            .h(px(220.))
            .border_t_1()
            .border_color(rgb(0x1c1c1c))
            .bg(rgb(0x1a1a1a))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_2()
                    .h(px(26.))
                    .bg(rgb(0x242424))
                    .child(div().text_size(px(12.)).child("Console"))
                    .child(
                        div()
                            .id("console-close")
                            .px_2()
                            .text_size(px(12.))
                            .on_click(cx.listener(|this, _ev, _window, cx| {
                                this.console_open = false;
                                cx.notify();
                            }))
                            .child("✕"),
                    ),
            )
            .child(
                div()
                    .id("console-log")
                    .flex_1()
                    .overflow_y_scroll()
                    .p_2()
                    .font_family("monospace")
                    .text_size(px(11.))
                    .text_color(rgb(0xcccccc))
                    .children(
                        lines
                            .iter()
                            .rev()
                            .take(500)
                            .rev()
                            .map(|l| div().child(l.clone()).into_any_element()),
                    ),
            )
            .into_any_element()
    }

    // -- bottom bars ---------------------------------------------------------

    fn render_bottom_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let description = self.app.read(cx).status_description();
        let total_played: u64 = self
            .app
            .read(cx)
            .instances
            .instances
            .iter()
            .map(|i| i.total_time_played)
            .sum();

        div()
            .flex()
            .flex_col()
            .border_t_1()
            .border_color(rgb(0x1c1c1c))
            .child(
                // News toolbar
                div()
                    .id("news-bar")
                    .flex()
                    .items_center()
                    .h(px(26.))
                    .px_2()
                    .gap_2()
                    .bg(rgb(0x2b2b2b))
                    .text_size(px(12.))
                    .text_color(rgb(0x99aabb))
                    .child("News")
                    .child("More News…"),
            )
            .child(
                // Status bar
                div()
                    .flex()
                    .items_center()
                    .h(px(22.))
                    .px_2()
                    .gap_4()
                    .bg(rgb(0x242424))
                    .text_size(px(11.))
                    .text_color(rgb(0x999999))
                    .child(description)
                    .child(div().flex_1())
                    .child(format!("Total play time: {}", format_play_time(total_played))),
            )
    }
}

impl Render for MainWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.poll_manifest(cx);
        // Keep state fresh while instances run.
        if self.app.read(cx).running.values().any(|r| {
            matches!(
                r.state.lock().map(|s| s.clone()).unwrap_or(InstanceRunState::Idle),
                InstanceRunState::Preparing | InstanceRunState::Running
            )
        }) {
            cx.spawn(async move |this, cx| {
                let _ = cx.background_executor().timer(std::time::Duration::from_millis(500)).await;
                let _ = this.update(cx, |_, cx| cx.notify());
            })
            .detach();
        }

        let console_open = self.console_open;
        let create_dialog = self
            .create_dialog
            .as_mut()
            .map(|d| d.render(&self.app.clone(), window, cx));

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0x232323))
            .text_color(rgb(0xdddddd))
            .relative()
            .child(self.render_toolbar(cx))
            .child(
                div()
                    .flex_1()
                    .flex()
                    .min_h_0()
                    .child(self.render_instance_grid(cx))
                    .child(self.render_instance_sidebar(cx)),
            )
            .children(console_open.then(|| self.render_console(cx)))
            .child(self.render_bottom_bar(cx))
            .children(create_dialog)
    }
}

fn toolbar_separator() -> impl IntoElement {
    div().w(px(1.)).h(px(24.)).mx_1().bg(rgb(0x444444))
}
