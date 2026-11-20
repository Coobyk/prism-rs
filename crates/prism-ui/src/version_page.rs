use super::{InstanceWindow, Page};
use crate::theme::Button;
use gpui_kit::base::Disableable;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::*;
use prism_core::{
    launch::Launcher,
    loaders::{self, Loader},
    profile::LaunchProfile,
    version_components::{version_id, ArchiveKind, Pack, MINECRAFT},
};
use std::path::PathBuf;

#[derive(Clone, Copy, PartialEq, Eq)]
enum LoaderKind {
    Fabric,
    Quilt,
    Forge,
    NeoForge,
}

impl LoaderKind {
    fn name(self) -> &'static str {
        match self {
            Self::Fabric => "Fabric",
            Self::Quilt => "Quilt",
            Self::Forge => "Forge",
            Self::NeoForge => "NeoForge",
        }
    }

    fn loader(self, version: String) -> Loader {
        match self {
            Self::Fabric => Loader::Fabric { version },
            Self::Quilt => Loader::Quilt { version },
            Self::Forge => Loader::Forge { version },
            Self::NeoForge => Loader::NeoForge { version },
        }
    }
}

#[derive(Clone, Copy)]
enum Edit {
    Minecraft,
    Lwjgl,
    SetLoader(LoaderKind),
    RemoveLoader,
    Add(ArchiveKind),
    Move(usize, isize),
    Remove(usize),
}

impl Edit {
    fn title(self) -> &'static str {
        match self {
            Self::Minecraft => "Change Minecraft version",
            Self::Lwjgl => "Change LWJGL version",
            Self::SetLoader(kind) => kind.name(),
            Self::RemoveLoader => "Remove loader?",
            Self::Add(ArchiveKind::JarMod) => "Add jar mod",
            Self::Add(ArchiveKind::Classpath) => "Add classpath JAR",
            Self::Move(..) => "Move",
            Self::Remove(_) => "Remove archive?",
        }
    }
}

struct Dialog {
    edit: Edit,
    input: Option<Entity<InputState>>,
    options: Vec<String>,
    note: String,
    id: u64,
}

#[derive(Default)]
pub(super) struct VersionPage {
    pack: Option<Pack>,
    busy: bool,
    message: String,
    dialog: Option<Dialog>,
    dialog_id: u64,
}

impl VersionPage {
    pub(super) fn on_navigation(&mut self) {
        self.dialog = None;
    }
}

impl InstanceWindow {
    pub(super) fn refresh_version(&mut self, cx: &mut Context<Self>) {
        if self.version_editor.busy {
            return;
        }
        let app = self.app.read(cx);
        let Some(instance) = app.instances.get(&self.instance_id) else {
            return;
        };
        let root = instance.root.clone();
        let task = app
            .runtime
            .handle()
            .spawn_blocking(move || Pack::load(&root));
        self.version_editor.busy = true;
        self.version_editor.pack = None;
        cx.spawn(async move |this, cx| {
            let result = task
                .await
                .map_err(|error| error.to_string())
                .and_then(|result| result.map_err(|error| error.to_string()));
            let _ = this.update(cx, |this, cx| {
                this.version_editor.busy = false;
                match result {
                    Ok(pack) => this.version_editor.pack = Some(pack),
                    Err(error) => {
                        this.version_editor.message = format!("Could not read components: {error}")
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn edit_version(&mut self, edit: Edit, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pack) = self.version_editor.pack.as_ref() else {
            return;
        };
        let mut note = String::new();
        let initial = match edit {
            Edit::Minecraft => pack.version(MINECRAFT),
            Edit::Lwjgl => {
                note = "Leave blank to use Minecraft’s default LWJGL version.".into();
                pack.lwjgl().map(|(_, version)| version).or(Some(""))
            }
            Edit::SetLoader(kind) => {
                note = "Enter the loader version for this Minecraft release.".into();
                pack.version(kind.loader(String::new()).uid()).or(Some(""))
            }
            Edit::Add(_) => {
                note =
                    "Choose a local archive or enter its absolute path. Only add files you trust."
                        .into();
                Some("")
            }
            Edit::Remove(index) => {
                note = format!(
                    "Remove {} from this instance? Its local copy will be deleted.",
                    pack.archives()[index].name
                );
                None
            }
            Edit::RemoveLoader => {
                note = "Remove the current loader from this instance? Installed mods will not be deleted.".into();
                None
            }
            Edit::Move(..) => return self.apply_version(edit, String::new(), cx),
        };
        let initial = initial.map(str::to_owned);
        self.version_editor.dialog_id += 1;
        let id = self.version_editor.dialog_id;
        self.version_editor.dialog = Some(Dialog {
            edit,
            note,
            id,
            options: Vec::new(),
            input: initial.map(|value| {
                cx.new(|cx| {
                    let mut input = InputState::new(window, cx);
                    input.set_value(value, window, cx);
                    input.focus(window, cx);
                    input
                })
            }),
        });
        if let Edit::SetLoader(kind @ (LoaderKind::Fabric | LoaderKind::Quilt)) = edit {
            let app = self.app.read(cx);
            let client = app.runtime.downloader.client();
            let version = self
                .version_editor
                .pack
                .as_ref()
                .unwrap()
                .version(MINECRAFT)
                .unwrap_or("1.20.1")
                .to_owned();
            let task = app.runtime.spawn(async move {
                loaders::loader_versions(&client, &kind.loader(String::new()), &version).await
            });
            cx.spawn(async move |this, cx| {
                let result = task.await;
                let _ = this.update(cx, |this, cx| {
                    if let Some(dialog) = this
                        .version_editor
                        .dialog
                        .as_mut()
                        .filter(|dialog| dialog.id == id)
                    {
                        if let Ok(Ok(options)) = result {
                            dialog.options = options.into_iter().take(8).collect();
                        }
                        cx.notify();
                    }
                });
            })
            .detach();
        }
        cx.notify();
    }

    fn browse_version_archive(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.version_editor.dialog.as_ref().map(|dialog| dialog.id) else {
            return;
        };
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Select archive".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(path) = paths.first() else { return };
            let _ = this.update(cx, |this, cx| {
                if let Some(dialog) = this
                    .version_editor
                    .dialog
                    .as_ref()
                    .filter(|dialog| dialog.id == id)
                {
                    let edit = dialog.edit;
                    this.apply_version(edit, path.to_string_lossy().into_owned(), cx);
                }
            });
        })
        .detach();
    }

    fn apply_version(&mut self, edit: Edit, text: String, cx: &mut Context<Self>) {
        if self.page != Page::Version || self.version_editor.busy {
            return;
        }
        let Some(pack) = self.version_editor.pack.clone() else {
            return;
        };
        let app = self.app.read(cx);
        let Some(instance) = app.instances.get(&self.instance_id).cloned() else {
            return;
        };
        let paths = app.paths.clone();
        let launcher = Launcher::new(paths.clone(), app.runtime.downloader.clone());
        let runtime = app.runtime.clone();
        let client = runtime.downloader.client();
        self.version_editor.dialog = None;
        self.version_editor.busy = true;
        self.version_editor.message.clear();
        let task = runtime.spawn(async move {
            let version = text.trim();
            let mut lwjgl = prism_core::version_components::LWJGL3;
            match edit {
                Edit::Minecraft => {
                    launcher
                        .fetch_version_data(version)
                        .await
                        .map_err(|error| error.to_string())?;
                }
                Edit::Lwjgl if !version.is_empty() => {
                    let mc = pack
                        .version(MINECRAFT)
                        .ok_or("Minecraft version not found")?;
                    let data = launcher
                        .fetch_version_data(mc)
                        .await
                        .map_err(|error| error.to_string())?;
                    let mut profile = LaunchProfile::from_mojang(&data);
                    lwjgl = profile.lwjgl_family().ok_or(
                        "This Minecraft version does not use a supported LWJGL component.",
                    )?;
                    let replacement = launcher
                        .fetch_lwjgl(lwjgl, version)
                        .await
                        .map_err(|error| error.to_string())?;
                    profile
                        .apply_lwjgl(&replacement)
                        .map_err(|error| error.to_string())?;
                }
                Edit::SetLoader(kind) => {
                    version_id(version).map_err(|error| error.to_string())?;
                    if matches!(kind, LoaderKind::Fabric | LoaderKind::Quilt) {
                        let minecraft = pack
                            .version(MINECRAFT)
                            .ok_or("Minecraft version not found")?;
                        loaders::fetch_loader_profile(
                            &client,
                            &kind.loader(version.into()),
                            minecraft,
                        )
                        .await?;
                    }
                }
                _ => {}
            }
            tokio::task::spawn_blocking(move || {
                let _lock = crate::instance_lock::acquire(&paths, &instance)?;
                let text = text.trim();
                match edit {
                    Edit::Minecraft => pack.set_minecraft(text),
                    Edit::Lwjgl => pack.set_lwjgl(lwjgl, (!text.is_empty()).then_some(text)),
                    Edit::SetLoader(kind) => pack.set_loader(&kind.loader(text.into())),
                    Edit::RemoveLoader => pack.set_loader(&Loader::None),
                    Edit::Add(kind) => {
                        let path = PathBuf::from(text);
                        if !path.is_absolute() {
                            return Err(std::io::Error::other(
                                "Enter the archive’s absolute path.",
                            ));
                        }
                        pack.add_archive(&path, kind)
                    }
                    Edit::Move(index, offset) => pack.move_archive(index, offset),
                    Edit::Remove(index) => pack.remove_archive(index),
                }
            })
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| error.to_string())
        });
        cx.spawn(async move |this, cx| {
            let result = task
                .await
                .map_err(|error| error.to_string())
                .and_then(|result| result);
            let _ = this.update(cx, |this, cx| {
                this.version_editor.busy = false;
                match result {
                    Ok(pack) => {
                        this.version_editor.pack = Some(pack);
                        this.version_editor.message = "Version changes saved.".into();
                    }
                    Err(error) => {
                        this.version_editor.message = format!("Could not save changes: {error}")
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn render_version_page(&self, cx: &mut Context<Self>) -> AnyElement {
        let palette = crate::theme::Palette::get(cx);
        let state = &self.version_editor;
        let blocked =
            state.busy || state.dialog.is_some() || self.app.read(cx).is_running(&self.instance_id);
        let list = div()
            .p_3()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                Button::new("version-refresh")
                    .label("Reload")
                    .disabled(state.busy || state.dialog.is_some())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.version_editor.message.clear();
                        this.refresh_version(cx);
                        cx.notify();
                    })),
            )
            .children(state.busy.then(|| div().child("Working…")))
            .children((!state.message.is_empty()).then(|| div().child(state.message.clone())))
            .children(state.pack.as_ref().map(|pack| {
                let action = |id: &'static str, label: &'static str, edit: Edit| {
                    Button::new(id).label(label).disabled(blocked).on_click(
                        cx.listener(move |this, _, window, cx| this.edit_version(edit, window, cx)),
                    )
                };
                let m = pack.version(MINECRAFT).unwrap_or("1.20.1");
                let lwjgl = pack
                    .lwjgl()
                    .map(|(_, version)| version)
                    .unwrap_or("Minecraft default");
                let mut rows = div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(row(palette, "Minecraft", m).child(action(
                        "version-minecraft",
                        "Change",
                        Edit::Minecraft,
                    )))
                    .child(row(palette, "LWJGL", lwjgl).child(action(
                        "version-lwjgl",
                        "Change",
                        Edit::Lwjgl,
                    )));
                let mut has_loader = false;
                for (uid, version) in pack.components() {
                    if matches!(
                        uid,
                        MINECRAFT
                            | prism_core::version_components::LWJGL
                            | prism_core::version_components::LWJGL3
                    ) {
                        continue;
                    }
                    let kind = [
                        LoaderKind::Fabric,
                        LoaderKind::Quilt,
                        LoaderKind::Forge,
                        LoaderKind::NeoForge,
                    ]
                    .into_iter()
                    .find(|kind| kind.loader(String::new()).uid() == uid);
                    has_loader |= kind.is_some();
                    rows = rows.child(row(
                        palette,
                        kind.map(LoaderKind::name).unwrap_or(uid),
                        version,
                    ));
                }
                rows = rows.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_2()
                        .child(action(
                            "version-fabric",
                            "Fabric",
                            Edit::SetLoader(LoaderKind::Fabric),
                        ))
                        .child(action(
                            "version-quilt",
                            "Quilt",
                            Edit::SetLoader(LoaderKind::Quilt),
                        ))
                        .child(action(
                            "version-forge",
                            "Forge",
                            Edit::SetLoader(LoaderKind::Forge),
                        ))
                        .child(action(
                            "version-neoforge",
                            "NeoForge",
                            Edit::SetLoader(LoaderKind::NeoForge),
                        ))
                        .children(has_loader.then(|| {
                            action("version-remove-loader", "Remove loader", Edit::RemoveLoader)
                        })),
                );
                for (kind, title, button, id, detail) in [
                    (
                        ArchiveKind::JarMod,
                        "Jar mods",
                        "Add jar mod",
                        "version-add-mod",
                        "Later jar mods override earlier ones.",
                    ),
                    (
                        ArchiveKind::Classpath,
                        "Extra classpath",
                        "Add classpath JAR",
                        "version-add-cp",
                        "JARs are appended in the order shown.",
                    ),
                ] {
                    rows = rows
                        .child(div().mt_3().font_weight(FontWeight::BOLD).child(title))
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(palette.secondary_text)
                                .child(detail),
                        )
                        .child(action(id, button, Edit::Add(kind)));
                    let entries: Vec<_> = pack
                        .archives()
                        .iter()
                        .enumerate()
                        .filter(|(_, entry)| entry.kind == kind)
                        .collect();
                    for (position, &(index, entry)) in entries.iter().enumerate() {
                        let control = |verb: &'static str, edit: Edit, disabled| {
                            Button::new((verb, index))
                                .label(verb)
                                .disabled(blocked || disabled)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.edit_version(edit, window, cx)
                                }))
                        };
                        rows = rows.child(
                            row(palette, &entry.name, "")
                                .child(control("Up", Edit::Move(index, -1), position == 0))
                                .child(control(
                                    "Down",
                                    Edit::Move(index, 1),
                                    position + 1 == entries.len(),
                                ))
                                .child(control("Remove", Edit::Remove(index), false)),
                        );
                    }
                }
                rows
            }));
        div()
            .relative()
            .flex_1()
            .flex()
            .flex_col()
            .min_h_0()
            .min_w_0()
            .child(self.page_header(cx, "Installed components and launch archives".into()))
            .child(
                div()
                    .id("version-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(list),
            )
            .children(
                state
                    .dialog
                    .as_ref()
                    .map(|dialog| self.version_dialog(dialog, cx)),
            )
            .into_any_element()
    }

    fn version_dialog(&self, dialog: &Dialog, cx: &Context<Self>) -> impl IntoElement {
        let palette = crate::theme::Palette::get(cx);
        let edit = dialog.edit;
        let input = dialog.input.clone();
        div()
            .id("version-dialog")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .occlude()
            .bg(black().opacity(0.5))
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
                    .child(div().font_weight(FontWeight::BOLD).child(edit.title()))
                    .children((!dialog.note.is_empty()).then(|| div().child(dialog.note.clone())))
                    .children(dialog.input.as_ref().map(Input::new))
                    .child(div().flex().flex_wrap().gap_1().children(
                        dialog.options.iter().enumerate().map(|(index, option)| {
                            let option = option.clone();
                            Button::new(("version-option", index))
                                .label(option.clone())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.apply_version(edit, option.clone(), cx)
                                }))
                        }),
                    ))
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap_2()
                            .children(matches!(edit, Edit::Add(_)).then(|| {
                                Button::new("version-browse").label("Browse…").on_click(
                                    cx.listener(|this, _, _, cx| this.browse_version_archive(cx)),
                                )
                            }))
                            .child(Button::new("version-cancel").label("Cancel").on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.version_editor.dialog = None;
                                    cx.notify();
                                }),
                            ))
                            .child(
                                Button::new("version-save")
                                    .label(match edit {
                                        Edit::Remove(_) | Edit::RemoveLoader => "Remove",
                                        Edit::Add(_) => "Add",
                                        _ => "Save",
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        let text = input
                                            .as_ref()
                                            .map(|input| input.read(cx).value().to_string())
                                            .unwrap_or_default();
                                        this.apply_version(edit, text, cx);
                                    })),
                            ),
                    ),
            )
    }
}

fn row(palette: crate::theme::Palette, name: &str, value: &str) -> Div {
    div()
        .flex()
        .items_center()
        .gap_2()
        .px_3()
        .py_2()
        .rounded_md()
        .bg(palette.panel)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_ellipsis()
                .child(name.to_owned()),
        )
        .child(div().text_color(palette.link).child(value.to_owned()))
}
