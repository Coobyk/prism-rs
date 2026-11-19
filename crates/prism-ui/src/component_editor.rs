use super::InstanceWindow;
use crate::state::App as LauncherApp;
use gpui_kit::{
    base::Disableable,
    component::{
        button::Button,
        input::{Input, InputEvent, InputState, Textarea, TextareaState},
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
    open_window(
        WindowOptions {
            window_min_size: Some(Size {
                width: px(680.),
                height: px(520.),
            }),
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
        let dirty = if self.uid.is_some() {
            self.text.read(cx).value().as_str() != self.original
        } else {
            self.ready
                && (!self.name.read(cx).value().is_empty()
                    || !self.identifier.read(cx).value().is_empty())
        };
        if !dirty {
            window.remove_window();
            return;
        }
        let answer = window.prompt(
            PromptLevel::Warning,
            "Discard unsaved changes?",
            None,
            &[
                PromptButton::cancel("Keep editing"),
                PromptButton::new("Discard"),
            ],
            cx,
        );
        self.confirming_close = true;
        cx.spawn_in(window, async move |this, cx| {
            let discard = answer.await == Ok(1);
            let _ = this.update_in(cx, |this, window, _| {
                this.confirming_close = false;
                if discard {
                    window.remove_window();
                }
            });
        })
        .detach();
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
            let result = task.await.map_err(|e| e.to_string()).and_then(|r| r.map_err(|e| e.to_string()));
            let _ = this.update_in(cx, |this, _, cx| {
                this.busy = false;
                match result {
                    Ok(pack) => {
                        this.pack = pack;
                        if this.uid.is_none() {
                            this.ready = false;
                            this.status = "Component created. Close this window and use Edit to customize it.".into();
                        } else {
                            this.original = saving_text;
                            this.status = format!("Saved {saving_uid}.");
                        }
                        let _ = this.parent.update(cx, |parent, cx| parent.refresh_version(cx));
                    }
                    Err(error) => this.status = format!("Could not save: {error}"),
                }
                cx.notify();
            });
        }).detach();
        cx.notify();
    }
}

impl Render for Editor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .p_4()
            .flex()
            .flex_col()
            .gap_3()
            .bg(rgb(0x232323))
            .text_color(rgb(0xdddddd))
            .child(
                div().child(
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
                    .child(Input::new(&self.name).disabled(self.busy || !self.ready))
                    .child(Input::new(&self.identifier).disabled(self.busy || !self.ready))
            }))
            .children(self.uid.is_some().then(|| {
                div()
                    .id("component-editor-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(Textarea::new(&self.text).disabled(self.busy || !self.ready))
            }))
            .child(div().child(self.status.clone()))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        Button::new("component-save")
                            .label("Save")
                            .disabled(!self.ready || self.busy)
                            .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                    )
                    .child(
                        Button::new("component-close")
                            .label("Close")
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, window, cx| this.close(window, cx))),
                    ),
            )
    }
}
