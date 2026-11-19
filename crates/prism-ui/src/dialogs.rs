//! Dialogs: create instance (mirrors NewInstanceDialog).

use crate::state::App;
use crate::main_window::MainWindow;
use gpui_kit::component::button::Button;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::*;

pub struct CreateInstanceState {
    pub name_input: Entity<InputState>,
    pub group_input: Entity<InputState>,
    pub version_filter: Entity<InputState>,
    pub selected_version: Option<String>,
    pub error: Option<String>,
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
        if let Some(manifest) = &manifest {
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
                this.create_dialog = None;
                cx.notify();
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
            )
            .child(
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
                        .on_click(cx.listener(|this: &mut MainWindow, _ev, _window, cx| {
                            this.create_dialog = None;
                            cx.notify();
                        })),
                )
                .child(
                    gpui_kit::component::button::Button::new("ok")
                        .label("OK")
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
    let name = dialog.name_input.read(cx).value().trim().to_string();
    let group = dialog.group_input.read(cx).value().trim().to_string();
    let version = dialog.selected_version.clone();

    if name.is_empty() {
        dialog.error = Some("Please enter a name.".to_string());
        cx.notify();
        return;
    }
    let Some(version) = version else {
        dialog.error = Some("Please select a Minecraft version.".to_string());
        cx.notify();
        return;
    };

    let app = this.app.clone();
    let mut error = None;
    let mut created_id = None;
    app.update(cx, |app, _cx| {
        match prism_core::InstanceList::create(&app.paths, &name, "default", &version) {
            Ok(inst) => {
                let inst_id = inst.id.clone();
                if !group.is_empty() {
                    app.instances.groups.set_group(&inst_id, Some(&group));
                    let _ = app.instances.groups.save(&app.paths.instances_dir());
                }
                created_id = Some(inst_id);
            }
            Err(e) => error = Some(format!("Failed to create instance: {e}")),
        }
    });

    if let Some(e) = error {
        dialog.error = Some(e);
        cx.notify();
        return;
    }

    app.update(cx, |app, _cx| {
        app.reload_instances();
        app.selected = created_id.clone();
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
}

#[derive(Clone, Copy, PartialEq)]
pub enum PromptKind {
    Rename,
    ChangeGroup,
    ImportPath,
    Export,
}

impl PromptState {
    pub fn new(kind: PromptKind, target_id: String, initial: String, window: &mut Window, cx: &mut Context<MainWindow>) -> Self {
        let placeholder = match kind {
            PromptKind::Rename => "Instance name",
            PromptKind::ChangeGroup => "Group name (empty = none)",
            PromptKind::ImportPath => "Path or URL to .zip/.mrpack",
            PromptKind::Export => "Destination path (must not already exist)",
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
        }
    }

    pub fn render(&self, window: &mut Window, cx: &mut Context<MainWindow>) -> impl IntoElement {
        let title = match self.kind {
            PromptKind::Rename => "Rename Instance",
            PromptKind::ChangeGroup => "Change Group",
            PromptKind::ImportPath => "Import Instance",
            PromptKind::Export => "Export Instance",
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
            .child(
                div()
                    .text_size(px(16.))
                    .font_weight(FontWeight::BOLD)
                    .child(title),
            )
            .child(Input::new(&input));

        if let Some(fields) = &self.export_fields {
            panel = panel.child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        Button::new("export-zip")
                            .label(if self.mrpack {
                                "Prism zip"
                            } else {
                                "✓ Prism zip"
                            })
                            .on_click(cx.listener(|this: &mut MainWindow, _, _, cx| {
                                if let Some(prompt) = this.prompt.as_mut() {
                                    prompt.mrpack = false;
                                }
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("export-mrpack")
                            .label(if self.mrpack {
                                "✓ Modrinth mrpack"
                            } else {
                                "Modrinth mrpack"
                            })
                            .on_click(cx.listener(|this: &mut MainWindow, _, _, cx| {
                                if let Some(prompt) = this.prompt.as_mut() {
                                    prompt.mrpack = true;
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
            panel = panel.child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(0xff6666))
                    .child(e.clone()),
            );
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
    if kind == PromptKind::Export {
        let mrpack = prompt.mrpack;
        let fields = prompt.export_fields.as_ref().unwrap();
        let metadata = prism_core::export::MrpackMetadata {
            name: fields[0].read(cx).value().to_string(),
            version: fields[1].read(cx).value().to_string(),
            summary: fields[2].read(cx).value().to_string(),
        };
        let mut output = std::path::PathBuf::from(&value);
        output.set_extension(if mrpack { "mrpack" } else { "zip" });
        if value.is_empty() {
            this.prompt.as_mut().unwrap().error = Some("Choose a destination path".into());
            cx.notify();
            return;
        }
        let instance = this.app.read(cx).instances.get(&target).cloned();
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
            PromptKind::Export => unreachable!(),
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
