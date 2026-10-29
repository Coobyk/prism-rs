//! Dialogs: create instance (mirrors NewInstanceDialog).

use crate::state::App;
use crate::main_window::MainWindow;
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
                    .bg(if is_selected { rgb(0x3d5a80) } else { gpui_kit::transparent() })
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
                        .primary()
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
