use crate::{
    instance_window::managed_pack_page::changelog_markdown, main_window::MainWindow, state::App,
    theme::Button,
};
use gpui_kit::base::Disableable;
use gpui_kit::component::{
    button::ButtonVariants,
    input::{Input, InputState},
    text::TextView,
};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use prism_core::modplatform::technic::{self, Listing, Modpack};
use std::sync::Arc;

pub(super) fn open(app: Entity<App>, owner: WeakEntity<MainWindow>, cx: &mut gpui::App) {
    let options = WindowOptions {
        window_min_size: Some(size(px(680.), px(480.))),
        ..WindowOptions::default()
    };
    let _ = open_window(options, cx, |window, cx| {
        cx.new(|cx| TechnicWindow::new(app, owner, window, cx))
    });
}

struct TechnicWindow {
    app: Entity<App>,
    owner: WeakEntity<MainWindow>,
    search: Entity<InputState>,
    results: Vec<Listing>,
    selected: Option<String>,
    pack: Option<Arc<Modpack>>,
    build: Option<usize>,
    busy: bool,
    status: String,
}

impl TechnicWindow {
    fn new(
        app: Entity<App>,
        owner: WeakEntity<MainWindow>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search Technic packs"));
        let mut this = Self {
            app,
            owner,
            search,
            results: Vec::new(),
            selected: None,
            pack: None,
            build: None,
            busy: false,
            status: String::new(),
        };
        this.run_search(String::new(), cx);
        this
    }

    fn run_search(&mut self, term: String, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let state = self.app.read(cx);
        let dl = state.runtime.downloader.clone();
        let task = state
            .runtime
            .spawn(async move { technic::search(&dl, &term).await });
        self.results.clear();
        self.selected = None;
        self.pack = None;
        self.build = None;
        self.busy = true;
        self.status = "Loading Technic packs…".into();
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                this.status = match result {
                    Ok(Ok(results)) => {
                        this.results = results;
                        if this.results.is_empty() {
                            "No packs found.".into()
                        } else {
                            "Select a pack to view its builds.".into()
                        }
                    }
                    Ok(Err(error)) => format!("Search failed: {error}"),
                    Err(error) => format!("Search failed: {error}"),
                };
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn select(&mut self, slug: String, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let state = self.app.read(cx);
        let dl = state.runtime.downloader.clone();
        let selected = slug.clone();
        let task = state
            .runtime
            .spawn(async move { technic::details(&dl, &slug).await });
        self.selected = Some(selected);
        self.pack = None;
        self.build = None;
        self.busy = true;
        self.status = "Loading pack details and builds…".into();
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                this.status = match result {
                    Ok(Ok(pack)) => {
                        this.build = Some(
                            pack.builds
                                .iter()
                                .position(|build| build == &pack.recommended)
                                .unwrap_or(0),
                        );
                        this.pack = Some(Arc::new(pack));
                        "Select a build to install.".into()
                    }
                    Ok(Err(error)) => format!("Could not load pack details: {error}"),
                    Err(error) => format!("Could not load pack details: {error}"),
                };
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn install(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let Some(pack) = self.pack.clone() else {
            return;
        };
        let Some(build) = self.build.and_then(|index| pack.builds.get(index)).cloned() else {
            return;
        };
        let app = self.app.clone();
        let owner = self.owner.clone();
        let state = app.read(cx);
        let dl = state.runtime.downloader.clone();
        let paths = state.paths.clone();
        let task = state
            .runtime
            .spawn(async move { technic::install(&paths, &dl, &pack, &build).await });
        self.busy = true;
        self.status = "Downloading and installing Technic pack…".into();
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let failed = !matches!(&result, Ok(Ok(_)));
            let status = match result {
                Ok(Ok(instance)) => {
                    let message = format!("Installed {}", instance.name);
                    let _ = app.update(cx, |state, cx| {
                        state.reload_instances();
                        state.selected = Some(instance.id);
                        state.toast = Some((message.clone(), std::time::Instant::now()));
                        cx.notify();
                    });
                    let _ = owner.update(cx, |_, cx| cx.notify());
                    message
                }
                Ok(Err(error)) => format!("Import failed: {error}"),
                Err(error) => format!("Import failed: {error}"),
            };
            let shown_in_window = this
                .update(cx, |this, cx| {
                    this.busy = false;
                    this.status.clone_from(&status);
                    cx.notify();
                })
                .is_ok();
            if failed && !shown_in_window {
                let _ = app.update(cx, |state, cx| {
                    state.toast = Some((status, std::time::Instant::now()));
                    cx.notify();
                });
                let _ = owner.update(cx, |_, cx| cx.notify());
            }
        })
        .detach();
        cx.notify();
    }

    fn render_pack(&self, pack: &Modpack, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = crate::theme::Palette::get(cx);
        let builds = pack.builds.iter().enumerate().map(|(index, build)| {
            let label = if build == &pack.recommended && pack.builds.len() > 1 {
                format!("{build} (recommended)")
            } else {
                build.clone()
            };
            Button::new(SharedString::from(format!("technic-build-{index}")))
                .label(label)
                .disabled(self.busy)
                .when(self.build == Some(index), |button| button.primary())
                .on_click(cx.listener(move |this, _, _, cx| {
                    if !this.busy {
                        this.build = Some(index);
                        cx.notify();
                    }
                }))
        });
        let description = pack.description.trim();
        let description = if description.is_empty() {
            "No description provided."
        } else {
            description
        };
        div()
            .flex()
            .flex_col()
            .gap_2()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .child(
                div()
                    .text_lg()
                    .font_weight(FontWeight::BOLD)
                    .child(pack.title.clone()),
            )
            .when(
                !pack.author.is_empty() || !pack.minecraft.is_empty(),
                |page| {
                    let byline = (!pack.author.is_empty()).then(|| format!("By {}", pack.author));
                    let minecraft = (!pack.minecraft.is_empty())
                        .then(|| format!("Minecraft {}", pack.minecraft));
                    page.child(
                        div().text_sm().child(
                            byline
                                .into_iter()
                                .chain(minecraft)
                                .collect::<Vec<_>>()
                                .join("  •  "),
                        ),
                    )
                },
            )
            .child(
                div()
                    .id("technic-description-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_3()
                    .rounded_md()
                    .bg(palette.base)
                    .text_color(palette.base_text)
                    .child(
                        TextView::markdown(
                            "technic-description",
                            changelog_markdown(description).into_owned(),
                        )
                        .image_source(|_| gpui::ImageSource::Custom(Arc::new(|_, _| None)))
                        .on_link_click(|target, _, _, cx| {
                            let target = target.as_str();
                            if (target.starts_with("https://") || target.starts_with("http://"))
                                && !target.chars().any(char::is_control)
                            {
                                cx.open_url(target);
                            }
                        }),
                    ),
            )
            .child(div().text_sm().child("Builds"))
            .child(
                div()
                    .id("technic-build-list")
                    .max_h(px(112.))
                    .overflow_y_scroll()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .children(builds),
            )
    }
}

impl Render for TechnicWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = crate::theme::Palette::get(cx);
        let results = self.results.iter().enumerate().map(|(index, listing)| {
            let slug = listing.slug.clone();
            div()
                .id(SharedString::from(format!("technic-result-{index}")))
                .w_full()
                .px_3()
                .py_2()
                .rounded_md()
                .text_sm()
                .when(self.selected.as_ref() == Some(&slug), |row| {
                    row.bg(palette.selection)
                })
                .when(!self.busy, |row| {
                    row.cursor_pointer().hover(|row| row.bg(palette.hover))
                })
                .on_click(cx.listener(move |this, _, _, cx| this.select(slug.clone(), cx)))
                .child(listing.name.clone())
        });
        div()
            .size_full()
            .p_4()
            .flex()
            .flex_col()
            .gap_3()
            .bg(palette.window)
            .text_color(palette.text)
            .child(
                div()
                    .text_xl()
                    .font_weight(FontWeight::BOLD)
                    .child("Technic Packs"),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(div().flex_1().min_w_0().child(Input::new(&self.search)))
                    .child(
                        Button::new("technic-search")
                            .label("Search")
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.run_search(this.search.read(cx).value().to_string(), cx)
                            })),
                    )
                    .child(
                        Button::new("technic-trending")
                            .label("Trending")
                            .disabled(self.busy)
                            .on_click(
                                cx.listener(|this, _, _, cx| this.run_search(String::new(), cx)),
                            ),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .gap_4()
                    .child(
                        div()
                            .id("technic-results")
                            .w(px(215.))
                            .overflow_y_scroll()
                            .p_1()
                            .rounded_md()
                            .bg(palette.panel)
                            .children(results),
                    )
                    .children(self.pack.as_ref().map(|pack| self.render_pack(pack, cx))),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .child(self.status.clone()),
                    )
                    .child(
                        Button::new("technic-install")
                            .label("Install Pack")
                            .primary()
                            .disabled(self.busy || self.build.is_none())
                            .on_click(cx.listener(|this, _, _, cx| this.install(cx))),
                    )
                    .child(
                        Button::new("technic-close")
                            .label("Close")
                            .on_click(|_, window, _| window.remove_window()),
                    ),
            )
    }
}
