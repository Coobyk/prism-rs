//! Dialogs: create instance (mirrors NewInstanceDialog).

use crate::state::App;
use crate::main_window::MainWindow;
use gpui_kit::base::Disableable;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::*;

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
}

#[derive(Clone, Copy, PartialEq)]
pub enum PromptKind {
    Rename,
    ChangeGroup,
    ImportPath,
}

impl PromptState {
    pub fn new(kind: PromptKind, target_id: String, initial: String, window: &mut Window, cx: &mut Context<MainWindow>) -> Self {
        let placeholder = match kind {
            PromptKind::Rename => "Instance name",
            PromptKind::ChangeGroup => "Group name (empty = none)",
            PromptKind::ImportPath => "Path or URL to .zip/.mrpack",
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
        }
    }

    pub fn render(&self, window: &mut Window, cx: &mut Context<MainWindow>) -> impl IntoElement {
        let title = match self.kind {
            PromptKind::Rename => "Rename Instance",
            PromptKind::ChangeGroup => "Change Group",
            PromptKind::ImportPath => "Import Instance",
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

    if kind == PromptKind::Rename && value.is_empty() {
        this.prompt.as_mut().unwrap().error = Some("Name cannot be empty.".to_string());
        cx.notify();
        return;
    }

    this.app.update(cx, |app, _cx| {
        match kind {
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
                        Ok(inst) => {
                            app.toast = Some((format!("Imported: {}", inst.name), std::time::Instant::now()));
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
