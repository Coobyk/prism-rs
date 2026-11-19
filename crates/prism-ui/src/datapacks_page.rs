use super::{Browser, BrowserTarget, InstanceWindow, Page};
use gpui_kit::base::Disableable;
use gpui_kit::component::button::Button;
use gpui_kit::*;
use prism_core::datapacks::{DataPack, World};
use prism_core::modplatform::modrinth::{ModrinthApi, SearchResult};
use prism_core::modplatform::ResourceType;
use std::io;

const BROWSER: Browser = Browser::Modrinth(ResourceType::DataPack);

#[derive(Default)]
pub(super) struct DataPacksPage {
    selected: Option<String>,
    revision: u64,
    status: String,
}

#[derive(Clone)]
pub(super) struct PackTarget {
    pub(super) world: World,
    revision: u64,
}

impl DataPacksPage {
    fn select(&mut self, name: String) -> bool {
        if self.selected.as_ref() == Some(&name) {
            return false;
        }
        self.selected = Some(name);
        self.revision += 1;
        self.status.clear();
        true
    }

    fn target(&self, world: World) -> PackTarget {
        PackTarget {
            world,
            revision: self.revision,
        }
    }

    fn matches(&self, target: &PackTarget) -> bool {
        self.revision == target.revision && self.selected.as_deref() == Some(target.world.name())
    }
}

impl InstanceWindow {
    pub(super) fn datapack_target(&self, cx: &Context<Self>) -> Option<PackTarget> {
        let instance = self.app.read(cx).instances.get(&self.instance_id)?;
        if instance.is_horizon() {
            return None;
        }
        let world = World::open(&instance.game_root(), self.datapacks.selected.as_deref()?).ok()?;
        Some(self.datapacks.target(world))
    }

    fn edit_datapacks<T>(
        &self,
        target: &PackTarget,
        cx: &Context<Self>,
        edit: impl FnOnce(&World) -> io::Result<T>,
    ) -> io::Result<T> {
        if !self.datapacks.matches(target) {
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
            return Err(io::Error::other(
                "Stop Minecraft before changing data packs",
            ));
        }
        let _lock = crate::instance_lock::acquire(&app.paths, instance).map_err(|error| {
            if error.kind() == io::ErrorKind::WouldBlock {
                io::Error::other("The instance is running or another operation is in progress")
            } else {
                error
            }
        })?;
        edit(&target.world)
    }

    fn datapack_action(
        &mut self,
        target: &PackTarget,
        cx: &mut Context<Self>,
        edit: impl FnOnce(&World) -> io::Result<()>,
    ) {
        if self.datapacks.matches(target) {
            self.datapacks.status = match self.edit_datapacks(target, cx, edit) {
                Ok(()) => String::new(),
                Err(error) => error.to_string(),
            };
            cx.notify();
        }
    }

    fn choose_datapack(&mut self, target: PackTarget, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Add Data Pack ZIP".into()),
        });
        cx.spawn(async move |this, cx| {
            let result = paths.await;
            let _ = this.update(cx, |this, cx| {
                if !this.datapacks.matches(&target) {
                    return;
                }
                match result {
                    Ok(Ok(Some(paths))) => {
                        if let Some(path) = paths.first() {
                            this.datapack_action(&target, cx, |world| world.add_zip(path));
                        }
                    }
                    Ok(Ok(None)) => {}
                    Ok(Err(error)) => this.datapacks.status = error.to_string(),
                    Err(error) => this.datapacks.status = error.to_string(),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn confirm_datapack_removal(
        &mut self,
        target: PackTarget,
        pack: DataPack,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let answer = window.prompt(
            gpui::PromptLevel::Warning,
            &format!(
                "Remove {:?} from world {:?}?",
                pack.name(),
                target.world.name()
            ),
            Some("This cannot be undone."),
            &[
                gpui::PromptButton::cancel("Cancel"),
                gpui::PromptButton::new("Remove"),
            ],
            cx,
        );
        cx.spawn(async move |this, cx| {
            if answer.await == Ok(1) {
                let _ = this.update(cx, |this, cx| {
                    this.datapack_action(&target, cx, |world| world.remove(&pack));
                });
            }
        })
        .detach();
    }

    pub(super) fn install_datapack(
        &mut self,
        scope: PackTarget,
        project: SearchResult,
        cx: &mut Context<Self>,
    ) {
        if self
            .datapack_target(cx)
            .is_none_or(|target| !self.datapacks.matches(&scope) || target.world != scope.world)
        {
            return;
        }
        let app = self.app.read(cx);
        if app.is_running(&self.instance_id) {
            return;
        }
        let Some(instance) = app.instances.get(&self.instance_id) else {
            return;
        };
        let target = BrowserTarget::new(instance, app);
        let runtime = app.runtime.clone();
        let downloader = runtime.downloader.clone();
        let Some(browser) = self.browsers.get_mut(&BROWSER) else {
            return;
        };
        if browser.target.as_ref() != Some(&target) {
            browser.install_status =
                "The instance version or loader changed. Search again before installing.".into();
            cx.notify();
            return;
        }
        let project_id = project.project_id.clone();
        if !browser.installing.insert(project_id.clone()) {
            return;
        }
        browser.install_status.clear();
        let title = project.title.clone();
        let mc_version = target.mc_version.clone();
        let task = runtime.spawn(async move {
            ModrinthApi::with_client(downloader.client().clone())
                .download_datapack(&downloader, &project, &mc_version)
                .await
        });
        cx.spawn(async move |this, cx| {
            let result = task
                .await
                .map_err(|error| error.to_string())
                .and_then(|result| result.map_err(|error| error.to_string()));
            let _ = this.update(cx, |this, cx| {
                let status = this.datapacks.matches(&scope).then(|| {
                    let current = this
                        .app
                        .read(cx)
                        .instances
                        .get(&this.instance_id)
                        .map(|instance| BrowserTarget::new(instance, this.app.read(cx)));
                    let result = if current.as_ref() != Some(&target) {
                        Err("The instance changed during the download".into())
                    } else {
                        result.and_then(|pack| {
                            this.edit_datapacks(&scope, cx, |world| pack.install(world))
                                .map_err(|error| error.to_string())
                        })
                    };
                    match result {
                        Ok(()) => format!("Installed {title} in {}", scope.world.name()),
                        Err(error) => format!("Could not install {title}: {error}"),
                    }
                });
                if let Some(browser) = this.browsers.get_mut(&BROWSER) {
                    browser.installing.remove(&project_id);
                    if let Some(status) = status {
                        browser.install_status = status;
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn render_datapacks(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let app = self.app.read(cx);
        let mut page = div()
            .flex_1()
            .flex()
            .flex_col()
            .min_w_0()
            .min_h_0()
            .child(self.page_header(cx, "Data Packs · per world".into()));
        let Some(instance) = app
            .instances
            .get(&self.instance_id)
            .filter(|instance| !instance.is_horizon())
        else {
            return page
                .child(div().p_3().child("This Java instance no longer exists."))
                .into_any_element();
        };
        let running = app.is_running(&self.instance_id);
        let worlds = match World::list(&instance.game_root()) {
            Ok(worlds) if worlds.is_empty() => {
                return page
                    .child(
                        div()
                            .p_3()
                            .child("No saved worlds. Create a world in Minecraft first."),
                    )
                    .into_any_element();
            }
            Ok(worlds) => worlds,
            Err(error) => {
                return page
                    .child(div().p_3().child(format!("Could not list worlds: {error}")))
                    .into_any_element()
            }
        };
        let selected = worlds
            .iter()
            .find(|world| self.datapacks.selected.as_deref() == Some(world.name()));
        if running {
            page = page.child(
                div()
                    .px_3()
                    .py_1()
                    .text_color(rgb(0x8fb4d9))
                    .child("Minecraft is running; data packs are read-only."),
            );
        }
        let mut world_list = div()
            .id("datapack-worlds")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_1();
        for world in &worlds {
            let active = selected == Some(world);
            let name = world.name().to_owned();
            world_list = world_list.child(
                div()
                    .id(SharedString::from(format!("datapack-world-{name}")))
                    .flex_shrink_0()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .text_ellipsis()
                    .bg(if active { rgb(0x3d5a80) } else { rgb(0x2b2b2b) })
                    .hover(|style| style.bg(rgb(0x384b60)))
                    .child(name.clone())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.datapacks.select(name.clone()) {
                            if let Some(browser) = this.browsers.get_mut(&BROWSER) {
                                browser.install_status.clear();
                            }
                            cx.notify();
                        }
                    })),
            );
        }
        let worlds_panel = div()
            .w(px(170.))
            .flex_shrink_0()
            .min_h_0()
            .flex()
            .flex_col()
            .gap_2()
            .p_2()
            .border_r_1()
            .border_color(rgb(0x1c1c1c))
            .child(div().font_weight(FontWeight::BOLD).child("Worlds"))
            .child(world_list);
        let mut packs_panel = div()
            .min_w_0()
            .min_h_0()
            .flex_1()
            .flex()
            .flex_col()
            .gap_2()
            .p_2();
        if let Some(world) = selected {
            let scope = self.datapacks.target(world.clone());
            let add_scope = scope.clone();
            let folder_scope = scope.clone();
            packs_panel = packs_panel
                .child(
                    div()
                        .font_weight(FontWeight::BOLD)
                        .text_ellipsis()
                        .child(format!("Data Packs in {}", world.name())),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_2()
                        .child(
                            Button::new("add-datapack")
                                .label("Add ZIP…")
                                .disabled(running)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.choose_datapack(add_scope.clone(), cx)
                                })),
                        )
                        .child(
                            Button::new("open-datapack-folder")
                                .label("Open Folder")
                                .disabled(running)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.datapack_action(&folder_scope, cx, |world| {
                                        let folder = world.ensure_pack_folder()?;
                                        open_folder(&folder)
                                    });
                                })),
                        )
                        .child(
                            Button::new("browse-datapacks")
                                .label("Browse Modrinth")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.page = Page::BrowseDataPacks;
                                    cx.notify();
                                })),
                        ),
                );
            if !self.datapacks.status.is_empty() {
                packs_panel = packs_panel.child(
                    div()
                        .text_color(rgb(0xff9999))
                        .child(self.datapacks.status.clone()),
                );
            }
            match world.packs() {
                Ok(packs) if packs.is_empty() => {
                    packs_panel = packs_panel.child(
                        div()
                            .text_color(rgb(0xaaaaaa))
                            .child("No data packs installed in this world."),
                    );
                }
                Ok(packs) => {
                    let mut list = div()
                        .id("datapack-list")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .flex()
                        .flex_col()
                        .gap_1();
                    for pack in packs {
                        let name = pack.name().to_owned();
                        let enabled = pack.enabled();
                        let toggle = pack.clone();
                        let toggle_scope = scope.clone();
                        let remove_scope = scope.clone();
                        list = list.child(
                            div()
                                .flex()
                                .flex_shrink_0()
                                .items_center()
                                .gap_2()
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .bg(rgb(0x2b2b2b))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .flex()
                                        .flex_col()
                                        .child(div().text_ellipsis().child(name.clone()))
                                        .child(
                                            div()
                                                .text_size(px(11.))
                                                .text_color(rgb(0xaaaaaa))
                                                .child(if enabled {
                                                    "Enabled"
                                                } else {
                                                    "Disabled"
                                                }),
                                        ),
                                )
                                .child(
                                    Button::new(SharedString::from(format!(
                                        "toggle-datapack-{enabled}-{name}"
                                    )))
                                    .label(if enabled { "Disable" } else { "Enable" })
                                    .disabled(running)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.datapack_action(&toggle_scope, cx, |world| {
                                            world.toggle(&toggle)
                                        });
                                    })),
                                )
                                .child(
                                    Button::new(SharedString::from(format!(
                                        "remove-datapack-{enabled}-{name}"
                                    )))
                                    .label("Remove")
                                    .disabled(running)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.confirm_datapack_removal(
                                            remove_scope.clone(),
                                            pack.clone(),
                                            window,
                                            cx,
                                        );
                                    })),
                                ),
                        );
                    }
                    packs_panel = packs_panel.child(list);
                }
                Err(error) => {
                    packs_panel = packs_panel.child(
                        div()
                            .text_color(rgb(0xff9999))
                            .child(format!("Could not list data packs: {error}")),
                    );
                }
            }
        } else {
            packs_panel = packs_panel
                .child(div().child("Select a world to manage its installed data packs."));
        }
        page.child(
            div()
                .flex()
                .flex_1()
                .min_h_0()
                .child(worlds_panel)
                .child(packs_panel),
        )
        .into_any_element()
    }
}

fn open_folder(path: &std::path::Path) -> io::Result<()> {
    #[cfg(target_os = "windows")]
    let command = "explorer";
    #[cfg(target_os = "macos")]
    let command = "open";
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let command = "xdg-open";
    std::process::Command::new(command).arg(path).spawn()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{DataPacksPage, World};
    use std::fs;

    #[test]
    fn switching_worlds_invalidates_in_flight_actions_even_after_switching_back() {
        let root = tempfile::tempdir().unwrap();
        let a = root.path().join("saves/Alpha");
        fs::create_dir_all(&a).unwrap();
        fs::write(a.join("level.dat"), "alpha").unwrap();
        let world = World::open(root.path(), "Alpha").unwrap();
        let mut page = DataPacksPage::default();
        page.select("Alpha".into());
        let pending = page.target(world.clone());
        assert!(page.matches(&pending));
        page.select("Beta".into());
        assert!(!page.matches(&pending));
        page.select("Alpha".into());
        assert!(!page.matches(&pending));
        assert!(page.matches(&page.target(world)));
    }
}
