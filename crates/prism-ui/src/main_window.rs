//! Main window: the launcher shell.
//!
//! Layout mirrors PrismLauncher's `MainWindow.ui`:
//! - top main toolbar (Add Instance | Folders, Settings, Help, Check Updates
//!   | Accounts)
//! - central instance view (grouped icon grid) with a right-side instance
//!   toolbar (name, Launch, Kill, Edit, ...)
//! - bottom news toolbar and status bar.

use crate::state::{format_play_time, App};
use gpui_kit::base::{Disableable, InteractiveElementExt};
use gpui_kit::component::button::Button;
use gpui_kit::*;

pub struct MainWindow {
    app: gpui::Entity<App>,
}

impl MainWindow {
    pub fn new(app: gpui::Entity<App>) -> Self {
        MainWindow { app }
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let app = self.app.clone();
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
                    .on_click(move |_, _, _cx| {
                        // TODO: open the NewInstanceDialog page flow.
                        let _ = app.clone();
                    }),
            )
            .child(toolbar_separator())
            .child(Button::new("folders").label("Folders"))
            .child(Button::new("settings").label("Settings"))
            .child(Button::new("help").label("Help"))
            .child(Button::new("check-updates").label("Check for Updates"))
            .child(div().flex_1())
            .child(Button::new("accounts").label("Accounts"))
    }

    fn render_instance_grid(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let app = self.app.clone();
        let instances: Vec<(String, String, Option<String>)> = {
            let app = app.read(cx);
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
                .child("Add an instance to get started");
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
            let mut grid = div()
                .flex()
                .flex_wrap()
                .gap_2()
                .w_full();
            for idx in indices {
                let (id, name, _) = instances[idx].clone();
                grid = grid.child(self.render_instance_tile(id, name, cx));
            }
            container = container.child(grid);
        }
        container
    }

    fn render_instance_tile(&self, id: String, name: String, cx: &mut Context<Self>) -> impl IntoElement {
        let app = self.app.clone();
        let selected = {
            let app = app.read(cx);
            app.selected.as_deref() == Some(id.as_str())
        };
        let tile_bg = if selected { rgb(0x3d5a80) } else { rgb(0x2b2b2b) };

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
                this.app.update(cx, |app, _| {
                    app.selected = if app.selected.as_deref() == Some(id) {
                        None
                    } else {
                        Some(id.to_string())
                    };
                });
                cx.notify();
            }))
            .on_double_click(cx.listener(move |this, _event, _window, cx| {
                // TODO: launch the instance.
                cx.notify();
                let _ = &this.app;
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
                    .child("🟩"),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .max_w(px(100.))
                    .text_ellipsis()
                    .child(name.to_string()),
            )
    }

    fn render_instance_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.app.read(cx).selected.clone();
        let name = selected
            .as_deref()
            .and_then(|id| self.app.read(cx).instances.get(id))
            .map(|i| i.name.clone())
            .unwrap_or_default();
        let enabled = selected.is_some();

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
                    .child(if name.is_empty() { "No selection" } else { &name }.to_string()),
            )
            .child(toolbar_separator())
            .child(self.sidebar_button("rename", "Rename", enabled))
            .child(self.sidebar_button("change-icon", "Change Icon", enabled))
            .child(toolbar_separator())
            .child(self.sidebar_button("launch", "Launch", enabled))
            .child(self.sidebar_button("kill", "Kill", enabled))
            .child(toolbar_separator())
            .child(self.sidebar_button("edit", "Edit", enabled))
            .child(self.sidebar_button("change-group", "Change Group", enabled))
            .child(self.sidebar_button("view-folder", "View Folder", enabled))
            .child(self.sidebar_button("export", "Export", enabled))
            .child(self.sidebar_button("copy", "Copy", enabled))
            .child(self.sidebar_button("delete", "Delete", enabled))
            .child(self.sidebar_button("shortcut", "Create Shortcut", enabled))
    }

    fn sidebar_button(&self, id: &str, label: &str, enabled: bool) -> impl IntoElement {
        let mut btn = Button::new(SharedString::from(format!("sb-{id}"))).label(label);
        if !enabled {
            btn = btn.disabled(true);
        }
        btn
    }

    fn render_bottom_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let description = self.app.read(cx).status_description();
        let total_played: u64 = self.app.read(cx).instances.instances.iter().map(|i| i.total_time_played).sum();

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
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0x232323))
            .text_color(rgb(0xdddddd))
            .font_family("Segoe UI")
            .child(self.render_toolbar(cx))
            .child(
                div()
                    .flex_1()
                    .flex()
                    .min_h_0()
                    .child(self.render_instance_grid(cx))
                    .child(self.render_instance_sidebar(cx)),
            )
            .child(self.render_bottom_bar(cx))
    }
}

fn toolbar_separator() -> impl IntoElement {
    div().w(px(1.)).h(px(24.)).mx_1().bg(rgb(0x444444))
}
