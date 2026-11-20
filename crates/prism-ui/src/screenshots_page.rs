use super::InstanceWindow;
use gpui_kit::base::Disableable;
use gpui_kit::component::{
    button::Button,
    input::{Input, InputState},
};
use gpui_kit::*;
use prism_core::screenshots::{imgur_client_id, upload_to_imgur, Screenshot, IMGUR_CLIENT_ID_ENV};
use std::{io::Cursor, path::PathBuf, sync::Arc};

#[derive(Clone)]
struct Tile {
    file: Screenshot,
    thumbnail: Option<Arc<RenderImage>>,
}

#[derive(Clone, Copy)]
enum Action {
    Rename,
    Delete,
    Upload,
}

impl Action {
    fn label(self) -> &'static str {
        match self {
            Self::Rename => "Rename",
            Self::Delete => "Delete",
            Self::Upload => "Upload",
        }
    }
}

struct Dialog {
    action: Action,
    target: Screenshot,
    input: Option<Entity<InputState>>,
    missing_key: bool,
}

#[derive(Default)]
pub(super) struct ScreenshotsPage {
    root: Option<PathBuf>,
    tiles: Vec<Tile>,
    selected: Option<Screenshot>,
    dialog: Option<Dialog>,
    status: String,
    url: Option<String>,
    loading: bool,
    busy: bool,
    revision: u64,
}

impl ScreenshotsPage {
    pub(super) fn on_navigation(&mut self) {
        self.dialog = None;
    }

    fn select(&mut self, target: Option<Screenshot>) {
        self.selected = target;
        self.url = None;
    }
}

fn thumbnail(file: &Screenshot) -> Option<Arc<RenderImage>> {
    let bytes = file.read_png().ok()?;
    let mut reader = image::ImageReader::with_format(Cursor::new(bytes), image::ImageFormat::Png);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16_384);
    limits.max_image_height = Some(16_384);
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    let mut buffer = reader.decode().ok()?.thumbnail(240, 144).into_rgba8();
    for pixel in buffer.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Some(Arc::new(RenderImage::new([image::Frame::new(buffer)])))
}

enum Outcome {
    Renamed(String),
    Deleted,
    Uploaded(String),
}

impl InstanceWindow {
    pub(super) fn load_screenshots(&mut self, select: Option<String>, cx: &mut Context<Self>) {
        if self.screenshots.busy {
            return;
        }
        let root = self
            .app
            .read(cx)
            .instances
            .get(&self.instance_id)
            .map(|instance| instance.game_root());
        let page = &mut self.screenshots;
        page.revision += 1;
        let revision = page.revision;
        let cache = std::mem::take(&mut page.tiles);
        let select = select.or_else(|| page.selected.as_ref().map(|shot| shot.name().to_owned()));
        page.select(None);
        page.root = root.clone();
        let Some(root) = root else {
            page.loading = false;
            page.status = "The instance no longer exists.".into();
            return;
        };
        page.loading = true;
        let task = self.app.read(cx).runtime.handle().spawn_blocking(move || {
            let files = Screenshot::list(&root)?;
            let tiles = files
                .into_iter()
                .map(|file| {
                    let thumbnail = match cache
                        .binary_search_by(|tile| tile.file.name().cmp(file.name()))
                    {
                        Ok(index) if cache[index].file == file => cache[index].thumbnail.clone(),
                        _ => thumbnail(&file),
                    };
                    Tile { file, thumbnail }
                })
                .collect::<Vec<_>>();
            Ok::<_, std::io::Error>(tiles)
        });
        cx.spawn(async move |this, cx| {
            let result = task
                .await
                .map_err(|error| error.to_string())
                .and_then(|result| result.map_err(|error| error.to_string()));
            let _ = this.update(cx, |this, cx| {
                let page = &mut this.screenshots;
                if revision != page.revision {
                    return;
                }
                page.loading = false;
                match result {
                    Ok(tiles) => {
                        page.selected = select
                            .as_deref()
                            .and_then(|name| tiles.iter().find(|tile| tile.file.name() == name))
                            .map(|tile| tile.file.clone());
                        page.tiles = tiles;
                    }
                    Err(error) => page.status = format!("Could not load screenshots: {error}"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn show_screenshot_dialog(
        &mut self,
        action: Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(target) = self.screenshots.selected.clone() else {
            return;
        };
        let input = matches!(action, Action::Rename).then(|| {
            cx.new(|cx| {
                let mut input = InputState::new(window, cx);
                input.set_value(target.name().to_owned(), window, cx);
                input.focus(window, cx);
                input
            })
        });
        self.screenshots.dialog = Some(Dialog {
            action,
            target,
            input,
            missing_key: matches!(action, Action::Upload) && imgur_client_id().is_none(),
        });
        cx.notify();
    }

    fn apply_screenshot_action(&mut self, cx: &mut Context<Self>) {
        let Some(Dialog {
            action,
            target,
            input,
            missing_key,
        }) = self.screenshots.dialog.take()
        else {
            return;
        };
        let app = self.app.read(cx);
        let instance = app.instances.get(&self.instance_id);
        if !instance.is_some_and(|instance| instance.game_root() == target.game_root()) {
            self.screenshots.status =
                "The instance game folder changed. Refresh and try again.".into();
            cx.notify();
            return;
        }
        let runtime = app.runtime.clone();
        let name = input.map(|input| input.read(cx).value().to_string());
        let client_id = if matches!(action, Action::Upload) {
            let Some(id) = imgur_client_id().filter(|_| !missing_key) else {
                return;
            };
            Some(id)
        } else {
            None
        };
        self.screenshots.busy = true;
        self.screenshots.url = None;
        self.screenshots.status = format!("{} in progress…", action.label());
        let task = runtime.spawn(async move {
            match action {
                Action::Upload => {
                    let file = target.clone();
                    let bytes = tokio::task::spawn_blocking(move || file.read_png())
                        .await
                        .map_err(|error| error.to_string())?
                        .map_err(|error| error.to_string())?;
                    upload_to_imgur(client_id.as_deref().unwrap(), target.name(), bytes)
                        .await
                        .map(|url| Outcome::Uploaded(url.to_string()))
                }
                Action::Rename | Action::Delete => tokio::task::spawn_blocking(move || {
                    if matches!(action, Action::Rename) {
                        target.rename(name.as_deref().unwrap()).map(|path| {
                            Outcome::Renamed(
                                path.file_name().unwrap().to_string_lossy().into_owned(),
                            )
                        })
                    } else {
                        target.delete().map(|()| Outcome::Deleted)
                    }
                    .map_err(|error| error.to_string())
                })
                .await
                .map_err(|error| error.to_string())?,
            }
        });
        cx.spawn(async move |this, cx| {
            let result = task
                .await
                .map_err(|error| error.to_string())
                .and_then(|result| result);
            let _ = this.update(cx, |this, cx| {
                this.screenshots.busy = false;
                match result {
                    Ok(Outcome::Renamed(name)) => {
                        this.screenshots.status = format!("Renamed to {name}.");
                        this.load_screenshots(Some(name), cx);
                    }
                    Ok(Outcome::Deleted) => {
                        this.screenshots.status = "Screenshot deleted.".into();
                        this.screenshots.select(None);
                        this.load_screenshots(None, cx);
                    }
                    Ok(Outcome::Uploaded(url)) => {
                        this.screenshots.status = "Uploaded to Imgur.".into();
                        if this.screenshots.selected.is_some() {
                            this.screenshots.url = Some(url);
                        }
                    }
                    Err(error) => {
                        this.screenshots.status = format!("{} failed: {error}", action.label())
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn render_screenshots_page(&self, cx: &mut Context<Self>) -> AnyElement {
        let palette = crate::theme::Palette::get(cx);
        let page = &self.screenshots;
        let selected = page.selected.is_some();
        let actions_disabled = page.busy || page.loading || page.dialog.is_some();
        let has_thumbnail = page
            .tiles
            .iter()
            .any(|tile| page.selected.as_ref() == Some(&tile.file) && tile.thumbnail.is_some());
        let mut toolbar = div().flex().flex_wrap().items_center().gap_2().p_2().child(
            Button::new("refresh-screenshots")
                .label("Refresh")
                .disabled(actions_disabled)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.screenshots.status.clear();
                    this.load_screenshots(None, cx);
                    cx.notify();
                })),
        );
        for action in [Action::Rename, Action::Delete, Action::Upload] {
            toolbar = toolbar.child(
                Button::new(SharedString::from(format!("screenshot-{}", action.label())))
                    .label(if matches!(action, Action::Upload) {
                        "Upload to Imgur"
                    } else {
                        action.label()
                    })
                    .disabled(
                        actions_disabled
                            || !selected
                            || (matches!(action, Action::Upload) && !has_thumbnail),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.show_screenshot_dialog(action, window, cx)
                    })),
            );
        }
        let grid =
            div()
                .flex()
                .flex_wrap()
                .gap_3()
                .p_3()
                .children(page.tiles.iter().enumerate().map(|(index, tile)| {
                    let target = tile.file.clone();
                    let active = page.selected.as_ref() == Some(&target);
                    let preview = match &tile.thumbnail {
                        Some(image) => img(ImageSource::Render(image.clone()))
                            .size_full()
                            .object_fit(ObjectFit::Contain)
                            .into_any_element(),
                        None => div()
                            .text_color(palette.secondary_text)
                            .child("Preview unavailable")
                            .into_any_element(),
                    };
                    div()
                        .id(("screenshot", index))
                        .cursor_pointer()
                        .w(px(204.))
                        .p_2()
                        .rounded_md()
                        .bg(if active {
                            palette.selection
                        } else {
                            palette.panel
                        })
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_center()
                                .w_full()
                                .h(px(114.))
                                .bg(palette.base)
                                .child(preview),
                        )
                        .child(
                            div()
                                .pt_1()
                                .text_size(px(12.))
                                .text_ellipsis()
                                .child(target.name().to_owned()),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if !actions_disabled {
                                this.screenshots.status.clear();
                                this.screenshots.select(Some(target.clone()));
                                cx.notify();
                            }
                        }))
                }));
        div()
            .relative()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                self.page_header(
                    cx,
                    page.root
                        .as_ref()
                        .map(|root| root.join("screenshots").display().to_string())
                        .unwrap_or_else(|| "Screenshots".into()),
                ),
            )
            .child(toolbar)
            .children(
                (!page.status.is_empty()).then(|| div().px_2().pb_1().child(page.status.clone())),
            )
            .children(page.url.as_ref().map(|url| {
                let open = url.clone();
                let copy = url.clone();
                div()
                    .flex()
                    .gap_2()
                    .px_2()
                    .child(
                        Button::new("open-imgur-url")
                            .label(url.clone())
                            .on_click(move |_, _, cx| cx.open_url(&open)),
                    )
                    .child(
                        Button::new("copy-imgur-url")
                            .label("Copy Link")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()));
                                this.screenshots.status = "Link copied to clipboard.".into();
                                cx.notify();
                            })),
                    )
            }))
            .child(
                div()
                    .id("screenshots-grid")
                    .min_h_0()
                    .flex_1()
                    .overflow_y_scroll()
                    .children(page.tiles.is_empty().then(|| {
                        div().p_3().child(if page.loading {
                            "Loading screenshots…"
                        } else {
                            "No screenshots."
                        })
                    }))
                    .child(grid),
            )
            .children(
                page.dialog
                    .as_ref()
                    .map(|dialog| self.render_screenshot_dialog(dialog, cx)),
            )
            .into_any_element()
    }

    fn render_screenshot_dialog(
        &self,
        dialog: &Dialog,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = crate::theme::Palette::get(cx);
        let (title, message) = if dialog.missing_key {
            ("Imgur client ID required", format!("Add a client ID in Settings → API Keys or set {IMGUR_CLIENT_ID_ENV}. No screenshot was sent."))
        } else {
            match dialog.action {
                Action::Rename => (
                    "Rename screenshot",
                    "Enter a new file name. The .png extension is kept.".into(),
                ),
                Action::Delete => (
                    "Delete screenshot?",
                    format!(
                        "Permanently delete {}? This cannot be undone.",
                        dialog.target.name()
                    ),
                ),
                Action::Upload => (
                    "Upload screenshot to Imgur?",
                    format!(
                        "Send {} to Imgur? Anyone with the link can view the image.",
                        dialog.target.name()
                    ),
                ),
            }
        };
        div()
            .id("screenshot-dialog")
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
                    .w(px(400.))
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
                            .child(title),
                    )
                    .child(div().whitespace_normal().child(message))
                    .children(dialog.input.as_ref().map(Input::new))
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap_2()
                            .child(
                                Button::new("cancel-screenshot-action")
                                    .label(if dialog.missing_key {
                                        "Close"
                                    } else {
                                        "Cancel"
                                    })
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.screenshots.dialog = None;
                                        cx.notify();
                                    })),
                            )
                            .children((!dialog.missing_key).then(|| {
                                Button::new("confirm-screenshot-action")
                                    .label(dialog.action.label())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.apply_screenshot_action(cx)
                                    }))
                            })),
                    ),
            )
    }
}
