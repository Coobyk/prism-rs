//! Dialogs: create instance (mirrors NewInstanceDialog).

use crate::state::App;
use crate::main_window::MainWindow;
use gpui_kit::base::{Disableable, Selectable};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use prism_core::launch::QuickPlayTarget;

pub struct QuickPlayState {
    target_id: String,
    name: String,
    worlds: Vec<String>,
    selected: Option<String>,
    multiplayer: bool,
    input: Entity<InputState>,
    error: Option<String>,
}

impl QuickPlayState {
    pub fn new(
        inst: &prism_core::Instance,
        window: &mut Window,
        cx: &mut Context<MainWindow>,
    ) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("mc.example.com:25565"));
        cx.subscribe(&input, |_, _, _: &InputEvent, cx| cx.notify())
            .detach();
        Self {
            target_id: inst.id.clone(),
            name: inst.name.clone(),
            worlds: inst.worlds(),
            selected: None,
            multiplayer: false,
            error: None,
            input,
        }
    }

    pub fn render(&self, cx: &mut Context<MainWindow>) -> impl IntoElement {
        let mut panel = div()
            .id("quick-play-dialog")
            .w(px(440.))
            .bg(rgb(0x2b2b2b))
            .rounded_lg()
            .border_1()
            .border_color(rgb(0x444444))
            .p_4()
            .flex()
            .flex_col()
            .gap_3()
            .on_click(|_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .text_size(px(16.))
                    .font_weight(FontWeight::BOLD)
                    .child(format!("Quick Play — {}", self.name)),
            );
        let mut tabs = div()
            .flex()
            .p_1()
            .bg(rgb(0x242424))
            .rounded_md();
        for (multiplayer, label) in [(false, "Singleplayer"), (true, "Multiplayer")] {
            tabs = tabs.child(
                div()
                    .id(label)
                    .flex_1()
                    .py_2()
                    .rounded_sm()
                    .text_center()
                    .bg(if multiplayer == self.multiplayer {
                        rgb(0x3d5a80)
                    } else {
                        rgba(0x00000000)
                    })
                    .hover(|style| style.bg(rgb(0x333333)))
                    .child(label)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if let Some(dialog) = &mut this.quick_play {
                            dialog.multiplayer = multiplayer;
                            dialog.error = None;
                        }
                        if multiplayer {
                            if let Some(dialog) = &this.quick_play {
                                window.focus(&dialog.input.read(cx).focus_handle(cx), cx);
                            }
                        }
                        cx.notify();
                    })),
            );
        }
        panel = panel.child(tabs);
        if self.multiplayer {
            panel = panel
                .child(div().child("Server address"))
                .child(Input::new(&self.input));
        } else {
            let mut list = div()
                .id("quick-worlds")
                .max_h(px(230.))
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap_1();
            if self.worlds.is_empty() {
                list = list.child(
                    div()
                        .text_color(rgb(0x777777))
                        .child("No worlds in this instance yet."),
                );
            }
            for (index, world) in self.worlds.iter().enumerate() {
                let world = world.clone();
                list = list.child(
                    div()
                        .id(("quick-world", index))
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .bg(rgb(if self.selected.as_ref() == Some(&world) {
                            0x3d5a80
                        } else {
                            0x333333
                        }))
                        .child(world.clone())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(dialog) = &mut this.quick_play {
                                dialog.selected = Some(world.clone());
                                dialog.error = None;
                            }
                            cx.notify();
                        })),
                );
            }
            panel = panel.child(list);
        }
        if let Some(error) = &self.error {
            panel = panel.child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(0xff6666))
                    .child(error.clone()),
            );
        }
        panel = panel.child(
            div()
                .flex()
                .justify_end()
                .gap_2()
                .child(
                    Button::new("quick-cancel")
                        .label("Cancel")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.quick_play = None;
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("quick-join")
                        .disabled(if self.multiplayer {
                            self.input.read(cx).value().trim().is_empty()
                        } else {
                            self.selected.is_none()
                        })
                        .label(if self.multiplayer {
                            "Join Server"
                        } else {
                            "Join World"
                        })
                        .on_click(cx.listener(|this, _, _, cx| {
                            let Some(dialog) = &this.quick_play else {
                                return;
                            };
                            let target = if dialog.multiplayer {
                                QuickPlayTarget::parse_server(
                                    dialog.input.read(cx).value().as_ref(),
                                )
                            } else {
                                dialog
                                    .selected
                                    .clone()
                                    .map(|world| QuickPlayTarget::Singleplayer { world })
                                    .ok_or("Select a world".into())
                            };
                            match target {
                                Ok(target) => {
                                    let id = dialog.target_id.clone();
                                    this.quick_play = None;
                                    this.console_open = true;
                                    this.app.update(cx, |app, cx| {
                                        app.start_launch_with(&id, Some(target), cx)
                                    });
                                }
                                Err(error) => this.quick_play.as_mut().unwrap().error = Some(error),
                            }
                            cx.notify();
                        })),
                ),
        );
        div()
            .id("quick-backdrop")
            .absolute()
            .size_full()
            .top_0()
            .left_0()
            .bg(gpui_kit::black().opacity(0.5))
            .flex()
            .items_center()
            .justify_center()
            .on_click(cx.listener(|this, _, _, cx| {
                this.quick_play = None;
                cx.notify();
            }))
            .child(panel)
    }
}

pub struct CreateInstanceState {
    pub name_input: Entity<InputState>,
    pub group_input: Entity<InputState>,
    pub version_filter: Entity<InputState>,
    pub selected_version: Option<String>,
    pub error: Option<String>,
    horizon: bool,
    creating: bool,
}

impl CreateInstanceState {
    pub fn new(window: &mut Window, cx: &mut Context<MainWindow>) -> Self {
        let name_input = cx.new(|cx| InputState::new(window, cx).placeholder("Instance name"));
        let group_input = cx.new(|cx| InputState::new(window, cx).placeholder("Group (optional)"));
        let version_filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter versions…"));
        CreateInstanceState {
            name_input,
            group_input,
            version_filter,
            selected_version: None,
            error: None,
            horizon: false,
            creating: false,
        }
    }

    pub fn render(
        &mut self,
        app: &Entity<App>,
        window: &mut Window,
        cx: &mut Context<MainWindow>,
    ) -> impl IntoElement {
        let manifest = app.read(cx).manifest.clone();
        let filter = self.version_filter.read(cx).value().to_lowercase();
        let selected = self.selected_version.clone();

        let mut version_rows: Vec<(String, String)> = Vec::new(); // (id, type)
        if let Some(manifest) = manifest.as_ref().filter(|_| !self.horizon) {
            for v in manifest.versions.iter().take(300) {
                if !filter.is_empty() && !v.id.to_lowercase().contains(&filter) {
                    continue;
                }
                version_rows.push((v.id.clone(), v.version_type.clone()));
            }
        }

        let backdrop = div()
            .id("create-dialog-backdrop")
            .absolute()
            .size_full()
            .top_0()
            .left_0()
            .bg(gpui_kit::black().opacity(0.5))
            .flex()
            .items_center()
            .justify_center()
            .on_click(cx.listener(|this: &mut MainWindow, _ev, _window, cx| {
                if this.create_dialog.as_ref().is_some_and(|d| !d.creating) {
                    this.create_dialog = None;
                    cx.notify();
                }
            }));

        let mut panel = div()
            .id("create-dialog")
            .w(px(560.))
            .h(px(520.))
            .bg(rgb(0x2b2b2b))
            .rounded_lg()
            .border_1()
            .border_color(rgb(0x444444))
            .p_4()
            .flex()
            .flex_col()
            .gap_3()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(div().text_size(px(16.)).font_weight(FontWeight::BOLD).child("Create Instance"))
            .child(
                div().flex().flex_col().gap_1().child("Name").child(Input::new(&self.name_input)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child("Group")
                    .child(Input::new(&self.group_input)),
            );

        let mut types = div().flex().items_center().gap_2().child("Game:");
        for (horizon, label) in [(false, "Minecraft: Java Edition"), (true, "Horizon")] {
            let selected = horizon == self.horizon;
            let button = Button::new(if horizon { "type-horizon" } else { "type-java" });
            types = types.child(
                (if selected { button.primary() } else { button })
                    .label(label)
                    .disabled(self.creating)
                    .on_click(cx.listener(move |this: &mut MainWindow, _ev, _window, cx| {
                        if let Some(dialog) = this.create_dialog.as_mut() {
                            if dialog.horizon != horizon {
                                dialog.horizon = horizon;
                                dialog.error = None;
                            }
                        }
                        cx.notify();
                    })),
            );
        }
        panel = panel.child(types);

        if self.horizon {
            panel = panel.child(
                div().flex_1().text_size(px(13.)).child(
                    "Creates a Horizon game folder through the official Horizon launcher. Install Horizon and sign in there before playing.",
                ),
            );
        } else {
            panel = panel.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(if app.read(cx).manifest_loading {
                        "Loading versions…".to_string()
                    } else {
                        format!(
                            "Minecraft Version{}",
                            selected
                                .clone()
                                .map(|s| format!(": {s}"))
                                .unwrap_or_default()
                        )
                    })
                    .child(Input::new(&self.version_filter)),
            );

            let mut list = div()
                .id("version-list")
                .flex_1()
                .flex()
                .flex_col()
                .gap_y_0p5()
                .overflow_y_scroll()
                .bg(rgb(0x242424))
                .rounded_md()
                .p_1();
            for (id, vtype) in &version_rows {
                let is_selected = selected.as_deref() == Some(id.as_str());
                let id2 = id.clone();
                list = list.child(
                    div()
                        .id(SharedString::from(format!("ver-{id}")))
                        .flex()
                        .justify_between()
                        .px_2()
                        .py_1()
                        .rounded_sm()
                        .text_size(px(12.))
                        .bg(if is_selected { rgb(0x3d5a80) } else { rgba(0x00000000) })
                        .hover(|s| s.bg(rgb(0x333333)))
                        .on_click(cx.listener(move |this: &mut MainWindow, _ev, _window, cx| {
                            if let Some(d) = this.create_dialog.as_mut() {
                                d.selected_version = Some(id2.clone());
                            }
                            cx.notify();
                        }))
                        .child(id.clone())
                        .child(
                            div()
                                .text_size(px(10.))
                                .text_color(rgb(0x888888))
                                .child(vtype.clone()),
                        ),
                );
            }
            panel = panel.child(list);
        }

        if let Some(e) = &self.error {
            panel = panel.child(div().text_size(px(12.)).text_color(rgb(0xff6666)).child(e.clone()));
        }

        panel = panel.child(
            div()
                .flex()
                .justify_end()
                .gap_2()
                .child(
                    gpui_kit::component::button::Button::new("cancel")
                        .label("Cancel")
                        .disabled(self.creating)
                        .on_click(cx.listener(|this: &mut MainWindow, _ev, _window, cx| {
                            this.create_dialog = None;
                            cx.notify();
                        })),
                )
                .child(
                    gpui_kit::component::button::Button::new("ok")
                        .label(if self.creating { "Creating…" } else { "OK" })
                        .disabled(self.creating)
                        .on_click(cx.listener(confirm_create)),
                ),
        );

        backdrop.child(panel)
    }
}

fn confirm_create(this: &mut MainWindow, _ev: &ClickEvent, _window: &mut Window, cx: &mut Context<MainWindow>) {
    let Some(dialog) = this.create_dialog.as_mut() else {
        return;
    };
    if dialog.creating {
        return;
    }
    let name = dialog.name_input.read(cx).value().trim().to_string();
    let group = dialog.group_input.read(cx).value().trim().to_string();
    let version = dialog.selected_version.clone();

    if name.is_empty() {
        dialog.error = Some("Please enter a name.".to_string());
        cx.notify();
        return;
    }
    if dialog.horizon {
        dialog.creating = true;
        dialog.error = None;
        let app = this.app.read(cx);
        let paths = app.paths.clone();
        let task = app.runtime.handle().spawn_blocking(move || {
            let settings = prism_core::settings::Settings::load(&paths.global_config());
            let launcher = prism_core::horizon::HorizonLauncher::from_settings(&settings);
            let horizon = launcher.create(&name).map_err(|e| e.to_string())?;
            prism_core::InstanceList::link_horizon(&paths, horizon).map_err(|e| {
                format!("Horizon created the game folder, but Prism could not save its link: {e}")
            })
        });
        cx.spawn(async move |this, cx| {
            let result = task.await.map_err(|e| e.to_string()).and_then(|result| result);
            let _ = this.update(cx, |this, cx| finish_create(this, &group, result, cx));
        })
        .detach();
        cx.notify();
        return;
    }
    let Some(version) = version else {
        dialog.error = Some("Please select a Minecraft version.".to_string());
        cx.notify();
        return;
    };

    let result = prism_core::InstanceList::create(&this.app.read(cx).paths, &name, "default", &version)
        .map_err(|e| e.to_string());
    finish_create(this, &group, result, cx);
}

fn finish_create(
    this: &mut MainWindow,
    group: &str,
    result: Result<prism_core::Instance, String>,
    cx: &mut Context<MainWindow>,
) {
    let inst = match result {
        Ok(inst) => inst,
        Err(e) => {
            if let Some(dialog) = this.create_dialog.as_mut() {
                dialog.creating = false;
                dialog.error = Some(format!("Failed to create instance: {e}"));
            }
            cx.notify();
            return;
        }
    };
    this.app.update(cx, |app, _cx| {
        if !group.is_empty() {
            app.instances.groups.set_group(&inst.id, Some(group));
            let _ = app.instances.groups.save(&app.paths.instances_dir());
        }
        app.reload_instances();
        app.selected = Some(inst.id);
    });
    this.create_dialog = None;
    cx.notify();
}

/// A simple one-field prompt: rename or change-group.
pub struct PromptState {
    pub kind: PromptKind,
    pub target_id: String,
    pub input: Entity<InputState>,
    pub error: Option<String>,
    pub export_fields: Option<[Entity<InputState>; 3]>,
    pub mrpack: bool,
    pub export_default_path: Option<String>,
    pub copy_group: Option<Entity<InputState>>,
    pub copy_flags: [bool; 8],
}

#[derive(Clone, Copy, PartialEq)]
pub enum PromptKind {
    Rename,
    ChangeGroup,
    ImportPath,
    Export,
    Copy,
}

impl PromptState {
    pub fn new(kind: PromptKind, target_id: String, initial: String, window: &mut Window, cx: &mut Context<MainWindow>) -> Self {
        let placeholder = match kind {
            PromptKind::Rename => "Instance name",
            PromptKind::ChangeGroup => "Group name (empty = none)",
            PromptKind::ImportPath => "Local path to .zip or .mrpack",
            PromptKind::Export => "Destination path (must not already exist)",
            PromptKind::Copy => "Instance name",
        };
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
        input.update(cx, |s, cx| {
            s.set_value(initial, window, cx);
        });
        PromptState {
            kind,
            target_id,
            input,
            error: None,
            export_fields: if kind == PromptKind::Export {
                Some(
                    ["Pack name", "Version (e.g. 1.0.0)", "Summary"]
                        .map(|label| cx.new(|cx| InputState::new(window, cx).placeholder(label))),
                )
            } else {
                None
            },
            mrpack: false,
            export_default_path: None,
            copy_group: (kind == PromptKind::Copy).then(|| cx.new(|cx| InputState::new(window, cx).placeholder("Group (empty = none)"))),
            copy_flags: [true; 8],
        }
    }

    pub fn set_export_defaults(&mut self, instance: &prism_core::Instance, window: &mut Window, cx: &mut Context<MainWindow>) {
        let path = std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| ".".into()))
            .join(format!("{}.zip", instance.name.replace(['/', '\\'], "_")))
            .to_string_lossy().into_owned();
        self.input.update(cx, |state, cx| state.set_value(path.clone(), window, cx));
        self.export_default_path = Some(path);
        if let Some(fields) = &self.export_fields {
            for (field, value) in fields.iter().zip([instance.name.clone(), "1.0.0".into(), String::new()]) {
                field.update(cx, |state, cx| state.set_value(value, window, cx));
            }
        }
    }

    fn set_export_format(&mut self, mrpack: bool, window: &mut Window, cx: &mut Context<MainWindow>) {
        if self.export_default_path.as_deref() == Some(self.input.read(cx).value().as_str()) {
            let mut path = std::path::PathBuf::from(self.export_default_path.as_ref().unwrap());
            path.set_extension(if mrpack { "mrpack" } else { "zip" });
            let value = path.to_string_lossy().into_owned();
            self.input.update(cx, |state, cx| state.set_value(value.clone(), window, cx));
            self.export_default_path = Some(value);
        }
        self.mrpack = mrpack;
    }

    pub fn render(&self, window: &mut Window, cx: &mut Context<MainWindow>) -> impl IntoElement {
        let title = match self.kind {
            PromptKind::Rename => "Rename Instance",
            PromptKind::ChangeGroup => "Change Group",
            PromptKind::ImportPath => "Import Instance",
            PromptKind::Export => "Export Instance",
            PromptKind::Copy => "Copy Instance",
        };
        let input = self.input.clone();

        let backdrop = div()
            .id("prompt-backdrop")
            .absolute()
            .size_full()
            .top_0()
            .left_0()
            .bg(gpui_kit::black().opacity(0.5))
            .flex()
            .items_center()
            .justify_center()
            .on_click(cx.listener(|this: &mut MainWindow, _ev, _window, cx| {
                this.prompt = None;
                cx.notify();
            }));

        let mut panel = div()
            .id("prompt-dialog")
            .w(px(380.))
            .bg(rgb(0x2b2b2b))
            .rounded_lg()
            .border_1()
            .border_color(rgb(0x444444))
            .p_4()
            .flex()
            .flex_col()
            .gap_3()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(div().text_size(px(16.)).font_weight(FontWeight::BOLD).child(title))
            .child(Input::new(&input));

        if let Some(group) = &self.copy_group {
            panel = panel.child(Input::new(group));
            for (index, label) in ["Saves", "Game options", "Resource packs", "Shader packs", "Servers", "Mods (including config)", "Screenshots", "Keep playtime"].into_iter().enumerate() {
                let checked = self.copy_flags[index];
                panel = panel.child(gpui_kit::component::checkbox::Checkbox::new(("copy-option", index)).label(label).checked(checked).on_click(cx.listener(move |this: &mut MainWindow, checked: &bool, _, cx| {
                    if let Some(prompt) = this.prompt.as_mut() { prompt.copy_flags[index] = *checked; }
                    cx.notify();
                })));
            }
            panel = panel.child(div().flex().gap_2().children([(true, "Select all"), (false, "Select none")].map(|(value, label)| {
                Button::new(label).label(label).on_click(cx.listener(move |this: &mut MainWindow, _, _, cx| {
                    if let Some(prompt) = this.prompt.as_mut() { prompt.copy_flags.fill(value); }
                    cx.notify();
                }))
            })));
        }

        if let Some(fields) = &self.export_fields {
            panel = panel.child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        Button::new("export-zip")
                            .label("Prism zip")
                            .selected(!self.mrpack)
                            .when(!self.mrpack, |button| button.primary())
                            .on_click(cx.listener(|this: &mut MainWindow, _, window, cx| {
                                if let Some(prompt) = this.prompt.as_mut() {
                                    prompt.set_export_format(false, window, cx);
                                }
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("export-mrpack")
                            .label("Modrinth mrpack")
                            .selected(self.mrpack)
                            .when(self.mrpack, |button| button.primary())
                            .on_click(cx.listener(|this: &mut MainWindow, _, window, cx| {
                                if let Some(prompt) = this.prompt.as_mut() {
                                    prompt.set_export_format(true, window, cx);
                                }
                                cx.notify();
                            })),
                    ),
            );
            if self.mrpack {
                for field in fields {
                    panel = panel.child(Input::new(field));
                }
            }
        }

        if let Some(e) = &self.error {
            panel = panel.child(div().text_size(px(12.)).text_color(rgb(0xff6666)).child(e.clone()));
        }

        panel = panel.child(
            div()
                .flex()
                .justify_end()
                .gap_2()
                .child(
                    Button::new("prompt-cancel")
                        .label("Cancel")
                        .on_click(cx.listener(|this: &mut MainWindow, _ev, _window, cx| {
                            this.prompt = None;
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("prompt-ok")
                        .label("OK")
                        .on_click(cx.listener(confirm_prompt)),
                ),
        );

        let _ = window;
        backdrop.child(panel)
    }
}

fn confirm_prompt(this: &mut MainWindow, _ev: &ClickEvent, _window: &mut Window, cx: &mut Context<MainWindow>) {
    let Some(prompt) = this.prompt.as_ref() else {
        return;
    };
    let value = prompt.input.read(cx).value().trim().to_string();
    let kind = prompt.kind;
    let target = prompt.target_id.clone();
    if kind == PromptKind::Copy {
        if value.is_empty() {
            this.prompt.as_mut().unwrap().error = Some("Name cannot be empty.".into());
            cx.notify();
            return;
        }
        let group = prompt.copy_group.as_ref().unwrap().read(cx).value().trim().to_string();
        let [saves, game_options, resource_packs, shader_packs, servers, mods, screenshots, keep_playtime] = prompt.copy_flags;
        let options = prism_core::copy::CopyOptions { saves, game_options, resource_packs, shader_packs, servers, mods, screenshots, keep_playtime };
        let app = this.app.clone();
        let state = app.read(cx);
        let source = state.instances.get(&target).cloned();
        let paths = state.paths.clone();
        let runtime = state.runtime.clone();
        let (tx, rx) = tokio::sync::oneshot::channel();
        runtime.handle().spawn_blocking(move || {
            let result = source.ok_or_else(|| "Source instance no longer exists".to_string()).and_then(|source| {
                prism_core::copy::copy_instance_files(&paths, &source, &value, &options).map_err(|e| e.to_string())
            });
            let _ = tx.send(result);
        });
        cx.spawn(async move |this, cx| {
            if let Ok(result) = rx.await {
                let _ = app.update(cx, |app, _| {
                    let message = match result {
                        Ok(instance) => {
                            app.instances.groups.set_group(&instance.id, if group.is_empty() { None } else { Some(&group) });
                            if let Err(error) = app.instances.groups.save(&app.paths.instances_dir()) {
                                app.toast = Some((format!("Copied {}, but could not save group: {error}", instance.name), std::time::Instant::now()));
                                app.reload_instances();
                                app.selected = Some(instance.id);
                                return;
                            }
                            app.reload_instances();
                            app.selected = Some(instance.id);
                            format!("Copied: {}", instance.name)
                        }
                        Err(e) => format!("Copy failed: {e}"),
                    };
                    app.toast = Some((message, std::time::Instant::now()));
                });
                let _ = this.update(cx, |_, cx| cx.notify());
            }
        }).detach();
        this.prompt = None;
        cx.notify();
        return;
    }
    if kind == PromptKind::Export {
        let mrpack = prompt.mrpack;
        let fields = prompt.export_fields.as_ref().unwrap();
        let metadata = prism_core::export::MrpackMetadata {
            name: fields[0].read(cx).value().to_string(),
            version: fields[1].read(cx).value().to_string(),
            summary: fields[2].read(cx).value().to_string(),
        };
        let mut output = std::path::PathBuf::from(&value);
        if output.extension().is_none() {
            output.set_extension(if mrpack { "mrpack" } else { "zip" });
        }
        if value.is_empty() {
            this.prompt.as_mut().unwrap().error = Some("Choose a destination path".into());
            cx.notify();
            return;
        }
        let instance = this.app.read(cx).instances.get(&target).cloned();
        if instance.is_none() {
            this.prompt.as_mut().unwrap().error = Some("Selected instance no longer exists".into());
            cx.notify();
            return;
        }
        if let Some(instance) = instance {
            let app = this.app.clone();
            let runtime = app.read(cx).runtime.clone();
            let (tx, rx) = tokio::sync::oneshot::channel();
            runtime.handle().spawn(async move {
                let result = if mrpack {
                    prism_core::export::export_mrpack(&instance, &output, metadata).await
                } else {
                    prism_core::export::export_zip(&instance, &output)
                };
                let message = match result {
                    Ok(()) => format!("Exported: {}", output.display()),
                    Err(e) => format!("Export failed: {e}"),
                };
                let _ = tx.send(message);
            });
            cx.spawn(async move |this, cx| {
                if let Ok(message) = rx.await {
                    let _ = app.update(cx, |app, _| {
                        app.toast = Some((message, std::time::Instant::now()))
                    });
                    let _ = this.update(cx, |_, cx| cx.notify());
                }
            })
            .detach();
        }
        this.prompt = None;
        cx.notify();
        return;
    }

    if kind == PromptKind::Rename && value.is_empty() {
        this.prompt.as_mut().unwrap().error = Some("Name cannot be empty.".to_string());
        cx.notify();
        return;
    }

    this.app.update(cx, |app, _cx| {
        match kind {
            PromptKind::Export | PromptKind::Copy => unreachable!(),
            PromptKind::Rename => {
                if let Some(inst) = app.instances.get_mut(&target) {
                    inst.name = value.clone();
                    let inst = inst.clone();
                    let _ = inst.save();
                }
            }
            PromptKind::ChangeGroup => {
                app.instances
                    .groups
                    .set_group(&target, if value.is_empty() { None } else { Some(value.as_str()) });
                let _ = app.instances.groups.save(&app.paths.instances_dir());
            }
            PromptKind::ImportPath => {
                // Local path (possibly a URL later). Run the import
                // synchronously — packs can take a moment.
                let source = std::path::PathBuf::from(&value);
                if !source.exists() {
                    app.toast = Some((format!("Import source not found: {value}"), std::time::Instant::now()));
                } else {
                    match prism_core::import::import_sync(&app.paths, &source, Some(&app.runtime.downloader)) {
                        Ok(outcome) => {
                            let mut message = format!("Imported: {}", outcome.instance.name);
                            if !outcome.missing_files.is_empty() {
                                let count = outcome.missing_files.len();
                                let report = prism_core::import::CURSEFORGE_MISSING_FILES_REPORT;
                                message.push_str(" — ");
                                if let Some(issue) = outcome.curseforge_key_issue {
                                    message.push_str(issue.message());
                                    message.push_str(". ");
                                }
                                message.push_str(&format!(
                                    "{count} file(s) need manual download. See {report} in the instance folder."
                                ));
                            }
                            app.toast = Some((message, std::time::Instant::now()));
                        }
                        Err(e) => {
                            app.toast = Some((format!("Import failed: {e}"), std::time::Instant::now()));
                        }
                    }
                }
            }
        }
        app.reload_instances();
    });
    this.prompt = None;
    cx.notify();
}
