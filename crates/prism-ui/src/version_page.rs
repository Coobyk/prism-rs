use super::{InstanceWindow, Page};
use gpui_kit::base::Disableable;
use gpui_kit::component::{
    button::Button,
    input::{Input, InputState},
};
use gpui_kit::*;
use prism_core::{
    launch::Launcher,
    loaders::{self, Loader},
    profile::LaunchProfile,
    version_components::{version_id, ArchiveKind, Pack, MINECRAFT},
};
use std::{collections::BTreeMap, path::PathBuf};

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

#[derive(Clone, Copy)]
enum ComponentAction {
    Customize,
    Revert,
    Remove,
}

#[derive(Clone, Copy)]
enum ComponentFile {
    Json,
    ClientJar,
    Agent,
}

#[derive(Default)]
struct ComponentLabels {
    name: Option<String>,
    subtitle: Option<String>,
}

#[derive(Default)]
pub(super) struct VersionPage {
    pack: Option<Pack>,
    busy: bool,
    message: String,
    dialog: Option<Dialog>,
    dialog_id: u64,
    refresh_id: u64,
    customize_reason: Option<String>,
    confirmation: Option<(String, ComponentAction)>,
    component_labels: BTreeMap<String, ComponentLabels>,
}

impl VersionPage {
    fn finish_customize_check(&mut self, refresh_id: u64, version: &str, reason: Option<String>) -> bool {
        if self.refresh_id != refresh_id
            || self.pack.as_ref().is_none_or(|pack| pack.is_custom(MINECRAFT) || pack.version(MINECRAFT) != Some(version))
        {
            return false;
        }
        self.customize_reason = reason;
        true
    }

    fn install_pack(&mut self, pack: Pack, labels: BTreeMap<String, ComponentLabels>) -> Option<String> {
        let version = (!pack.is_custom(MINECRAFT)).then(|| pack.version(MINECRAFT).map(str::to_owned)).flatten();
        self.customize_reason = if pack.is_custom(MINECRAFT) {
            None
        } else if version.is_some() {
            Some("Checking Customize availability…".into())
        } else {
            Some("Minecraft version is missing.".into())
        };
        self.busy = false;
        self.pack = Some(pack);
        self.component_labels = labels;
        version
    }

    pub(super) fn on_navigation(&mut self) {
        self.dialog = None;
        self.confirmation = None;
    }
}

impl InstanceWindow {
    fn open_component_editor(&self, uid: Option<String>, cx: &mut Context<Self>) {
        let Some(pack) = self.version_editor.pack.clone() else {
            return;
        };
        let app = self.app.read(cx);
        let Some(instance) = app.instances.get(&self.instance_id).cloned() else {
            return;
        };
        if app.is_running(&self.instance_id) || self.version_editor.busy {
            return;
        }
        super::component_editor::open(
            cx.entity().downgrade(),
            self.app.clone(),
            instance,
            pack,
            uid,
            cx,
        );
    }

    fn component_action(
        &mut self,
        uid: String,
        action: ComponentAction,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !matches!(action, ComponentAction::Customize) {
            self.version_editor.confirmation = Some((uid, action));
            cx.notify();
            return;
        }
        self.apply_component_action(uid, action, cx);
    }

    fn apply_component_action(
        &mut self,
        uid: String,
        action: ComponentAction,
        cx: &mut Context<Self>,
    ) {
        if self.version_editor.busy || self.app.read(cx).is_running(&self.instance_id) {
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
        let runtime = app.runtime.clone();
        let launcher = Launcher::new(paths.clone(), runtime.downloader.clone());
        self.version_editor.busy = true;
        cx.spawn(async move |this, cx| {
            let task = runtime.spawn(async move {
                let doc = if matches!(action, ComponentAction::Customize) {
                    Some(
                        launcher
                            .component_document(&uid, pack.version(&uid).unwrap_or_default())
                            .await
                            .map_err(|e| e.to_string())?,
                    )
                } else {
                    None
                };
                tokio::task::spawn_blocking(move || {
                    let _lock = crate::instance_lock::acquire(&paths, &instance)?;
                    match action {
                        ComponentAction::Customize => pack.customize(&uid, doc.as_ref().unwrap()),
                        ComponentAction::Revert => pack.revert(&uid),
                        ComponentAction::Remove => pack.remove_component(&uid),
                    }
                })
                .await
                .map_err(|e| e.to_string())?
                .map_err(|e| e.to_string())
            });
            let result = task.await.map_err(|e| e.to_string()).and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                this.version_editor.busy = false;
                match result {
                    Ok(pack) => {
                        this.version_editor.pack = Some(pack);
                        this.version_editor.message = "Component changes saved.".into();
                        this.refresh_version(cx);
                    }
                    Err(error) => this.version_editor.message = error,
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn import_components(&mut self, kind: ComponentFile, cx: &mut Context<Self>) {
        if self.version_editor.busy || self.app.read(cx).is_running(&self.instance_id) {
            return;
        }
        let Some(mut pack) = self.version_editor.pack.clone() else {
            return;
        };
        let app = self.app.read(cx);
        let Some(instance) = app.instances.get(&self.instance_id).cloned() else {
            return;
        };
        let runtime = app.runtime.clone();
        let paths = app.paths.clone();
        let files = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: !matches!(kind, ComponentFile::ClientJar),
            prompt: Some(
                match kind {
                    ComponentFile::Json => "Import component JSON files",
                    ComponentFile::ClientJar => "Replace Minecraft.jar — select a trusted JAR",
                    ComponentFile::Agent => "Add trusted Java agents",
                }
                .into(),
            ),
        });
        self.version_editor.busy = true;
        cx.spawn(async move |this, cx| {
            let selected = files.await;
            let result = if let Ok(Ok(Some(files))) = selected {
                runtime
                    .handle()
                    .spawn_blocking(move || -> std::io::Result<_> {
                        let _lock = crate::instance_lock::acquire(&paths, &instance)?;
                        let mut failures = Vec::new();
                        let mut count = 0;
                        for file in files {
                            let result = match kind {
                                ComponentFile::Json => pack.clone().import_component(&file),
                                ComponentFile::ClientJar => pack.clone().install_custom_jar(&file),
                                ComponentFile::Agent => pack.clone().install_agent(&file),
                            };
                            match result {
                                Ok(saved) => {
                                    pack = saved;
                                    count += 1;
                                }
                                Err(error) => failures.push(format!("{}: {error}", file.display())),
                            }
                        }
                        Ok((
                            pack,
                            import_message(count, &failures),
                        ))
                    })
                    .await
                    .map_err(|e| e.to_string())
                    .and_then(|r| r.map_err(|e| e.to_string()))
            } else {
                Ok((pack, String::new()))
            };
            let _ = this.update(cx, |this, cx| {
                this.version_editor.busy = false;
                match result {
                    Ok((pack, message)) => {
                        this.version_editor.pack = Some(pack);
                        this.version_editor.message = message;
                        this.refresh_version(cx);
                    }
                    Err(error) => this.version_editor.message = error,
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn refresh_version(&mut self, cx: &mut Context<Self>) {
        if self.version_editor.busy {
            return;
        }
        let app = self.app.read(cx);
        let Some(instance) = app.instances.get(&self.instance_id) else {
            return;
        };
        let root = instance.root.clone();
        let load = app
            .runtime
            .handle()
            .spawn_blocking(move || {
                let pack = Pack::load(&root)?;
                let names = pack.components().filter_map(|(uid, _)| {
                    let patch = pack.patch(uid).ok().flatten()?;
                    Some((uid.to_owned(), ComponentLabels {
                        name: patch.0.get("name").and_then(|name| name.as_str()).map(str::to_owned),
                        subtitle: local_jar_subtitle(uid, &patch.0),
                    }))
                }).collect::<BTreeMap<_, _>>();
                Ok::<_, std::io::Error>((pack, names))
            });
        let launcher = Launcher::new(app.paths.clone(), app.runtime.downloader.clone());
        let runtime = app.runtime.handle();
        self.version_editor.busy = true;
        self.version_editor.refresh_id += 1;
        let refresh_id = self.version_editor.refresh_id;
        cx.spawn(async move |this, cx| {
            let result = load
                .await
                .map_err(|error| error.to_string())
                .and_then(|result| result.map_err(|error| error.to_string()));
            let version = this.update(cx, |this, cx| {
                if this.version_editor.refresh_id != refresh_id {
                    return None;
                }
                this.version_editor.busy = false;
                let version = match result {
                    Ok((pack, names)) => this.version_editor.install_pack(pack, names),
                    Err(error) => {
                        this.version_editor.message = format!("Could not read components: {error}");
                        None
                    }
                };
                cx.notify();
                version
            }).ok().flatten();
            let Some(version) = version else { return; };
            let checked_version = version.clone();
            let reason = runtime.spawn(async move {
                launcher.component_document(MINECRAFT, &checked_version).await.err().map(|error| error.to_string())
            }).await.unwrap_or_else(|error| Some(error.to_string()));
            let _ = this.update(cx, |this, cx| {
                if this.version_editor.finish_customize_check(refresh_id, &version, reason) {
                    cx.notify();
                }
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
            Edit::Minecraft => {
                if pack.is_custom(MINECRAFT) {
                    note = "Changing Minecraft version discards its customized component patch.".into();
                }
                pack.version(MINECRAFT)
            }
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
                        if matches!(edit, Edit::Minecraft) {
                            this.refresh_version(cx);
                        }
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
                let mut components: Vec<_> = pack.components().collect();
                if pack.lwjgl().is_none() {
                    components.insert(components.len().min(1), ("", "Minecraft default"));
                }
                let mut rows = div().flex().flex_col().gap_2();
                for (index, (uid, version)) in components.into_iter().enumerate() {
                    let kind = [
                        LoaderKind::Fabric,
                        LoaderKind::Quilt,
                        LoaderKind::Forge,
                        LoaderKind::NeoForge,
                    ]
                    .into_iter()
                    .find(|kind| kind.loader(String::new()).uid() == uid);
                    let lwjgl = uid.is_empty() || matches!(uid,
                        prism_core::version_components::LWJGL | prism_core::version_components::LWJGL3);
                    let fallback = if uid == MINECRAFT { "Minecraft" } else if lwjgl { "LWJGL" } else { kind.map(LoaderKind::name).unwrap_or(uid) };
                    let labels = state.component_labels.get(uid);
                    let name = labels.and_then(|labels| labels.name.as_deref()).filter(|name| !name.trim().is_empty()).unwrap_or(fallback);
                    let subtitle = labels.and_then(|labels| labels.subtitle.as_deref()).unwrap_or(uid);
                    let custom = pack.is_custom(uid);
                    let reason = if uid == MINECRAFT { state.customize_reason.clone() } else { Some("Equivalent component JSON unavailable".into()) };
                    let mut controls = component_row(name, subtitle, version, custom,
                        (!custom && !lwjgl).then_some(reason.as_deref()).flatten());
                    let edit = if uid == MINECRAFT { Some(Edit::Minecraft) } else if lwjgl { Some(Edit::Lwjgl) } else { kind.map(Edit::SetLoader) };
                    if let Some(edit) = edit {
                        controls = controls.child(Button::new(("component-change", index)).label("Change")
                            .disabled(blocked).on_click(cx.listener(move |this, _, window, cx| this.edit_version(edit, window, cx))));
                    }
                    if custom {
                        let edit_uid = uid.to_owned();
                        controls = controls.child(Button::new(("component-edit", index)).label("Edit").disabled(blocked)
                            .on_click(cx.listener(move |this, _, _, cx| this.open_component_editor(Some(edit_uid.clone()), cx))));
                        let meta = pack.has_meta(uid);
                        let uid = uid.to_owned();
                        controls = controls.child(Button::new(("component-remove", index)).label(if meta { "Revert" } else { "Remove" }).disabled(blocked)
                            .on_click(cx.listener(move |this, _, window, cx| this.component_action(uid.clone(),
                                if meta { ComponentAction::Revert } else { ComponentAction::Remove }, window, cx))));
                    } else if !lwjgl {
                        let uid = uid.to_owned();
                        controls = controls.child(Button::new(("component-customize", index)).label("Customize")
                            .disabled(blocked || reason.is_some()).on_click(cx.listener(move |this, _, window, cx|
                                this.component_action(uid.clone(), ComponentAction::Customize, window, cx))));
                        if kind.is_some() {
                            controls = controls.child(Button::new(("loader-remove", index)).label("Remove").disabled(blocked)
                                .on_click(cx.listener(|this, _, window, cx| this.edit_version(Edit::RemoveLoader, window, cx))));
                        }
                    }
                    rows = rows.child(controls);
                }
                rows = rows.child(div().flex().gap_2().mt_1()
                    .child(Button::new("component-import").label("Import components").disabled(blocked)
                        .on_click(cx.listener(|this, _, _, cx| this.import_components(ComponentFile::Json, cx))))
                    .child(Button::new("component-empty").label("Add empty").disabled(blocked)
                        .on_click(cx.listener(|this, _, _, cx| this.open_component_editor(None, cx))))
                    .child(Button::new("component-client").label("Replace Minecraft.jar").disabled(blocked)
                        .on_click(cx.listener(|this, _, _, cx| this.import_components(ComponentFile::ClientJar, cx))))
                    .child(Button::new("component-agents").label("Add agents").disabled(blocked)
                        .on_click(cx.listener(|this, _, _, cx| this.import_components(ComponentFile::Agent, cx)))));
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
                        ,
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
                                .text_color(rgb(0xaaaaaa))
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
                            row(&entry.name, "")
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
            .children(state.confirmation.as_ref().map(|(_, action)| {
                let revert = matches!(action, ComponentAction::Revert);
                div().absolute().inset_0().size_full().occlude()
                    .bg(black().opacity(0.5)).flex().items_center().justify_center()
                    .child(div().w(px(410.)).p_4().rounded_lg().bg(rgb(0x2b2b2b))
                        .border_1().border_color(rgb(0x555555)).flex().flex_col().gap_3()
                        .child(div().font_weight(FontWeight::BOLD).child(if revert { "Revert component?" } else { "Remove component?" }))
                        .child(if revert { "This permanently deletes the local patch and restores the metadata component." } else { "This permanently deletes the component patch and its unshared local artifacts." })
                        .child(div().flex().justify_end().gap_2()
                            .child(Button::new("component-confirm-cancel").label("Cancel")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.version_editor.confirmation = None;
                                    cx.notify();
                                })))
                            .child(Button::new("component-confirm-apply").label(if revert { "Revert" } else { "Remove" })
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if let Some((uid, action)) = this.version_editor.confirmation.take() {
                                        this.apply_component_action(uid, action, cx);
                                    }
                                    cx.notify();
                                })))))
            }))
            .into_any_element()
    }

    fn version_dialog(&self, dialog: &Dialog, cx: &Context<Self>) -> impl IntoElement {
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
                    .bg(rgb(0x2b2b2b))
                    .border_1()
                    .border_color(rgb(0x555555))
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

fn import_message(count: usize, failures: &[String]) -> String {
    let suffix = if count == 1 { "" } else { "s" };
    let mut message = format!("Imported {count} component{suffix}.");
    if !failures.is_empty() {
        message.push(' ');
        message.push_str(&failures.join("; "));
    }
    message
}

fn local_jar_subtitle(uid: &str, document: &serde_json::Value) -> Option<String> {
    let artifact = if uid.starts_with("custom.agent.") {
        document.get("+agents")?.as_array()?.first()?
    } else if uid == "customjar" {
        document.get("mainJar")?
    } else {
        return None;
    };
    let name = ["MMC-displayname", "MMC-filename"].into_iter().find_map(|key| {
        artifact.get(key)?.as_str().filter(|name| !name.trim().is_empty())
    })?;
    Some(if name.to_ascii_lowercase().ends_with(".jar") {
        name.to_owned()
    } else {
        format!("{name}.jar")
    })
}

fn component_row(name: &str, subtitle: &str, version: &str, custom: bool, reason: Option<&str>) -> Div {
    div().flex().items_center().gap_2().px_3().py_2().rounded_md().bg(rgb(0x2b2b2b))
        .child(div().flex_1().min_w_0().flex().flex_col().gap_1()
            .child(div().flex().items_center().gap_2()
                .child(div().min_w_0().text_ellipsis().child(name.to_owned()))
                .children(custom.then(|| div().text_size(px(11.)).text_color(rgb(0xaaaaaa)).child("custom"))))
            .child(div().text_size(px(12.)).text_color(rgb(0xaaaaaa)).text_ellipsis()
                .child(if subtitle.is_empty() { "Provided by Minecraft".to_owned() } else { subtitle.to_owned() }))
            .children(reason.map(|reason| div().text_size(px(11.)).text_color(rgb(0xaaaaaa)).text_ellipsis().child(reason.to_owned()))))
        .child(div().flex_shrink_0().text_color(rgb(0x8fb4d9)).child(version.to_owned()))
}

fn row(name: &str, value: &str) -> Div {
    div()
        .flex()
        .items_center()
        .gap_2()
        .px_3()
        .py_2()
        .rounded_md()
        .bg(rgb(0x2b2b2b))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_ellipsis()
                .child(name.to_owned()),
        )
        .child(div().text_color(rgb(0x8fb4d9)).child(value.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::{import_message, local_jar_subtitle, Pack, VersionPage, MINECRAFT};
    use serde_json::json;

    #[test]
    fn local_components_are_usable_before_customize_metadata_completes() {
        let root = tempfile::tempdir().unwrap();
        let paths = prism_core::Paths { root: root.path().into() };
        let instance = prism_core::InstanceList::create(&paths, "Version", "default", "1.20.1").unwrap();
        let pack = Pack::load(&instance.root).unwrap();
        let mut state = VersionPage::default();
        state.busy = true;
        assert_eq!(state.install_pack(pack, Default::default()).as_deref(), Some("1.20.1"));
        assert!(!state.busy);
        assert!(state.pack.as_ref().unwrap().components().next().is_some());
        assert_eq!(state.customize_reason.as_deref(), Some("Checking Customize availability…"));
        let custom = state.pack.take().unwrap().customize(MINECRAFT,
            &json!({"uid":MINECRAFT,"formatVersion":1,"mainClass":"Main"})).unwrap();
        assert!(state.install_pack(custom, Default::default()).is_none());
        assert!(state.customize_reason.is_none());
    }

    #[test]
    fn customize_availability_survives_a_local_edit_in_progress() {
        let root = tempfile::tempdir().unwrap();
        let paths = prism_core::Paths { root: root.path().into() };
        let instance = prism_core::InstanceList::create(&paths, "Version", "default", "1.20.1").unwrap();
        let mut state = VersionPage::default();
        state.install_pack(Pack::load(&instance.root).unwrap(), Default::default());
        state.refresh_id = 1;
        state.busy = true;
        assert!(state.finish_customize_check(1, "1.20.1", None));
        assert!(state.busy);
        state.pack = Some(Pack::load(&instance.root).unwrap());
        state.busy = false;
        assert!(state.customize_reason.is_none());
        state.busy = true;
        assert!(state.finish_customize_check(1, "1.20.1", Some("Metadata unavailable".into())));
        state.busy = false;
        assert_eq!(state.customize_reason.as_deref(), Some("Metadata unavailable"));
        assert!(!state.finish_customize_check(0, "1.20.1", None));
        assert!(!state.finish_customize_check(1, "1.20.4", None));
        assert_eq!(state.customize_reason.as_deref(), Some("Metadata unavailable"));
        state.pack = None;
        assert!(!state.finish_customize_check(1, "1.20.1", None));
    }

    #[test]
    fn import_counts_and_failures() {
        assert_eq!(import_message(0, &[]), "Imported 0 components.");
        assert_eq!(import_message(1, &[]), "Imported 1 component.");
        assert_eq!(import_message(3, &[]), "Imported 3 components.");
        assert_eq!(import_message(0, &["bad.json: invalid UID".into()]),
            "Imported 0 components. bad.json: invalid UID");
    }

    #[test]
    fn local_jar_subtitles_prefer_display_names() {
        let document = json!({"+agents": [{"MMC-displayname": "proof-agent", "MMC-filename": "uuid.jar"}],
            "mainJar": {"MMC-displayname": "vanilla-client.jar"}});
        assert_eq!(local_jar_subtitle("custom.agent.uuid", &document).as_deref(), Some("proof-agent.jar"));
        assert_eq!(local_jar_subtitle("customjar", &document).as_deref(), Some("vanilla-client.jar"));
        assert_eq!(local_jar_subtitle("net.minecraft", &document), None);
    }

    #[test]
    fn local_jar_subtitles_handle_missing_or_invalid_names() {
        let uid = "custom.agent.uuid";
        assert_eq!(local_jar_subtitle(uid, &json!({"+agents": [{"MMC-displayname": "", "MMC-filename": "original.JAR"}]})).as_deref(), Some("original.JAR"));
        for document in [json!({}), json!({"+agents": []}), json!({"+agents": "invalid"}),
            json!({"+agents": [{"MMC-displayname": 3}]}), json!({"+agents": [{}]})] {
            assert_eq!(local_jar_subtitle(uid, &document), None);
        }
    }
}
