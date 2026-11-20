use crate::theme::Button;
use super::InstanceWindow;
use crate::state::App as LauncherApp;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    base::Disableable,
    component::{
        input::{Input, InputEvent, InputState, Textarea, TextareaState},
        ActiveTheme,
    },
    *,
};
use prism_core::{version_components::Pack, Instance};

pub(super) fn open(
    parent: WeakEntity<InstanceWindow>,
    app: Entity<LauncherApp>,
    instance: Instance,
    pack: Pack,
    uid: Option<String>,
    cx: &mut App,
) {
    let adding = uid.is_none();
    let dimensions = if adding {
        size(px(480.), px(300.))
    } else {
        size(px(900.), px(680.))
    };
    open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None, dimensions, cx,
            ))),
            window_min_size: Some(if adding {
                dimensions
            } else {
                size(px(680.), px(520.))
            }),
            kind: if adding {
                WindowKind::Dialog
            } else {
                WindowKind::Normal
            },
            is_resizable: !adding,
            app_id: Some(
                if adding {
                    "prism-rs-add-component"
                } else {
                    "prism-rs-component-editor"
                }
                .into(),
            ),
            ..Default::default()
        },
        cx,
        |window, cx| cx.new(|cx| Editor::new(parent, app, instance, pack, uid, window, cx)),
    )
    .ok();
}

struct Editor {
    parent: WeakEntity<InstanceWindow>,
    app: Entity<LauncherApp>,
    instance: Instance,
    pack: Pack,
    uid: Option<String>,
    text: Entity<TextareaState>,
    name: Entity<InputState>,
    identifier: Entity<InputState>,
    original: String,
    ready: bool,
    busy: bool,
    confirming_close: bool,
    status: String,
}

impl Editor {
    fn new(
        parent: WeakEntity<InstanceWindow>,
        app: Entity<LauncherApp>,
        instance: Instance,
        pack: Pack,
        uid: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let text = cx.new(|cx| TextareaState::new(window, cx).rows(20));
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("Component name"));
        let identifier = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Component UID (e.g. custom.example)")
        });
        let editor = cx.entity().downgrade();
        window.on_window_should_close(cx, move |window, cx| {
            let _ = editor.update(cx, |editor, cx| editor.close(window, cx));
            false
        });
        cx.subscribe(&text, |_, _, _: &InputEvent, cx| cx.notify())
            .detach();
        cx.subscribe(&name, |_, _, _: &InputEvent, cx| cx.notify())
            .detach();
        cx.subscribe(&identifier, |_, _, _: &InputEvent, cx| cx.notify())
            .detach();
        let ready = uid.is_none();
        if let Some(uid) = uid.clone() {
            let pack = pack.clone();
            let load = cx.background_spawn(async move { pack.patch_text(&uid) });
            cx.spawn_in(window, async move |this, cx| {
                let result = load.await;
                let _ = this.update_in(cx, |this, window, cx| {
                    match result {
                        Ok(value) => {
                            this.original = value.clone();
                            this.text
                                .update(cx, |state, cx| state.set_value(value, window, cx));
                            this.ready = true;
                        }
                        Err(error) => this.status = error.to_string(),
                    }
                    cx.notify();
                });
            })
            .detach();
        }
        Self {
            parent,
            app,
            instance,
            pack,
            uid,
            text,
            name,
            identifier,
            original: String::new(),
            ready,
            busy: false,
            confirming_close: false,
            status: String::new(),
        }
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy || self.confirming_close {
            return;
        }
        let dirty = self.uid.is_some() && self.text.read(cx).value().as_str() != self.original;
        if !dirty {
            window.remove_window();
            return;
        }
        self.confirming_close = true;
        cx.notify();
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy || !self.ready {
            return;
        }
        let app = self.app.read(cx);
        if app.is_running(&self.instance.id) || app.instances.get(&self.instance.id).is_none() {
            self.status = "The instance is running or no longer exists.".into();
            cx.notify();
            return;
        }
        let runtime = app.runtime.clone();
        let paths = app.paths.clone();
        let instance = self.instance.clone();
        let pack = self.pack.clone();
        let uid = self.uid.clone();
        let text = self.text.read(cx).value().to_string();
        let name = self.name.read(cx).value().to_string();
        let identifier = self.identifier.read(cx).value().to_string();
        let saving_text = text.clone();
        let saving_uid = uid.clone().unwrap_or_else(|| identifier.clone());
        let task = runtime.handle().spawn_blocking(move || {
            let _lock = crate::instance_lock::acquire(&paths, &instance)?;
            match uid {
                Some(uid) => pack.save_patch(&uid, &text),
                None => pack.add_empty(&identifier, &name),
            }
        });
        self.busy = true;
        cx.spawn_in(window, async move |this, cx| {
            let result = task
                .await
                .map_err(|e| e.to_string())
                .and_then(|r| r.map_err(|e| e.to_string()));
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match result {
                    Ok(pack) => {
                        this.pack = pack;
                        if this.uid.is_none() {
                            window.remove_window();
                        } else {
                            this.original = saving_text;
                            this.status = format!("Saved {saving_uid}.");
                        }
                        let _ = this
                            .parent
                            .update(cx, |parent, cx| parent.refresh_version(cx));
                    }
                    Err(error) => this.status = format!("Could not save: {error}"),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}

impl Render for Editor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = crate::theme::Palette::get(cx);
        let adding = self.uid.is_none();
        let identifier = self.identifier.read(cx).value().to_string();
        let invalid = if adding {
            if identifier.is_empty() {
                Some("Enter a unique component UID.".to_owned())
            } else if let Err(error) = prism_core::component::uid(&identifier) {
                Some(error.to_string())
            } else if self.pack.components().any(|(uid, _)| uid == identifier) {
                Some("A component with this UID already exists.".into())
            } else {
                None
            }
        } else {
            None
        };
        let message = invalid
            .as_ref()
            .filter(|_| !identifier.is_empty())
            .unwrap_or(&self.status)
            .clone();
        div()
            .relative()
            .size_full()
            .p_4()
            .flex()
            .flex_col()
            .gap_3()
            .bg(palette.window)
            .when(adding, |dialog| {
                dialog
                    .bg(palette.panel)
                    .border_1()
                    .border_color(palette.strong_border)
                    .rounded_lg()
            })
            .text_color(palette.text)
            .child(
                div().font_weight(FontWeight::BOLD).child(
                    self.uid
                        .as_ref()
                        .map(|uid| format!("Edit component — {uid}"))
                        .unwrap_or_else(|| "Add empty component".into()),
                ),
            )
            .children(self.uid.is_none().then(|| {
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(palette.secondary_text)
                            .child("Name"),
                    )
                    .child(Input::new(&self.name).disabled(self.busy || !self.ready))
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(palette.secondary_text)
                            .child("UID"),
                    )
                    .child(Input::new(&self.identifier).disabled(self.busy || !self.ready))
            }))
            .children(self.uid.is_some().then(|| {
                div()
                    .id("component-editor-scroll")
                    .flex_1()
                    .min_h_0()
                    .child(
                        Textarea::new(&self.text)
                            .h(relative(1.))
                            .font_family(cx.theme().mono_font_family.clone())
                            .disabled(self.busy || !self.ready),
                    )
            }))
            .children((!message.is_empty()).then(|| {
                div()
                    .text_size(px(12.))
                    .text_color(if invalid.is_some() || message.starts_with("Could not") {
                        palette.error
                    } else {
                        palette.secondary_text
                    })
                    .child(message)
            }))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .mt_auto()
                    .gap_2()
                    .children(adding.then(|| {
                        Button::new("component-cancel")
                            .label("Cancel")
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, window, cx| this.close(window, cx)))
                    }))
                    .child(
                        Button::new("component-save")
                            .label(if adding { "Add" } else { "Save" })
                            .disabled(
                                !self.ready
                                    || self.busy
                                    || invalid.is_some()
                                    || (adding && self.name.read(cx).value().trim().is_empty()),
                            )
                            .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                    )
                    .children((!adding).then(|| {
                        Button::new("component-close")
                            .label("Close")
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, window, cx| this.close(window, cx)))
                    })),
            )
            .children(self.confirming_close.then(|| {
                div()
                    .absolute()
                    .inset_0()
                    .size_full()
                    .occlude()
                    .bg(palette.overlay)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .w(px(410.))
                            .p_4()
                            .rounded_lg()
                            .bg(palette.panel)
                            .border_1()
                            .border_color(palette.strong_border)
                            .flex()
                            .flex_col()
                            .gap_3()
                            .child(
                                div()
                                    .font_weight(FontWeight::BOLD)
                                    .child("Discard unsaved changes?"),
                            )
                            .child("Your changes have not been saved to the component patch.")
                            .child(
                                div()
                                    .flex()
                                    .justify_end()
                                    .gap_2()
                                    .child(
                                        Button::new("component-keep-editing")
                                            .label("Keep editing")
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.confirming_close = false;
                                                cx.notify();
                                            })),
                                    )
                                    .child(
                                        Button::new("component-discard").label("Discard").on_click(
                                            cx.listener(|_, _, window, _| window.remove_window()),
                                        ),
                                    ),
                            ),
                    )
            }))
    }
}
