use super::{InstanceWindow, Page, datapacks_page::open_folder};
use crate::theme::Button;
use gpui_kit::base::Disableable;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::*;
use prism_core::Instance;
use prism_core::launch::QuickPlayTarget;
use prism_core::worlds::{RenameOutcome, World, WorldDetails};
use std::{io, path::PathBuf};

enum WorldRow {
    Java(World, io::Result<WorldDetails>),
    Horizon(String),
}

impl WorldRow {
    fn folder(&self) -> &str {
        match self {
            Self::Java(world, _) => world.name(),
            Self::Horizon(folder) => folder,
        }
    }
}

struct WorldList {
    game_root: PathBuf,
    rows: Vec<WorldRow>,
    error: Option<String>,
}

#[derive(Clone, Copy)]
enum Action {
    Rename,
    Copy,
    Delete,
}

impl Action {
    fn label(self) -> &'static str {
        match self {
            Self::Rename => "Rename",
            Self::Copy => "Copy",
            Self::Delete => "Delete",
        }
    }
}

struct WorldDialog {
    action: Action,
    target: World,
    name: String,
    input: Option<Entity<InputState>>,
    error: Option<&'static str>,
}

#[derive(Default)]
pub(super) struct WorldsPage {
    list: Option<WorldList>,
    dialog: Option<WorldDialog>,
    status: Option<String>,
    busy: bool,
    was_running: bool,
}

impl WorldsPage {
    pub(super) fn on_navigation(&mut self) {
        self.list = None;
        self.dialog = None;
        self.status = None;
    }
}

impl InstanceWindow {
    pub(super) fn sync_worlds_page(&mut self, running: bool, cx: &Context<Self>) {
        if self.page == Page::Worlds {
            if self.app.read(cx).instances.get(&self.instance_id).is_none() {
                self.worlds.on_navigation();
            } else if self.worlds.list.is_none() || (self.worlds.was_running && !running) {
                self.refresh_worlds(cx);
            }
        }
        self.worlds.was_running = running;
    }

    fn refresh_worlds(&mut self, cx: &Context<Self>) {
        let Some(instance) = self.app.read(cx).instances.get(&self.instance_id) else {
            self.worlds.list = None;
            return;
        };
        let mut list = WorldList {
            game_root: instance.game_root(),
            rows: Vec::new(),
            error: None,
        };
        if instance.is_horizon() {
            list.rows = instance
                .worlds()
                .into_iter()
                .map(WorldRow::Horizon)
                .collect();
        } else {
            match World::list(&list.game_root) {
                Ok(worlds) => {
                    list.rows = worlds
                        .into_iter()
                        .map(|world| {
                            let details = world.details();
                            WorldRow::Java(world, details)
                        })
                        .collect();
                }
                Err(error) => list.error = Some(format!("Could not list worlds: {error}")),
            }
        }
        self.worlds.list = Some(list);
    }

    fn current_world_instance(&self, cx: &Context<Self>) -> io::Result<Instance> {
        self.app
            .read(cx)
            .instances
            .get(&self.instance_id)
            .filter(|instance| instance.root.join("instance.cfg").is_file())
            .cloned()
            .ok_or_else(|| io::Error::other("The instance no longer exists"))
    }

    fn open_world_folder(&mut self, world: Option<&World>, cx: &mut Context<Self>) {
        let result = (|| {
            let game_root = self.current_world_instance(cx)?.game_root();
            let folder = match world {
                Some(world) if world.game_root() == game_root => world.world_path()?,
                Some(_) => return Err(io::Error::other("The instance game folder changed")),
                None => game_root.join("saves"),
            };
            if !folder.is_dir() {
                return Err(io::Error::other("No saves folder yet"));
            }
            open_folder(&folder)
        })();
        self.worlds.status = result
            .err()
            .map(|error| format!("Could not open folder: {error}"));
        cx.notify();
    }

    fn join_world(&mut self, folder: &str, world: Option<&World>, cx: &mut Context<Self>) {
        let result = self.current_world_instance(cx).and_then(|instance| {
            if let Some(world) = world {
                if instance.game_root() != world.game_root() {
                    return Err(io::Error::other("The instance game folder changed"));
                }
                world.world_path()?;
            }
            Ok(())
        });
        if let Err(error) = result {
            self.worlds.status = Some(format!("Could not join world: {error}"));
            self.refresh_worlds(cx);
        } else {
            let id = self.instance_id.clone();
            self.app.update(cx, |app, cx| {
                app.start_launch_with(
                    &id,
                    Some(QuickPlayTarget::Singleplayer {
                        world: folder.to_owned(),
                    }),
                    cx,
                );
            });
        }
        cx.notify();
    }

    fn show_world_dialog(
        &mut self,
        action: Action,
        target: World,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = match action {
            Action::Delete => None,
            Action::Rename | Action::Copy => {
                let value = if matches!(action, Action::Copy) {
                    format!("{name} Copy")
                } else {
                    name.clone()
                };
                Some(cx.new(|cx| {
                    let mut input = InputState::new(window, cx).placeholder("World name");
                    input.set_value(value, window, cx);
                    input.focus(window, cx);
                    input
                }))
            }
        };
        self.worlds.dialog = Some(WorldDialog {
            action,
            target,
            name,
            input,
            error: None,
        });
        cx.notify();
    }

    fn apply_world_action(&mut self, cx: &mut Context<Self>) {
        if self.worlds.busy {
            return;
        }
        let Some(dialog) = self.worlds.dialog.as_mut() else {
            return;
        };
        let name = dialog
            .input
            .as_ref()
            .map(|input| input.read(cx).value().trim().to_string());
        if name.as_ref().is_some_and(|value| value.is_empty()) {
            dialog.error = Some("Enter a world name.");
            cx.notify();
            return;
        }
        let WorldDialog { action, target, .. } = self.worlds.dialog.take().unwrap();
        let guard = (|| {
            let instance = self.current_world_instance(cx)?;
            if instance.is_horizon() || instance.game_root() != target.game_root() {
                return Err(io::Error::other("The Java instance game folder changed"));
            }
            let app = self.app.read(cx);
            if app.is_running(&self.instance_id) {
                return Err(io::Error::other("Stop Minecraft before changing worlds"));
            }
            let lock = crate::instance_lock::acquire(&app.paths, &instance)?;
            Ok((app.runtime.clone(), lock, instance))
        })();
        let (runtime, lock, instance) = match guard {
            Ok(guard) => guard,
            Err(error) => {
                self.worlds.status = Some(format!("{} failed: {error}", action.label()));
                self.refresh_worlds(cx);
                cx.notify();
                return;
            }
        };
        self.worlds.busy = true;
        self.worlds.status = Some(format!("{} in progress…", action.label()));
        let task = runtime.handle().spawn_blocking(move || {
            let _lock = lock;
            if !instance.root.join("instance.cfg").is_file()
                || instance.game_root() != target.game_root()
            {
                return Err(io::Error::other("The instance game folder changed"));
            }
            match action {
                Action::Rename => target
                    .rename(name.as_deref().unwrap())
                    .map(|outcome| match outcome {
                        RenameOutcome::Renamed => "World renamed.".into(),
                        RenameOutcome::NameOnly(error) => {
                            format!("World name changed, but the save folder could not be renamed: {error}")
                        }
                    }),
                Action::Copy => target
                    .copy(name.as_deref().unwrap())
                    .map(|_| "World copied.".into()),
                Action::Delete => target.delete().map(|()| "World deleted.".into()),
            }
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.worlds.busy = false;
                if this.page == Page::Worlds {
                    this.worlds.status = Some(match result {
                        Ok(Ok(message)) => message,
                        Ok(Err(error)) => format!("{} failed: {error}", action.label()),
                        Err(error) => format!("{} failed: {error}", action.label()),
                    });
                    this.refresh_worlds(cx);
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn render_worlds_page(&self, cx: &mut Context<Self>) -> AnyElement {
        let palette = crate::theme::Palette::get(cx);
        let Some(list) = &self.worlds.list else {
            return div()
                .p_3()
                .child("The instance no longer exists.")
                .into_any_element();
        };
        let running = self.app.read(cx).is_running(&self.instance_id);
        let busy = self.worlds.busy;
        let mut rows = div().flex().flex_col().gap_2().p_3();
        if let Some(error) = &list.error {
            rows = rows.child(div().text_color(palette.error).child(error.clone()));
        } else if list.rows.is_empty() {
            rows = rows.child(div().text_color(palette.secondary_text).child("No worlds."));
        }
        for (index, row) in list.rows.iter().enumerate() {
            let folder = row.folder().to_string();
            let (world, details) = match row {
                WorldRow::Java(world, details) => (Some(world), details.as_ref().ok()),
                WorldRow::Horizon(_) => (None, None),
            };
            let name =
                details.map_or_else(|| folder.clone(), |details| details.display_name.clone());
            let mut description = div().flex_1().min_w_0().flex().flex_col().gap_1().child(
                div()
                    .font_weight(FontWeight::BOLD)
                    .text_ellipsis()
                    .child(name.clone()),
            );
            if world.is_some() {
                description = description.child(
                    div()
                        .text_size(px(12.))
                        .text_color(palette.secondary_text)
                        .text_ellipsis()
                        .child(format!("Folder: {folder}")),
                );
                let detail =
                    details.map_or_else(|| "Could not read level.dat".into(), details_label);
                description = description.child(
                    div()
                        .text_size(px(12.))
                        .text_color(palette.secondary_text)
                        .child(detail),
                );
            }

            let join_world = world.cloned();
            let mut buttons = div().flex().flex_wrap().justify_end().gap_1().child(
                Button::new(("join-world", index))
                    .label("Join")
                    .disabled(running || busy)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.join_world(&folder, join_world.as_ref(), cx)
                    })),
            );
            if let Some(world) = world.cloned() {
                let editor_world = world.clone();
                let editor = prism_core::tools::Tool::MCEdit;
                buttons = buttons.child(
                    Button::new(("mcedit-world", index))
                        .label("MCEdit")
                        .disabled(running || busy)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let settings = prism_core::settings::Settings::load(
                                &this.app.read(cx).paths.global_config(),
                            );
                            if editor.check(&editor.path(&settings)).is_err() {
                                this.worlds.status = Some(
                                    "No MCEdit found or set up. Set the MCEdit path in Settings → External Tools.".into(),
                                );
                                cx.notify();
                                return;
                            }
                            let result = this.current_world_instance(cx).and_then(|instance| {
                                if instance.game_root() != editor_world.game_root() {
                                    return Err(io::Error::other("The instance game folder changed"));
                                }
                                let lock = crate::instance_lock::acquire(
                                    &this.app.read(cx).paths,
                                    &instance,
                                )
                                .map_err(|error| {
                                    if error.kind() == io::ErrorKind::WouldBlock {
                                        io::Error::new(
                                            error.kind(),
                                            "Minecraft is running for this instance; close it before editing worlds in MCEdit.",
                                        )
                                    } else {
                                        error
                                    }
                                })?;
                                drop(lock);
                                editor.open_world(&settings, &editor_world.world_path()?)
                            });
                            this.worlds.status = Some(match result {
                                Ok(()) => "MCEdit started".into(),
                                Err(error) => format!("Could not start MCEdit: {error}"),
                            });
                            cx.notify();
                        })),
                );
                let open_world = world.clone();
                buttons = buttons.child(
                    Button::new(("open-world", index))
                        .label("Open Folder")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.open_world_folder(Some(&open_world), cx)
                        })),
                );
                for action in [Action::Rename, Action::Copy, Action::Delete] {
                    let target = world.clone();
                    let name = name.clone();
                    buttons = buttons.child(
                        Button::new((action.label(), index))
                            .label(action.label())
                            .disabled(
                                running
                                    || busy
                                    || (details.is_none() && !matches!(action, Action::Delete)),
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.show_world_dialog(
                                    action,
                                    target.clone(),
                                    name.clone(),
                                    window,
                                    cx,
                                );
                            })),
                    );
                }
            }
            rows = rows.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .bg(palette.panel)
                    .rounded_md()
                    .child(description)
                    .child(buttons),
            );
        }

        div()
            .relative()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .min_h_0()
            .child(self.page_header(cx, list.game_root.join("saves").display().to_string()))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .child(
                        Button::new("refresh-worlds")
                            .label("Refresh")
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.worlds.status = None;
                                this.refresh_worlds(cx);
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("view-saves")
                            .label("View Saves Folder")
                            .on_click(
                                cx.listener(|this, _, _, cx| this.open_world_folder(None, cx)),
                            ),
                    ),
            )
            .children(
                self.worlds
                    .status
                    .as_ref()
                    .map(|status| div().px_3().text_size(px(13.)).child(status.clone())),
            )
            .child(
                div()
                    .id("worlds-scroll")
                    .flex_1()
                    .overflow_y_scroll()
                    .child(rows),
            )
            .children(
                self.worlds
                    .dialog
                    .as_ref()
                    .map(|dialog| self.render_world_dialog(dialog, running, cx)),
            )
            .into_any_element()
    }

    fn render_world_dialog(
        &self,
        dialog: &WorldDialog,
        running: bool,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let palette = crate::theme::Palette::get(cx);
        let action = dialog.action.label();
        div()
            .id("world-dialog-backdrop")
            .absolute()
            .size_full()
            .top_0()
            .left_0()
            .occlude()
            .bg(black().opacity(0.5))
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .w(px(380.))
                    .p_4()
                    .rounded_lg()
                    .border_1()
                    .border_color(palette.strong_border)
                    .bg(palette.panel)
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .text_size(px(16.))
                            .font_weight(FontWeight::BOLD)
                            .child(format!("{action} World")),
                    )
                    .child(match dialog.action {
                        Action::Delete => format!(
                            "Permanently delete {:?} ({})? This cannot be undone.",
                            dialog.name,
                            dialog.target.name()
                        ),
                        Action::Rename => format!(
                            "Enter a new name for {:?}. The save folder will also be renamed.",
                            dialog.name
                        ),
                        Action::Copy => format!("Enter a name for the copy of {:?}.", dialog.name),
                    })
                    .children(dialog.input.as_ref().map(Input::new))
                    .children(
                        dialog
                            .error
                            .map(|error| div().text_color(palette.error).child(error)),
                    )
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap_2()
                            .child(Button::new("cancel-world-action").label("Cancel").on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.worlds.dialog = None;
                                    cx.notify();
                                }),
                            ))
                            .child(
                                Button::new("confirm-world-action")
                                    .label(action)
                                    .disabled(running)
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.apply_world_action(cx)),
                                    ),
                            ),
                    ),
            )
    }
}

fn details_label(details: &WorldDetails) -> String {
    let seed = details
        .seed
        .map_or_else(|| "Unknown".into(), |seed| seed.to_string());
    let mode = match details.game_mode {
        Some(0) => "Survival".into(),
        Some(1) => "Creative".into(),
        Some(2) => "Adventure".into(),
        Some(3) => "Spectator".into(),
        Some(mode) => format!("Unknown ({mode})"),
        None => "Unknown".into(),
    };
    let played = details.last_played.map_or_else(
        || "Unknown".into(),
        |date| date.format("%Y-%m-%d %H:%M UTC").to_string(),
    );
    format!("{mode}  ·  Seed: {seed}  ·  Last played: {played}")
}
