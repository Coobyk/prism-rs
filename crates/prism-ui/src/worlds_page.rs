use super::{datapacks_page::open_folder, InstanceWindow};
use gpui_kit::base::Disableable;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::*;
use prism_core::datapacks::World;
use prism_core::launch::QuickPlayTarget;
use prism_core::worlds::WorldDetails;
use std::io;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Default)]
pub(super) struct WorldsPage {
    entries: Option<Vec<WorldEntry>>,
    selected: Option<String>,
    revision: u64,
    status: String,
    busy: bool,
    name_input: Option<Entity<InputState>>,
}

struct WorldEntry {
    folder: String,
    /// `None` for worlds that cannot be managed safely (Bedrock, symlinked
    /// or missing a regular `level.dat`); those keep only Join.
    world: Option<World>,
    details: Option<Result<WorldDetails, String>>,
}

impl WorldEntry {
    fn display_name(&self) -> &str {
        match &self.details {
            Some(Ok(details)) => &details.display_name,
            _ => &self.folder,
        }
    }
}

#[derive(Clone)]
struct WorldTarget {
    world: World,
    revision: u64,
}

impl WorldsPage {
    pub(super) fn invalidate(&mut self) {
        self.entries = None;
    }

    fn matches(&self, target: &WorldTarget) -> bool {
        self.revision == target.revision && self.selected.as_deref() == Some(target.world.name())
    }

    fn selected(&self) -> Option<&WorldEntry> {
        let selected = self.selected.as_deref()?;
        self.entries
            .as_ref()?
            .iter()
            .find(|entry| entry.folder == selected)
    }
}

type WorldOp = Box<dyn FnOnce(&World) -> io::Result<(String, Option<String>)> + Send>;

impl InstanceWindow {
    fn load_worlds(&mut self, cx: &Context<Self>) {
        let inst = self.instance(cx);
        let java = !inst.is_horizon();
        let game_root = inst.game_root();
        let entries = inst
            .worlds()
            .into_iter()
            .map(|folder| {
                let world = java
                    .then(|| World::open(&game_root, &folder).ok())
                    .flatten();
                let details = world
                    .as_ref()
                    .map(|world| world.details().map_err(|error| error.to_string()));
                WorldEntry {
                    folder,
                    world,
                    details,
                }
            })
            .collect();
        self.worlds.entries = Some(entries);
    }

    fn select_world(
        &mut self,
        folder: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.worlds.selected != folder {
            self.worlds.revision += 1;
            self.worlds.selected = folder;
        }
        let Some(name) = self
            .worlds
            .selected()
            .map(|entry| entry.display_name().to_owned())
        else {
            self.worlds.name_input = None;
            return;
        };
        let input = self
            .worlds
            .name_input
            .get_or_insert_with(|| {
                let input = cx.new(|cx| InputState::new(window, cx).placeholder("World name"));
                cx.subscribe(&input, |_, _, _: &InputEvent, cx| cx.notify())
                    .detach();
                input
            })
            .clone();
        input.update(cx, |state, cx| state.set_value(name, window, cx));
    }

    fn world_target(&self) -> Option<WorldTarget> {
        Some(WorldTarget {
            world: self.worlds.selected()?.world.clone()?,
            revision: self.worlds.revision,
        })
    }

    /// Runs `op` off the UI thread while holding the instance lock, after
    /// checking that the target is still the selected world of this instance.
    fn run_world_op(
        &mut self,
        target: WorldTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
        op: WorldOp,
    ) {
        let lock = (|| {
            if self.worlds.busy {
                return Err(io::Error::other("Another world operation is in progress"));
            }
            if !self.worlds.matches(&target) {
                return Err(io::Error::other("The selected world changed"));
            }
            let app = self.app.read(cx);
            let instance = app
                .instances
                .get(&self.instance_id)
                .filter(|instance| {
                    !instance.is_horizon() && instance.root.join("instance.cfg").is_file()
                })
                .ok_or_else(|| io::Error::other("The Java instance no longer exists"))?;
            if instance.game_root() != target.world.game_root() {
                return Err(io::Error::other("The instance game folder changed"));
            }
            if app.is_running(&self.instance_id) {
                return Err(io::Error::other("Stop Minecraft before changing worlds"));
            }
            crate::instance_lock::acquire(&app.paths, instance).map_err(|error| {
                if error.kind() == io::ErrorKind::WouldBlock {
                    io::Error::other("The instance is running or another operation is in progress")
                } else {
                    error
                }
            })
        })();
        let lock = match lock {
            Ok(lock) => lock,
            Err(error) => {
                self.worlds.status = error.to_string();
                cx.notify();
                return;
            }
        };
        self.worlds.busy = true;
        self.worlds.status = "Working…".into();
        let task = self.app.read(cx).runtime.handle().spawn_blocking(move || {
            let result = op(&target.world);
            drop(lock);
            result
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task
                .await
                .map_err(io::Error::other)
                .and_then(|result| result);
            let _ = this.update_in(cx, |this, window, cx| {
                this.worlds.busy = false;
                this.load_worlds(cx);
                let select = match result {
                    Ok((status, select)) => {
                        this.worlds.status = status;
                        select.or_else(|| this.worlds.selected.clone())
                    }
                    Err(error) => {
                        this.worlds.status = format!("Failed: {error}");
                        this.worlds.selected.clone()
                    }
                };
                let select = select.filter(|folder| {
                    this.worlds
                        .entries
                        .iter()
                        .flatten()
                        .any(|entry| &entry.folder == folder)
                });
                this.select_world(select, window, cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn entered_name(&self, cx: &Context<Self>) -> String {
        self.worlds
            .name_input
            .as_ref()
            .map(|input| input.read(cx).value().trim().to_owned())
            .unwrap_or_default()
    }

    fn rename_world(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.world_target() else {
            return;
        };
        let name = self.entered_name(cx);
        self.run_world_op(
            target,
            window,
            cx,
            Box::new(move |world| {
                let renamed = world.rename(&name)?;
                Ok((
                    format!("Renamed to {name:?} (folder {:?})", renamed.name()),
                    Some(renamed.name().into()),
                ))
            }),
        );
    }

    fn copy_world(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.world_target() else {
            return;
        };
        let name = self.entered_name(cx);
        self.run_world_op(
            target,
            window,
            cx,
            Box::new(move |world| {
                let copy = world.copy(&name)?;
                Ok((
                    format!("Copied to {name:?} (folder {:?})", copy.name()),
                    Some(copy.name().into()),
                ))
            }),
        );
    }

    fn confirm_delete_world(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.world_target() else {
            return;
        };
        let Some(entry) = self.worlds.selected() else {
            return;
        };
        let answer = window.prompt(
            gpui::PromptLevel::Warning,
            &format!("Permanently delete {:?}?", entry.display_name()),
            Some("This cannot be undone."),
            &[
                gpui::PromptButton::cancel("Cancel"),
                gpui::PromptButton::new("Delete"),
            ],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if answer.await == Ok(1) {
                let _ = this.update_in(cx, |this, window, cx| {
                    let folder = target.world.name().to_owned();
                    this.run_world_op(
                        target,
                        window,
                        cx,
                        Box::new(move |world| {
                            world.delete()?;
                            Ok((format!("Deleted saves/{folder}"), None))
                        }),
                    );
                });
            }
        })
        .detach();
    }

    pub(super) fn render_worlds_page(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        if self.worlds.entries.is_none() {
            self.load_worlds(cx);
        }
        let inst = self.instance(cx);
        let saves = inst.game_root().join("saves");
        let running = self.app.read(cx).is_running(&inst.id);
        let entries = self.worlds.entries.as_deref().unwrap_or_default();
        let mut list = div().flex().flex_col().gap_1().p_3();
        for (index, entry) in entries.iter().enumerate() {
            let active = self.worlds.selected.as_deref() == Some(entry.folder.as_str());
            let folder = entry.folder.clone();
            let id = inst.id.clone();
            let mut label = div()
                .flex()
                .flex_col()
                .min_w_0()
                .child(div().text_ellipsis().child(entry.display_name().to_owned()));
            if entry.display_name() != entry.folder {
                label = label.child(
                    div()
                        .text_ellipsis()
                        .text_size(px(11.))
                        .text_color(rgb(0x888888))
                        .child(format!("saves/{}", entry.folder)),
                );
            }
            let select = folder.clone();
            list = list.child(
                div()
                    .id(("world-row", index))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .cursor_pointer()
                    .bg(if active { rgb(0x3d5a80) } else { rgb(0x2b2b2b) })
                    .hover(|style| style.bg(rgb(0x384b60)))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.worlds.status.clear();
                        this.select_world(Some(select.clone()), window, cx);
                        cx.notify();
                    }))
                    .child(label)
                    .child(
                        Button::new(("join-world", index))
                            .label("Join")
                            .disabled(running)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.app.update(cx, |app, cx| {
                                    app.start_launch_with(
                                        &id,
                                        Some(QuickPlayTarget::Singleplayer {
                                            world: folder.clone(),
                                        }),
                                        cx,
                                    )
                                });
                                cx.notify();
                            })),
                    ),
            );
        }
        if entries.is_empty() {
            list = list.child(div().text_color(rgb(0x777777)).child("No worlds."));
        }
        let list = div()
            .id("worlds-scroll")
            .flex_1()
            .min_w_0()
            .overflow_y_scroll()
            .child(list);

        let mut page = div().flex_1().flex().flex_col().min_h_0().child(
            self.page_header(cx, saves.display().to_string()).child(
                Button::new("worlds-refresh")
                    .label("Refresh")
                    .disabled(self.worlds.busy)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.worlds.status.clear();
                        this.load_worlds(cx);
                        let selected = this.worlds.selected.clone();
                        this.select_world(
                            selected.filter(|_| this.worlds.selected().is_some()),
                            window,
                            cx,
                        );
                        cx.notify();
                    })),
            ),
        );
        if running {
            page = page.child(
                div()
                    .px_3()
                    .py_1()
                    .text_color(rgb(0x8fb4d9))
                    .child("Minecraft is running; worlds are read-only."),
            );
        }
        page.child(
            div()
                .flex_1()
                .flex()
                .min_h_0()
                .child(list)
                .child(self.render_world_details(running, cx)),
        )
        .into_any_element()
    }

    fn render_world_details(&self, running: bool, cx: &mut Context<Self>) -> gpui::AnyElement {
        let panel = div()
            .w(px(300.))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .border_l_1()
            .border_color(rgb(0x1c1c1c));
        let Some(entry) = self.worlds.selected() else {
            return panel
                .child(
                    div()
                        .text_color(rgb(0x777777))
                        .child("Select a world to manage it."),
                )
                .child(self.render_world_status())
                .into_any_element();
        };
        let field = |label: &str, value: String| {
            div()
                .flex()
                .gap_2()
                .child(
                    div()
                        .w(px(80.))
                        .flex_shrink_0()
                        .text_color(rgb(0x888888))
                        .child(label.to_owned()),
                )
                .child(div().min_w_0().text_ellipsis().child(value))
        };
        let mut panel = panel
            .child(
                div()
                    .font_weight(FontWeight::BOLD)
                    .text_ellipsis()
                    .child(entry.display_name().to_owned()),
            )
            .child(field("Folder", entry.folder.clone()));
        match &entry.details {
            Some(Ok(details)) => {
                let mode = details.game_mode.map_or("Unknown", |mode| mode.label());
                panel = panel
                    .child(field(
                        "Game mode",
                        if details.hardcore {
                            format!("{mode} (Hardcore)")
                        } else {
                            mode.into()
                        },
                    ))
                    .child(field(
                        "Last played",
                        details.last_played.map_or("Never".into(), last_played),
                    ));
                if let Some(seed) = details.seed {
                    panel = panel.child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .child(field("Seed", seed.to_string()))
                            .child(Button::new("world-copy-seed").label("Copy").on_click(
                                cx.listener(move |this, _, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        seed.to_string(),
                                    ));
                                    this.worlds.status = "Seed copied".into();
                                    cx.notify();
                                }),
                            )),
                    );
                }
            }
            Some(Err(error)) => {
                panel = panel.child(
                    div()
                        .text_color(rgb(0xd98f8f))
                        .child(format!("Could not read level.dat: {error}")),
                );
            }
            None => {
                panel = panel.child(
                    div()
                        .text_color(rgb(0x777777))
                        .child("This world can only be joined from here."),
                );
            }
        }
        if entry.world.is_some() {
            let locked = running || self.worlds.busy;
            let name = self.entered_name(cx);
            let named = !name.is_empty() && name != entry.display_name();
            let path = entry.world.as_ref().and_then(|world| world.path().ok());
            if let Some(input) = &self.worlds.name_input {
                panel = panel.child(div().pt_2().child(Input::new(input).disabled(locked)));
            }
            panel =
                panel
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                Button::new("world-rename")
                                    .label("Rename")
                                    .disabled(locked || !named)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.rename_world(window, cx)
                                    })),
                            )
                            .child(
                                Button::new("world-copy")
                                    .label("Copy")
                                    .disabled(locked || name.is_empty())
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.copy_world(window, cx)
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                Button::new("world-open-folder")
                                    .label("Open Folder")
                                    .disabled(path.is_none())
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if let Some(path) = &path {
                                            if let Err(error) = open_folder(path) {
                                                this.worlds.status =
                                                    format!("Could not open folder: {error}");
                                                cx.notify();
                                            }
                                        }
                                    })),
                            )
                            .child(
                                Button::new("world-delete")
                                    .label("Delete…")
                                    .danger()
                                    .disabled(locked)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.confirm_delete_world(window, cx)
                                    })),
                            ),
                    );
        }
        panel.child(self.render_world_status()).into_any_element()
    }

    fn render_world_status(&self) -> Div {
        div()
            .text_color(rgb(0xaaaaaa))
            .child(self.worlds.status.clone())
    }
}

fn last_played(millis: i64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64);
    match (now - millis).max(0) / 86_400_000 {
        0 => "Today".into(),
        1 => "Yesterday".into(),
        days => format!("{days} days ago"),
    }
}
