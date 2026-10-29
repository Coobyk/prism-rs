//! Global settings window: general/Minecraft/Java preferences persisted in
//! `prismlauncher.cfg`. Mirrors the LauncherPage/MinecraftPage/JavaPage.

use crate::state::App;
use gpui_kit::base::Disableable;
use gpui_kit::component::button::Button;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::*;

pub struct SettingsWindow {
    pub app: gpui::Entity<App>,
    min_mem: gpui::Entity<InputState>,
    max_mem: gpui::Entity<InputState>,
    java_path: gpui::Entity<InputState>,
    win_width: gpui::Entity<InputState>,
    win_height: gpui::Entity<InputState>,
    status: String,
}

impl SettingsWindow {
    pub fn new(app: gpui::Entity<App>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let config_path = app.read(cx).paths.global_config();
        let cfg = prism_core::settings::Settings::load(&config_path);

        let mk = |placeholder: &str, value: String, window: &mut Window, cx: &mut Context<Self>| {
            let state = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
            state.update(cx, |s, cx| {
                s.set_value(value.clone(), window, cx);
            });
            state
        };

        SettingsWindow {
            app,
            min_mem: mk("512", cfg.get_or("MinMemAlloc", "512"), window, cx),
            max_mem: mk("4096", cfg.get_or("MaxMemAlloc", "4096"), window, cx),
            java_path: mk("(auto-detect)", cfg.get_or("JavaPath", ""), window, cx),
            win_width: mk("854", cfg.get_or("MinecraftWinWidth", "854"), window, cx),
            win_height: mk("480", cfg.get_or("MinecraftWinHeight", "480"), window, cx),
            status: String::new(),
        }
    }

    fn apply(&mut self, _ev: &ClickEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let values = (
            self.min_mem.read(cx).value().trim().to_string(),
            self.max_mem.read(cx).value().trim().to_string(),
            self.java_path.read(cx).value().trim().to_string(),
            self.win_width.read(cx).value().trim().to_string(),
            self.win_height.read(cx).value().trim().to_string(),
        );
        let config_path = self.app.read(cx).paths.global_config();
        let mut cfg = prism_core::settings::Settings::load(&config_path);
        cfg.set("MinMemAlloc", values.0);
        cfg.set("MaxMemAlloc", values.1);
        cfg.set("JavaPath", values.2);
        cfg.set("MinecraftWinWidth", values.3);
        cfg.set("MinecraftWinHeight", values.4);
        match cfg.save(&config_path) {
            Ok(()) => self.status = "Settings saved.".to_string(),
            Err(e) => self.status = format!("Save failed: {e}"),
        }
        cx.notify();
    }
}

impl Render for SettingsWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let detected_java = prism_core::java::find_java()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "not found".to_string());

        let row = |label: &str, input: &gpui::Entity<InputState>| {
            div()
                .flex()
                .items_center()
                .gap_3()
                .child(
                    div()
                        .w(px(220.))
                        .text_size(px(13.))
                        .text_color(rgb(0xaaaaaa))
                        .child(label.to_string()),
                )
                .child(div().w(px(280.)).child(Input::new(input)))
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0x232323))
            .text_color(rgb(0xdddddd))
            .p_4()
            .gap_3()
            .child(
                div()
                    .text_size(px(15.))
                    .font_weight(FontWeight::BOLD)
                    .child("Settings"),
            )
            .child(div().h(px(1.)).bg(rgb(0x1c1c1c)))
            .child(div().text_size(px(12.)).text_color(rgb(0x8fb4d9)).child("Minecraft"))
            .child(row("Minimum memory (MB)", &self.min_mem))
            .child(row("Maximum memory (MB)", &self.max_mem))
            .child(row("Window width", &self.win_width))
            .child(row("Window height", &self.win_height))
            .child(div().text_size(px(12.)).text_color(rgb(0x8fb4d9)).child("Java"))
            .child(row("Java path", &self.java_path))
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(rgb(0x777777))
                    .child(format!("Auto-detected: {detected_java}")),
            )
            .child(div().flex_1())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        Button::new("apply-settings")
                            .label("Apply")
                            .on_click(cx.listener(Self::apply)),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(0x8fb4d9))
                            .child(self.status.clone()),
                    ),
            )
    }
}
