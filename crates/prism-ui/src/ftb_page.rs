use crate::state::App as LauncherApp;
use gpui_kit::base::Disableable;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::ActiveTheme;
use gpui_kit::*;
use prism_core::import::ImportOutcome;
use prism_core::import_ftb::AppInstance;
use prism_core::modplatform::ftb::{filter_catalog, FtbApi, Pack, Sort};
use std::collections::HashMap;
use std::path::PathBuf;

pub(crate) fn buttons(cx: &mut Context<crate::main_window::MainWindow>) -> impl IntoElement {
    div()
        .flex()
        .gap_2()
        .child(
            Button::new("open-ftb")
                .label("FTB Modpacks")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.create_dialog = None;
                    this.prompt = None;
                    open(this.app.clone(), false, cx);
                    cx.notify();
                })),
        )
        .child(
            Button::new("open-ftb-app")
                .label("FTB App Import")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.create_dialog = None;
                    this.prompt = None;
                    open(this.app.clone(), true, cx);
                    cx.notify();
                })),
        )
}

pub(crate) fn open(app: Entity<LauncherApp>, local: bool, cx: &mut gpui::App) {
    let options = WindowOptions {
        titlebar: Some(TitlebarOptions {
            title: Some(
                if local {
                    "FTB App Import"
                } else {
                    "FTB Modpacks"
                }
                .into(),
            ),
            ..Default::default()
        }),
        window_min_size: Some(Size {
            width: px(780.),
            height: px(620.),
        }),
        ..WindowOptions::default()
    };
    open_window(options, cx, |window, cx| {
        cx.new(|cx| FtbPage::new(app, local, window, cx))
    })
    .ok();
}

struct FtbPage {
    app: Entity<LauncherApp>,
    api: FtbApi,
    local: bool,
    search: Entity<InputState>,
    root: Entity<InputState>,
    packs: Vec<Pack>,
    artwork: HashMap<u64, PathBuf>,
    instances: Vec<AppInstance>,
    selected: Option<u64>,
    version: Option<u64>,
    selected_local: Option<PathBuf>,
    sort: Sort,
    busy: bool,
    status: String,
}

fn plain(value: &str) -> String {
    let mut text = String::new();
    let mut tag = false;
    for ch in value.chars() {
        match ch {
            '<' => {
                tag = true;
                text.push(' ');
            }
            '>' => tag = false,
            _ if !tag => text.push(ch),
            _ => {}
        }
    }
    let text = text
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ");
    let mut text = text
        .replace("**", "")
        .replace("__", "")
        .replace("~~", "")
        .replace('`', "");
    while let Some(start) = text.find('[') {
        let Some(end) = text[start..].find("](").map(|n| start + n) else {
            break;
        };
        let Some(close) = text[end + 2..].find(')').map(|n| end + 2 + n) else {
            break;
        };
        let label = text[start + 1..end].to_owned();
        text.replace_range(start..=close, &label);
    }
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

impl FtbPage {
    fn new(
        app: Entity<LauncherApp>,
        local: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search by name…"));
        cx.subscribe(&search, |_, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        })
        .detach();
        let root = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("FTB App instances directory")
                .default_value(
                    prism_core::import_ftb::discover_root()
                        .map(|p| p.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                )
        });
        let api = FtbApi::new(app.read(cx).runtime.downloader.clone());
        let mut page = Self {
            app,
            api,
            local,
            search,
            root,
            packs: Vec::new(),
            artwork: HashMap::new(),
            instances: Vec::new(),
            selected: None,
            version: None,
            selected_local: None,
            sort: Sort::Plays,
            busy: false,
            status: String::new(),
        };
        page.load(cx);
        page
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.status = if self.local {
            "Scanning instances…"
        } else {
            "Loading FTB catalog…"
        }
        .into();
        self.selected = None;
        self.selected_local = None;
        let runtime = self.app.read(cx).runtime.clone();
        let local = self.local;
        let root = PathBuf::from(self.root.read(cx).value().as_str());
        let api = self.api.clone();
        let task = runtime.spawn(async move {
            if local {
                let result =
                    tokio::task::spawn_blocking(move || prism_core::import_ftb::scan(&root))
                        .await
                        .map_err(|e| e.to_string())?
                        .map_err(|e| e.to_string())?;
                Ok::<_, String>((Vec::new(), result))
            } else {
                Ok((api.catalog().await.map_err(|e| e.to_string())?, Vec::new()))
            }
        });
        cx.spawn(async move |this, cx| {
            let result = task.await.map_err(|e| e.to_string()).and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok((packs, instances)) => {
                        this.status = if packs.is_empty() && instances.is_empty() {
                            "No instances or packs found.".into()
                        } else if this.local {
                            "Select an instance to import.".into()
                        } else {
                            "Select a pack to import.".into()
                        };
                        this.packs = packs;
                        this.instances = instances;
                        this.load_artwork(cx);
                    }
                    Err(error) => this.status = format!("Could not load FTB: {error}"),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn load_artwork(&mut self, cx: &mut Context<Self>) {
        let app = self.app.read(cx);
        let cache = app.paths.cache_dir().join("ftb-artwork");
        let images: HashMap<_, _> = self
            .packs
            .iter()
            .filter_map(|pack| {
                pack.art.iter().find(|art| art.kind == "square").map(|art| {
                    (
                        pack.id,
                        (art.url.clone(), cache.join(format!("{}.png", pack.id))),
                    )
                })
            })
            .collect();
        if images.is_empty() {
            return;
        }
        let downloader = app.runtime.downloader.clone();
        let task = app.runtime.spawn(async move {
            let requests = images
                .values()
                .map(|(url, path)| prism_core::net::DownloadRequest::new(url, path.clone()))
                .collect();
            let _ = downloader.download_all(requests, 3, |_| {}).await;
            images
                .into_iter()
                .filter_map(|(id, (_, path))| path.is_file().then_some((id, path)))
                .collect()
        });
        cx.spawn(async move |this, cx| {
            if let Ok(images) = task.await {
                let _ = this.update(cx, |this, cx| {
                    this.artwork = images;
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn choose_directory(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let answer = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Select FTB App instances directory".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = answer.await;
            let _ = this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(Ok(Some(paths))) => {
                        if let Some(path) = paths.first() {
                            this.root.update(cx, |state, cx| {
                                state.set_value(path.to_string_lossy().into_owned(), window, cx)
                            });
                            this.load(cx);
                        }
                    }
                    Ok(Ok(None)) => {}
                    _ => {
                        this.status =
                            "Could not open directory picker; enter a path and click Scan.".into()
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn install(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let source = if self.local {
            let Some(path) = &self.selected_local else {
                return;
            };
            path.to_string_lossy().into_owned()
        } else {
            let (Some(pack), Some(version)) = (self.selected, self.version) else {
                return;
            };
            format!("ftb:{pack}:{version}")
        };
        self.busy = true;
        self.status =
            "Importing… downloading and verifying files. This may take several minutes.".into();
        let app = self.app.read(cx);
        let paths = app.paths.clone();
        let downloader = app.runtime.downloader.clone();
        let (progress, mut updates) = tokio::sync::mpsc::unbounded_channel();
        let api = self.api.clone().with_progress(move |message| {
            let _ = progress.send(message);
        });
        let task = app.runtime.spawn(async move {
            if let Some(source) = prism_core::modplatform::ftb::Source::parse(&source)? {
                api.install(&paths, source.pack, source.version).await
            } else {
                drop(api);
                prism_core::import::import_source(&paths, &source, &downloader).await
            }
        });
        cx.spawn(async move |this, cx| {
            while let Some(message) = updates.recv().await {
                let _ = this.update(cx, |this, cx| {
                    if this.busy {
                        this.status = message;
                        cx.notify();
                    }
                });
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            let result = task.await.map_err(|e| e.to_string()).and_then(|r| r.map_err(|e| e.to_string()));
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                this.status = match result {
                    Ok(ImportOutcome { instance, missing_files, .. }) => {
                        let message = if missing_files.is_empty() { format!("Imported: {}", instance.name) } else {
                            format!("Imported: {} — {} manual downloads required; see curseforge-missing-files.txt in the instance folder.", instance.name, missing_files.len())
                        };
                        this.app.update(cx, |app, cx| {
                            app.reload_instances();
                            app.selected = Some(instance.id);
                            app.toast = Some((message.clone(), std::time::Instant::now()));
                            cx.notify();
                        });
                        message
                    }
                    Err(error) => format!("Import failed: {error}"),
                };
                cx.notify();
            });
        }).detach();
        cx.notify();
    }
}

impl Render for FtbPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut panel = div()
            .size_full()
            .p_5()
            .flex()
            .flex_col()
            .gap_4()
            .bg(rgb(0x242424))
            .text_color(rgb(0xeeeeee))
            .font_family(cx.theme().font_family.clone())
            .text_size(px(13.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(24.))
                            .font_weight(FontWeight::BOLD)
                            .child(if self.local {
                                "FTB App Import"
                            } else {
                                "FTB Modpacks"
                            }),
                    )
                    .child(muted(if self.local {
                        "Bring your worlds, settings and playtime into Prism."
                    } else {
                        "Discover your next adventure. Install a pack directly from Feed The Beast."
                    })),
            );
        if self.local {
            panel = panel.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(muted("FTB APP INSTANCES DIRECTORY"))
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .items_center()
                            .child(div().flex_1().min_w_0().child(Input::new(&self.root)))
                            .child(
                                Button::new("ftb-directory")
                                    .label("Choose…")
                                    .disabled(self.busy)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.choose_directory(window, cx)
                                    })),
                            )
                            .child(
                                Button::new("ftb-scan")
                                    .label("Rescan")
                                    .disabled(self.busy)
                                    .on_click(cx.listener(|this, _, _, cx| this.load(cx))),
                            ),
                    ),
            );
        } else {
            let mut sorts = div().flex().gap_2().items_center().child(muted("Sort by"));
            for (sort, label, id) in [
                (Sort::Plays, "Plays", "ftb-plays"),
                (Sort::Installs, "Installs", "ftb-installs"),
                (Sort::Name, "Name", "ftb-name"),
            ] {
                let button = Button::new(id).label(label);
                sorts = sorts.child(
                    (if self.sort == sort {
                        button.primary()
                    } else {
                        button
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.sort = sort;
                        cx.notify();
                    })),
                );
            }
            panel = panel.child(
                div()
                    .flex()
                    .gap_3()
                    .items_center()
                    .child(div().flex_1().min_w_0().child(Input::new(&self.search)))
                    .child(sorts)
                    .child(
                        Button::new("ftb-retry")
                            .label(if self.busy && self.packs.is_empty() {
                                "Loading…"
                            } else {
                                "Refresh"
                            })
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, _, cx| this.load(cx))),
                    ),
            );
        }
        if self.local {
            panel = panel.child(Input::new(&self.search));
        }
        let query = self.search.read(cx).value().to_lowercase();
        let mut list = div()
            .id("ftb-list")
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .w(px(310.))
            .flex_shrink_0()
            .h_full()
            .rounded_md()
            .bg(rgb(0x202020))
            .overflow_y_scroll();
        if self.local {
            for (i, instance) in self
                .instances
                .iter()
                .enumerate()
                .filter(|(_, i)| i.name.to_lowercase().contains(query.trim()))
            {
                let path = instance.path.clone();
                list = list.child(
                    row(
                        ("ftb-local", i),
                        self.selected_local.as_ref() == Some(&path),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .min_w_0()
                            .child(
                                div()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_ellipsis()
                                    .child(plain(&instance.name)),
                            )
                            .child(muted(format!(
                                "Minecraft {} · {}",
                                instance.minecraft,
                                local_loader(instance)
                            ))),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.busy {
                            this.selected_local = Some(path.clone());
                            this.status = "Ready to import into a new instance.".into();
                        }
                        cx.notify();
                    })),
                );
            }
        } else {
            for pack in filter_catalog(&self.packs, &query, self.sort) {
                let id = pack.id;
                let icon =
                    div()
                        .size(px(42.))
                        .flex_shrink_0()
                        .rounded_md()
                        .overflow_hidden()
                        .bg(rgb(0x343434))
                        .children(self.artwork.get(&id).map(|path| {
                            img(path.clone()).size_full().object_fit(ObjectFit::Cover)
                        }));
                list = list.child(
                    row(("ftb-pack", id), self.selected == Some(id))
                        .child(icon)
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(
                                    div()
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_ellipsis()
                                        .child(plain(&pack.name)),
                                )
                                .child(muted(format!("{} plays", count(pack.plays)))),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if this.busy {
                                return;
                            }
                            this.selected = Some(id);
                            this.status =
                                "Choose a version, then install into a new instance.".into();
                            this.version = None;
                            cx.notify();
                        })),
                );
            }
        }
        let mut detail = div()
            .id("ftb-detail")
            .flex_1()
            .min_w_0()
            .h_full()
            .p_4()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_4();
        if let Some(pack) = self.packs.iter().find(|p| Some(p.id) == self.selected) {
            detail = detail
                .child(
                    div()
                        .text_size(px(24.))
                        .font_weight(FontWeight::BOLD)
                        .whitespace_normal()
                        .child(plain(&pack.name)),
                )
                .child(
                    div()
                        .w_full()
                        .whitespace_normal()
                        .child(plain(&pack.synopsis)),
                )
                .child(muted(format!(
                    "By {}",
                    pack.authors
                        .iter()
                        .map(|a| plain(&a.name))
                        .collect::<Vec<_>>()
                        .join(", ")
                )))
                .child(div().text_color(rgb(0xc5d7e8)).child(format!(
                    "{} plays  ·  {} installs",
                    count(pack.plays),
                    count(pack.installs)
                )));
            if !pack.tags.is_empty() {
                detail = detail.child(div().flex().flex_wrap().gap_2().children(
                    pack.tags.iter().map(|tag| {
                        div()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .bg(rgb(0x30363c))
                            .text_size(px(11.))
                            .text_color(rgb(0xb4c6d6))
                            .child(plain(&tag.name))
                    }),
                ));
            }
            detail = detail.child(
                div()
                    .mt_2()
                    .font_weight(FontWeight::BOLD)
                    .child("Choose a version"),
            );
            let mut versions: Vec<_> = pack.versions.iter().collect();
            versions.sort_by_key(|v| std::cmp::Reverse((v.updated, v.id)));
            let mut picker = div()
                .id("ftb-versions")
                .h(px(208.))
                .flex_shrink_0()
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap_1()
                .p_1()
                .rounded_md()
                .bg(rgb(0x202020));
            for version in versions {
                let id = version.id;
                picker = picker.child(
                    row(("ftb-version", id), self.version == Some(id))
                        .py_2()
                        .child(
                            div()
                                .w(px(80.))
                                .flex_shrink_0()
                                .font_weight(FontWeight::MEDIUM)
                                .child(plain(&version.name)),
                        )
                        .child(div().w(px(64.)).flex_shrink_0().child(muted(
                            if version.release_type.is_empty() {
                                "Release".into()
                            } else {
                                plain(&version.release_type)
                            },
                        )))
                        .child(muted(target_label(&version.targets)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if !this.busy {
                                this.version = Some(id);
                                this.status = "Ready to install into a new instance.".into();
                            }
                            cx.notify();
                        })),
                );
            }
            detail = detail.child(picker).child(muted(
                "A new instance will be created. Your existing instances are not changed.",
            ));
        }
        if let Some(instance) = self
            .instances
            .iter()
            .find(|i| Some(&i.path) == self.selected_local.as_ref())
        {
            detail = detail.child(div().text_size(px(24.)).font_weight(FontWeight::BOLD).whitespace_normal().child(plain(&instance.name)))
                .child(format!("Minecraft {}  ·  Pack version {}", instance.minecraft, instance.version))
                .child(muted(local_loader(instance)))
                .child(div().whitespace_normal().child(instance.path.display().to_string()))
                .child(div().whitespace_normal().child("Copies worlds, mods, settings, JVM arguments and playtime into a new instance. Your FTB App installation stays untouched."));
        }
        if self.selected.is_none() && self.selected_local.is_none() {
            detail = detail
                .child(
                    div()
                        .mt_4()
                        .text_size(px(18.))
                        .font_weight(FontWeight::BOLD)
                        .child(if self.local {
                            "Select an instance"
                        } else {
                            "Find your next modpack"
                        }),
                )
                .child(muted(if self.local {
                    "Choose an FTB App instance from the list to see its details."
                } else {
                    "Search the catalog, then choose a pack and version to install."
                }));
        }
        panel
            .child(
                div()
                    .flex()
                    .gap_2()
                    .flex_1()
                    .min_h(px(0.))
                    .child(list)
                    .child(detail),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_4()
                    .pt_3()
                    .border_t_1()
                    .border_color(rgb(0x3a3a3a))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .whitespace_normal()
                            .text_size(px(12.))
                            .text_color(rgb(0xa9c7df))
                            .child(self.status.clone()),
                    )
                    .child(
                        Button::new("ftb-install")
                            .primary()
                            .label(if self.busy {
                                "Working…"
                            } else if self.local {
                                "Import"
                            } else {
                                "Install"
                            })
                            .disabled(
                                self.busy
                                    || if self.local {
                                        self.selected_local.is_none()
                                    } else {
                                        self.selected.is_none() || self.version.is_none()
                                    },
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.install(cx))),
                    ),
            )
    }
}

fn muted(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(12.))
        .text_color(rgb(0xa5a9af))
        .child(text.into())
}

fn row(id: impl Into<ElementId>, selected: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .gap_3()
        .px_3()
        .py_3()
        .flex_shrink_0()
        .rounded_md()
        .cursor_pointer()
        .bg(rgb(if selected { 0x354453 } else { 0x202020 }))
        .hover(|style| style.bg(rgb(if selected { 0x3b4c5e } else { 0x2c2f33 })))
}

fn count(value: u64) -> String {
    for (unit, suffix) in [(1_000_000_000, "B"), (1_000_000, "M"), (1_000, "K")] {
        if value >= unit {
            return format!("{:.1}{suffix}", value as f64 / unit as f64);
        }
    }
    value.to_string()
}

fn local_loader(instance: &AppInstance) -> String {
    use prism_core::loaders::Loader;
    match instance.loader() {
        Ok(Loader::None) => "Vanilla".into(),
        Ok(loader) => format!(
            "{} {}",
            match &loader {
                Loader::Fabric { .. } => "Fabric",
                Loader::Quilt { .. } => "Quilt",
                Loader::Forge { .. } => "Forge",
                Loader::NeoForge { .. } => "NeoForge",
                Loader::None => unreachable!(),
            },
            loader.version().unwrap_or_default()
        ),
        Err(_) => "Loader unavailable".into(),
    }
}

fn target_label(targets: &[prism_core::modplatform::ftb::Target]) -> String {
    targets
        .iter()
        .filter_map(|target| match target.kind.as_str() {
            "game" if target.name == "minecraft" => Some(format!("MC {}", plain(&target.version))),
            "modloader" => Some(format!(
                "{} {}",
                plain(&target.name),
                plain(&target.version)
            )),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

#[cfg(test)]
mod tests {
    use super::{count, plain, target_label};
    #[test]
    fn formats_counts_and_version_targets() {
        assert_eq!(count(0), "0");
        assert_eq!(count(999), "999");
        assert_eq!(count(14_500_000), "14.5M");
        assert_eq!(count(2_500), "2.5K");
        assert_eq!(target_label(&[]), "");
        let targets: Vec<prism_core::modplatform::ftb::Target> = serde_json::from_str(r#"[{"type":"game","name":"minecraft","version":"1.20.1"},{"type":"modloader","name":"fabric","version":"0.14.21"}]"#).unwrap();
        assert_eq!(target_label(&targets), "MC 1.20.1 · fabric 0.14.21");
    }
    #[test]
    fn remote_text_never_uses_a_markup_renderer() {
        assert_eq!(plain("<b>FTB</b> <a href='x'>Pack</a>"), "FTB Pack");
        assert_eq!(plain("A plain synopsis"), "A plain synopsis");
        assert_eq!(plain(""), "");
        assert_eq!(plain("**FTB** [Pack](https://example.com)"), "FTB Pack");
    }
}
