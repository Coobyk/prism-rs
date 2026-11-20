//! Global launcher preferences persisted in `prismlauncher.cfg`.

use crate::i18n::Locale;
use crate::state::App;
use crate::theme::Button;
use gpui_kit::base::{Disableable, Selectable};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::select::{Select, SelectDelegate, SelectEvent, SelectItem, SelectState};
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
    Appearance,
    Tools,
    Language,
}

#[derive(Clone)]
struct ThemeChoice {
    id: String,
    name: String,
}

impl SelectItem for ThemeChoice {
    type Value = String;
    fn title(&self) -> SharedString {
        self.name.clone().into()
    }
    fn value(&self) -> &String {
        &self.id
    }
}

type ThemeSelect = SelectState<Vec<ThemeChoice>>;
type LanguageSelect = SelectState<Vec<Locale>>;

impl SelectItem for Locale {
    type Value = Locale;

    fn title(&self) -> SharedString {
        self.native_name().into()
    }

    fn value(&self) -> &Locale {
        self
    }
}

fn select_row<D: SelectDelegate + 'static>(
    label: &'static str,
    state: &Entity<SelectState<D>>,
    palette: crate::theme::Palette,
) -> impl IntoElement
where
    <D::Item as SelectItem>::Value: PartialEq + Clone,
{
    div()
        .flex()
        .items_center()
        .gap_3()
        .child(
            div()
                .w(px(220.))
                .flex_shrink_0()
                .text_size(px(13.))
                .text_color(palette.secondary_text)
                .child(label),
        )
        .child(
            div().w(px(280.)).child(
                Select::new(state)
                    .w_full()
                    .menu_width(px(280.))
                    .accessibility_label(label),
            ),
        )
}

fn choices(cx: &gpui::App, icons: bool) -> Vec<ThemeChoice> {
    let catalog = crate::theme::catalog(cx);
    if icons {
        std::iter::once(ThemeChoice {
            id: "default".into(),
            name: "Default".into(),
        })
        .chain(catalog.icons.iter().map(|t| ThemeChoice {
            id: t.id.clone(),
            name: t.name.clone(),
        }))
        .collect()
    } else {
        catalog
            .applications
            .iter()
            .map(|t| ThemeChoice {
                id: t.id.clone(),
                name: t.name.clone(),
            })
            .collect()
    }
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
    themes: Entity<ThemeSelect>,
    icon_themes: Entity<ThemeSelect>,
    languages: Entity<LanguageSelect>,
    tools: Entity<crate::external_tools_page::ExternalToolsPage>,
}

impl SettingsWindow {
    pub fn new(app: gpui::Entity<App>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let config_path = app.read(cx).paths.global_config();
        let cfg = Settings::load(&config_path);
        let locale = app.read(cx).locale;
        let mut previous_locale = locale;
        window.set_window_title(locale.text("Settings"));
        cx.observe_in(&app, window, move |this, app, window, cx| {
            let locale = app.read(cx).locale;
            if locale == previous_locale {
                return;
            }
            previous_locale = locale;
            window.set_window_title(locale.text("Settings"));
            this.languages
                .update(cx, |state, cx| state.set_selected_value(&locale, window, cx));
            for (input, source) in [
                (&this.java_path, "(auto-detect)"),
                (&this.jvm_args, "e.g. -Dexample=value"),
                (&this.horizon_launcher_path, "(find on PATH)"),
                (&this.proxy_user, "(optional)"),
                (&this.proxy_password, "(optional)"),
                (&this.curseforge_key, "(optional)"),
                (&this.imgur_id, "(optional)"),
            ] {
                input.update(cx, |input, cx| {
                    input.set_placeholder(locale.text(source), window, cx);
                });
            }
            this.status.clear();
            cx.notify();
        })
        .detach();

        let languages = cx.new(|cx| SelectState::new(Locale::ALL.to_vec(), None, window, cx));
        languages.update(cx, |state, cx| state.set_selected_value(&locale, window, cx));
        cx.subscribe_in(
            &languages,
            window,
            |this, _, event: &SelectEvent<Vec<Locale>>, window, cx| {
                if let SelectEvent::Confirm(Some(locale)) = event {
                    this.select_language(*locale, window, cx);
                }
            },
        )
        .detach();

        let themes = cx.new(|cx| SelectState::new(choices(cx, false), None, window, cx));
        let icon_themes = cx.new(|cx| SelectState::new(choices(cx, true), None, window, cx));
        for (state, icons) in [(&themes, false), (&icon_themes, true)] {
            let (theme, icon) = crate::theme::selected(cx);
            let id = if icons { icon } else { theme }.to_owned();
            state.update(cx, |state, cx| state.set_selected_value(&id, window, cx));
            cx.subscribe_in(
                state,
                window,
                move |this, _, event: &SelectEvent<Vec<ThemeChoice>>, window, cx| {
                    if let SelectEvent::Confirm(Some(id)) = event {
                        let paths = this.app.read(cx).paths.clone();
                        let key = if icons {
                            "IconTheme"
                        } else {
                            "ApplicationTheme"
                        };
                        this.status = match crate::theme::persist(&paths, key, id) {
                            Ok(()) => {
                                if icons {
                                    crate::theme::select_icons(id, cx);
                                } else {
                                    crate::theme::select(id, window.appearance(), cx);
                                }
                                this.app.read(cx).locale.text("Appearance saved.").into()
                            }
                            Err(error) => {
                                let (theme, icon) = crate::theme::selected(cx);
                                let id = if icons { icon } else { theme }.to_owned();
                                let state = if icons {
                                    &this.icon_themes
                                } else {
                                    &this.themes
                                };
                                state.update(cx, |state, cx| {
                                    state.set_selected_value(&id, window, cx)
                                });
                                error
                            }
                        };
                        cx.notify();
                    }
                },
            )
            .detach();
        }

        let mk = |placeholder, value: String, window: &mut Window, cx: &mut Context<Self>| {
            let state = cx.new(|cx| InputState::new(window, cx).placeholder(locale.text(placeholder)));
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
                .new(|cx| crate::external_tools_page::ExternalToolsPage::new(app.clone(), &cfg, window, cx)),
            app,
            themes,
            icon_themes,
            languages,
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
                .unwrap_or_default(),
            status: String::new(),
        }
    }

    fn apply(&mut self, _ev: &ClickEvent, _window: &mut Window, cx: &mut Context<Self>) {
        self.status = match self.save(cx) {
            Ok(()) => self.app.read(cx).locale.text("Settings saved.").into(),
            Err(error) => error,
        };
        cx.notify();
    }

    fn save(&self, cx: &mut Context<Self>) -> Result<(), String> {
        let config_path = self.app.read(cx).paths.global_config();
        let locale = self.app.read(cx).locale;
        let mut cfg = Settings::load(&config_path);
        let value = |input: &Entity<InputState>| input.read(cx).value().trim().to_owned();
        match self.page {
            Page::Appearance | Page::Language => return Ok(()),
            Page::Tools => self.tools.read(cx).save(&mut cfg, cx),
            Page::General => {
                let args = value(&self.jvm_args);
                prism_core::launch::split_args(&args)
                    .map_err(|err| locale.format("Invalid JVM arguments: {0}", &[&err.to_string()]))?;
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
                        return Err(locale.format("Enter a valid {0}.", &[locale.text(label)]));
                    }
                }
            }
        }
        cfg.save(&config_path)
            .map_err(|error| locale.format("Save failed: {0}", &[&error.to_string()]))?;
        if self.page == Page::ApiKeys {
            let configured = ApiKey::CurseForge.configured(&cfg).is_some();
            self.app.update(cx, |app, cx| {
                app.curseforge_configured = configured;
                cx.notify();
            });
        }
        Ok(())
    }

    fn select_language(&mut self, locale: Locale, window: &mut Window, cx: &mut Context<Self>) {
        let path = self.app.read(cx).paths.global_config();
        self.status = match locale.save(&path) {
            Ok(()) => {
                self.app.update(cx, |app, cx| {
                    app.locale = locale;
                    cx.notify();
                });
                String::new()
            }
            Err(error) => {
                let previous_locale = self.app.read(cx).locale;
                self.languages.update(cx, |state, cx| {
                    state.set_selected_value(&previous_locale, window, cx);
                });
                previous_locale.format("Save failed: {0}", &[&error.to_string()])
            }
        };
        cx.notify();
    }

    fn reload_themes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = self.app.read(cx).paths.clone();
        self.status = crate::theme::reload(&paths, window.appearance(), cx).unwrap_or_else(|e| e);
        for (state, icons) in [(&self.themes, false), (&self.icon_themes, true)] {
            let items = choices(cx, icons);
            let (theme, icon) = crate::theme::selected(cx);
            let id = if icons { icon } else { theme }.to_owned();
            state.update(cx, |state, cx| {
                state.set_items(items, window, cx);
                state.set_selected_value(&id, window, cx);
            });
        }
        cx.notify();
    }

    fn open_theme_folder(&mut self, icons: bool, cx: &mut Context<Self>) {
        let paths = &self.app.read(cx).paths;
        let path = if icons {
            paths.icon_themes_dir()
        } else {
            paths.themes_dir()
        };
        self.status = match std::fs::create_dir_all(&path)
            .and_then(|_| crate::instance_window::datapacks_page::open_folder(&path))
        {
            Ok(()) => String::new(),
            Err(e) => self.app.read(cx).locale.format("Could not open folder: {0}", &[&e.to_string()]),
        };
        cx.notify();
    }
}

impl Render for SettingsWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = crate::theme::Palette::get(cx);
        let locale = self.app.read(cx).locale;
        let t = |source| locale.text(source);
        let row = |label: &'static str, input: &gpui::Entity<InputState>| {
            div()
                .flex()
                .items_center()
                .gap_3()
                .child(
                    div()
                        .w(px(220.))
                        .text_size(px(13.))
                        .text_color(palette.secondary_text)
                        .child(t(label)),
                )
                .child(div().w(px(280.)).child(Input::new(input)))
        };

        let label = |text: &'static str| {
            div()
                .text_size(px(12.))
                .text_color(palette.link)
                .child(t(text))
        };
        let body = match self.page {
            Page::Language => div()
                .flex()
                .flex_col()
                .gap_2()
                .child(select_row(t("Launcher language"), &self.languages, palette))
                .child(div().text_size(px(12.)).text_color(palette.secondary_text).child(
                    t("Language changes apply and save immediately."),
                )),
            Page::Appearance => {
                let folders = div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .mt_2()
                    .child(
                        Button::new("themes-folder")
                            .label(t("Open Themes Folder"))
                            .on_click(
                                cx.listener(|this, _, _, cx| this.open_theme_folder(false, cx)),
                            ),
                    )
                    .child(
                        Button::new("icon-themes-folder")
                            .label(t("Open Icon Themes Folder"))
                            .on_click(
                                cx.listener(|this, _, _, cx| this.open_theme_folder(true, cx)),
                            ),
                    )
                    .child(Button::new("reload-themes").label(t("Reload")).on_click(
                        cx.listener(|this, _, window, cx| this.reload_themes(window, cx)),
                    ));
                let hint = locale.format(
                    "Custom themes: {0}/<name>/theme.json (PrismLauncher format).",
                    &[&self.app.read(cx).paths.themes_dir().display().to_string()],
                );
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(select_row(t("Theme"), &self.themes, palette))
                    .child(select_row(t("Icon theme"), &self.icon_themes, palette))
                    .child(folders)
                    .child(div().text_size(px(12.)).text_color(palette.secondary_text).child(hint))
                    .child(
                        div().text_size(px(12.)).text_color(palette.muted_text).child(
                            t("Changes apply and save immediately. Qt stylesheets (QSS) are not supported."),
                        ),
                    )
                    .children(crate::theme::catalog(cx).warnings.iter().map(|warning| {
                        div().text_size(px(12.)).text_color(palette.warning).child(warning.clone())
                    }))
            }
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
                .child(label("Detected Java").child(format!(": {}", if self.detected_java.is_empty() {
                    t("not found")
                } else {
                    &self.detected_java
                })))
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
                    .child(div().flex().flex_wrap().gap_2().children(choices.into_iter().map(|(mode, name)| {
                        Button::new(name).label(t(name)).selected(self.proxy_mode == mode)
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
                        .text_color(palette.link)
                        .child(format!("{}: {}", t(mode), key.environment()))
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
            .bg(palette.window)
            .text_color(palette.text)
            .p_4()
            .gap_3()
            .child(
                div()
                    .text_size(px(15.))
                    .font_weight(FontWeight::BOLD)
                    .child(t("Settings")),
            )
            .child(div().h(px(1.)).bg(palette.border))
            .child(
                div().flex().flex_wrap().gap_2().children(
                    [
                        (Page::General, "General & Java"),
                        (Page::Proxy, "Proxy"),
                        (Page::ApiKeys, "API Keys"),
                        (Page::Appearance, "Appearance"),
                        (Page::Tools, "External Tools"),
                        (Page::Language, "Language"),
                    ]
                    .into_iter()
                    .map(|(page, name)| {
                        Button::new(name)
                            .label(t(name))
                            .selected(self.page == page)
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
                            .label(t("Apply"))
                            .disabled(matches!(self.page, Page::Appearance | Page::Language))
                            .on_click(cx.listener(Self::apply)),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(palette.link)
                            .child(self.status.clone()),
                    ),
            )
    }
}
