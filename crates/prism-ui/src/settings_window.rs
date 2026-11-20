//! Global launcher preferences persisted in `prismlauncher.cfg`.

use crate::state::App;
use gpui_kit::base::Disableable;
use gpui_kit::component::button::Button;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use prism_core::horizon::LAUNCHER_PATH_SETTING;
use prism_core::net::ProxyMode;
use prism_core::settings::{ApiKey, Settings};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    General,
    Proxy,
    ApiKeys,
    Tools,
}

pub struct SettingsWindow {
    pub app: gpui::Entity<App>,
    min_mem: gpui::Entity<InputState>,
    max_mem: gpui::Entity<InputState>,
    java_path: gpui::Entity<InputState>,
    jvm_args: gpui::Entity<InputState>,
    horizon_launcher_path: gpui::Entity<InputState>,
    win_width: gpui::Entity<InputState>,
    win_height: gpui::Entity<InputState>,
    page: Page,
    proxy_mode: ProxyMode,
    proxy_address: gpui::Entity<InputState>,
    proxy_port: gpui::Entity<InputState>,
    proxy_user: gpui::Entity<InputState>,
    proxy_password: gpui::Entity<InputState>,
    curseforge_key: gpui::Entity<InputState>,
    imgur_id: gpui::Entity<InputState>,
    detected_java: String,
    status: String,
    tools: Entity<crate::external_tools_page::ExternalToolsPage>,
}

impl SettingsWindow {
    pub fn new(app: gpui::Entity<App>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let config_path = app.read(cx).paths.global_config();
        let cfg = Settings::load(&config_path);

        let mk = |placeholder: &str, value: String, window: &mut Window, cx: &mut Context<Self>| {
            let state = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
            state.update(cx, |s, cx| {
                s.set_value(value.clone(), window, cx);
            });
            state
        };
        let masked = |value: String, window: &mut Window, cx: &mut Context<Self>| {
            let state = mk("(optional)", value, window, cx);
            state.update(cx, |input, cx| input.set_masked(true, window, cx));
            state
        };

        SettingsWindow {
            tools: cx
                .new(|cx| crate::external_tools_page::ExternalToolsPage::new(&cfg, window, cx)),
            app,
            min_mem: mk("512", cfg.get_or("MinMemAlloc", "512"), window, cx),
            max_mem: mk("4096", cfg.get_or("MaxMemAlloc", "4096"), window, cx),
            java_path: mk("(auto-detect)", cfg.get_or("JavaPath", ""), window, cx),
            jvm_args: mk(
                "e.g. -Dexample=value",
                cfg.get_or("JvmArgs", ""),
                window,
                cx,
            ),
            horizon_launcher_path: mk(
                "(find on PATH)",
                cfg.get_or(LAUNCHER_PATH_SETTING, ""),
                window,
                cx,
            ),
            win_width: mk("854", cfg.get_or("MinecraftWinWidth", "854"), window, cx),
            win_height: mk("480", cfg.get_or("MinecraftWinHeight", "480"), window, cx),
            page: Page::General,
            proxy_mode: ProxyMode::from_settings(&cfg),
            proxy_address: mk(
                "127.0.0.1",
                cfg.get("ProxyAddr")
                    .or_else(|| cfg.get("ProxyHostName"))
                    .unwrap_or("127.0.0.1")
                    .to_owned(),
                window,
                cx,
            ),
            proxy_port: mk("8080", cfg.get_or("ProxyPort", "8080"), window, cx),
            proxy_user: masked(cfg.get_or("ProxyUser", ""), window, cx),
            proxy_password: masked(cfg.get_or("ProxyPass", ""), window, cx),
            curseforge_key: masked(cfg.get_or(ApiKey::CurseForge.setting(), ""), window, cx),
            imgur_id: masked(cfg.get_or(ApiKey::Imgur.setting(), ""), window, cx),
            detected_java: prism_core::java::find_java()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "not found".to_string()),
            status: String::new(),
        }
    }

    fn apply(&mut self, _ev: &ClickEvent, _window: &mut Window, cx: &mut Context<Self>) {
        self.status = match self.save(cx) {
            Ok(()) => "Settings saved.".into(),
            Err(error) => error,
        };
        cx.notify();
    }

    fn save(&self, cx: &mut Context<Self>) -> Result<(), String> {
        let config_path = self.app.read(cx).paths.global_config();
        let mut cfg = Settings::load(&config_path);
        let value = |input: &Entity<InputState>| input.read(cx).value().trim().to_owned();
        match self.page {
            Page::Tools => self.tools.read(cx).save(&mut cfg, cx),
            Page::General => {
                let args = value(&self.jvm_args);
                prism_core::launch::split_args(&args)
                    .map_err(|err| format!("Invalid JVM arguments: {err}"))?;
                for (key, field) in [
                    ("MinMemAlloc", &self.min_mem),
                    ("MaxMemAlloc", &self.max_mem),
                    ("JavaPath", &self.java_path),
                    ("MinecraftWinWidth", &self.win_width),
                    ("MinecraftWinHeight", &self.win_height),
                    (LAUNCHER_PATH_SETTING, &self.horizon_launcher_path),
                ] {
                    cfg.set(key, value(field));
                }
                cfg.set("JvmArgs", args);
            }
            Page::Proxy => {
                cfg.set("ProxyType", self.proxy_mode.config_value());
                for (key, field) in [
                    ("ProxyAddr", &self.proxy_address),
                    ("ProxyPort", &self.proxy_port),
                    ("ProxyUser", &self.proxy_user),
                ] {
                    cfg.set(key, value(field));
                }
                let password = self.proxy_password.read(cx).value();
                if password.is_empty() {
                    cfg.ini.remove("ProxyPass");
                } else {
                    cfg.set("ProxyPass", password.to_string());
                }
                return self
                    .app
                    .read(cx)
                    .runtime
                    .downloader
                    .update_proxy(&cfg, &config_path);
            }
            Page::ApiKeys => {
                for (key, label, field) in [
                    (
                        ApiKey::CurseForge,
                        "CurseForge API key",
                        &self.curseforge_key,
                    ),
                    (ApiKey::Imgur, "Imgur client ID", &self.imgur_id),
                ] {
                    let secret = value(field);
                    if secret.is_empty() {
                        cfg.ini.remove(key.setting());
                    } else if key.is_valid(&secret) {
                        cfg.set(key.setting(), secret);
                    } else {
                        return Err(format!("Enter a valid {label}."));
                    }
                }
            }
        }
        cfg.save(&config_path)
            .map_err(|error| format!("Save failed: {error}"))?;
        if self.page == Page::ApiKeys {
            let configured = ApiKey::CurseForge.configured(&cfg).is_some();
            self.app.update(cx, |app, cx| {
                app.curseforge_configured = configured;
                cx.notify();
            });
        }
        Ok(())
    }
}

impl Render for SettingsWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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

        let label = |text: &'static str| {
            div()
                .text_size(px(12.))
                .text_color(rgb(0x8fb4d9))
                .child(text)
        };
        let body = match self.page {
            Page::Tools => div().child(self.tools.clone()),
            Page::General => div()
                .flex()
                .flex_col()
                .gap_2()
                .child(label("Minecraft"))
                .child(row("Minimum memory (MB)", &self.min_mem))
                .child(row("Maximum memory (MB)", &self.max_mem))
                .child(row("Window width", &self.win_width))
                .child(row("Window height", &self.win_height))
                .child(label("Java"))
                .child(row("Java path", &self.java_path))
                .child(label("Detected Java").child(format!(": {}", self.detected_java)))
                .child(row("Global JVM arguments", &self.jvm_args))
                .child(label("Used unless an instance overrides Java arguments."))
                .child(label("Horizon"))
                .child(row("Horizon launcher path", &self.horizon_launcher_path)),
            Page::Proxy => {
                let manual = matches!(self.proxy_mode, ProxyMode::Http | ProxyMode::Socks5);
                let choices = [
                    (ProxyMode::System, "System"),
                    (ProxyMode::Direct, "No proxy"),
                    (ProxyMode::Http, "HTTP"),
                    (ProxyMode::Socks5, "SOCKS5"),
                ];
                div().flex().flex_col().gap_3()
                    .child(label("Launcher HTTP proxy"))
                    .child(div().flex().gap_2().children(choices.into_iter().map(|(mode, name)| {
                        Button::new(name).label(if self.proxy_mode == mode { format!("✓ {name}") } else { name.into() })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.proxy_mode = mode;
                                this.status.clear();
                                cx.notify();
                            }))
                    })))
                    .when(manual, |body| body
                        .child(row("Proxy address", &self.proxy_address))
                        .child(row("Port", &self.proxy_port))
                        .child(row("Username (optional)", &self.proxy_user))
                        .child(row("Password (optional)", &self.proxy_password)))
                    .child(label("Applies to new launcher requests. Minecraft uses its own network settings."))
            }
            Page::ApiKeys => {
                let hint = |key: ApiKey| {
                    let mode = if key.has_environment_override() {
                        "Overrides the saved value"
                    } else {
                        "Optional environment override"
                    };
                    div()
                        .text_size(px(12.))
                        .text_color(rgb(0x8fb4d9))
                        .child(format!("{mode}: {}", key.environment()))
                };
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(row("CurseForge API key", &self.curseforge_key))
                    .child(hint(ApiKey::CurseForge))
                    .child(row("Imgur client ID", &self.imgur_id))
                    .child(hint(ApiKey::Imgur))
                    .child(label("Clear a field and apply to remove its saved value."))
            }
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
            .child(
                div().flex().gap_2().children(
                    [
                        (Page::General, "General & Java"),
                        (Page::Proxy, "Proxy"),
                        (Page::ApiKeys, "API Keys"),
                        (Page::Tools, "External Tools"),
                    ]
                    .into_iter()
                    .map(|(page, name)| {
                        Button::new(name)
                            .label(name)
                            .disabled(self.page == page)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.page = page;
                                this.status.clear();
                                cx.notify();
                            }))
                    }),
                ),
            )
            .child(
                div()
                    .id("settings-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(body),
            )
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
