use crate::state::App as LauncherApp;
use gpui_kit::base::Disableable;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::*;
use prism_core::import::ImportOutcome;
use prism_core::import_ftb::AppInstance;
use prism_core::modplatform::ftb::{filter_catalog, FtbApi, Pack, Sort};
use std::path::PathBuf;

pub(crate) fn buttons(cx: &mut Context<crate::main_window::MainWindow>) -> impl IntoElement {
    div()
        .flex()
        .gap_2()
        .child(
            Button::new("open-ftb")
                .label("FTB Modpacks")
                .on_click(cx.listener(|this, _, _, cx| {
                    open(this.app.clone(), false, cx);
                })),
        )
        .child(
            Button::new("open-ftb-app")
                .label("FTB App Import")
                .on_click(cx.listener(|this, _, _, cx| {
                    open(this.app.clone(), true, cx);
                })),
        )
}

pub(crate) fn open(app: Entity<LauncherApp>, local: bool, cx: &mut gpui::App) {
    let options = WindowOptions {
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
                        } else {
                            "Select a pack to import.".into()
                        };
                        this.packs = packs;
                        this.instances = instances;
                    }
                    Err(error) => this.status = format!("Could not load FTB: {error}"),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
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
            .p_4()
            .flex()
            .flex_col()
            .gap_3()
            .bg(rgb(0x242424))
            .text_color(rgb(0xeeeeee))
            .child(div().text_size(px(20.)).child(if self.local {
                "FTB App Import"
            } else {
                "FTB Modpacks"
            }))
            .child(Input::new(&self.search));
        if self.local {
            panel =
                panel.child(
                    div()
                        .flex()
                        .gap_2()
                        .child(Input::new(&self.root))
                        .child(
                            Button::new("ftb-directory")
                                .label("Choose directory…")
                                .disabled(self.busy)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.choose_directory(window, cx)
                                })),
                        )
                        .child(
                            Button::new("ftb-scan")
                                .label("Scan")
                                .disabled(self.busy)
                                .on_click(cx.listener(|this, _, _, cx| this.load(cx))),
                        ),
                );
        } else {
            let mut sorts = div().flex().gap_2().child("Sort:");
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
            panel = panel.child(sorts).child(
                Button::new("ftb-retry")
                    .label("Load catalog")
                    .disabled(self.busy)
                    .on_click(cx.listener(|this, _, _, cx| this.load(cx))),
            );
        }
        let query = self.search.read(cx).value().to_lowercase();
        let mut list = div()
            .id("ftb-list")
            .flex()
            .flex_col()
            .gap_1()
            .w(px(290.))
            .flex_shrink_0()
            .h_full()
            .overflow_y_scroll();
        if self.local {
            for (i, instance) in self
                .instances
                .iter()
                .enumerate()
                .filter(|(_, i)| i.name.to_lowercase().contains(query.trim()))
            {
                let path = instance.path.clone();
                let button = Button::new(("ftb-local", i))
                    .label(format!(
                        "{} ({})",
                        plain(&instance.name),
                        instance.minecraft
                    ))
                    .disabled(self.busy);
                list = list.child(
                    (if self.selected_local.as_ref() == Some(&path) {
                        button.primary()
                    } else {
                        button
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.selected_local = Some(path.clone());
                        cx.notify();
                    })),
                );
            }
        } else {
            for pack in filter_catalog(&self.packs, &query, self.sort) {
                let id = pack.id;
                let button = Button::new(("ftb-pack", id))
                    .label(plain(&pack.name))
                    .disabled(self.busy);
                list = list.child(
                    (if self.selected == Some(id) {
                        button.primary()
                    } else {
                        button
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.selected = Some(id);
                        this.version = this
                            .packs
                            .iter()
                            .find(|p| p.id == id)
                            .and_then(|p| p.versions.iter().max_by_key(|v| (v.updated, v.id)))
                            .map(|v| v.id);
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
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_2();
        if let Some(pack) = self.packs.iter().find(|p| Some(p.id) == self.selected) {
            detail = detail
                .child(plain(&pack.name))
                .child(
                    div()
                        .w_full()
                        .whitespace_normal()
                        .child(plain(&pack.synopsis)),
                )
                .child(format!(
                    "Authors: {}",
                    pack.authors
                        .iter()
                        .map(|a| plain(&a.name))
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
                .child(format!("{} plays · {} installs", pack.plays, pack.installs))
                .child("Version:");
            let mut versions: Vec<_> = pack.versions.iter().collect();
            versions.sort_by_key(|v| std::cmp::Reverse((v.updated, v.id)));
            for version in versions {
                let id = version.id;
                let button = Button::new(("ftb-version", id))
                    .label(plain(&version.name))
                    .disabled(self.busy);
                detail = detail.child(
                    (if self.version == Some(id) {
                        button.primary()
                    } else {
                        button
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.version = Some(id);
                        cx.notify();
                    })),
                );
            }
        }
        if let Some(instance) = self
            .instances
            .iter()
            .find(|i| Some(&i.path) == self.selected_local.as_ref())
        {
            detail = detail.child(plain(&instance.name)).child(format!("Minecraft {} · {}", instance.minecraft, instance.version))
                .child(instance.path.display().to_string()).child("Copies this instance, including worlds and settings. The source is not changed.");
        }
        panel
            .child(
                div()
                    .flex()
                    .gap_4()
                    .flex_1()
                    .min_h(px(0.))
                    .child(list)
                    .child(detail),
            )
            .child(self.status.clone())
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
            )
    }
}

#[cfg(test)]
mod tests {
    use super::plain;
    #[test]
    fn remote_text_never_uses_a_markup_renderer() {
        assert_eq!(plain("<b>FTB</b> <a href='x'>Pack</a>"), "FTB Pack");
        assert_eq!(plain("A plain synopsis"), "A plain synopsis");
        assert_eq!(plain(""), "");
        assert_eq!(plain("**FTB** [Pack](https://example.com)"), "FTB Pack");
    }
}
