use super::{InstanceWindow, Page};
use gpui_kit::base::Disableable;
use gpui_kit::component::{
    button::Button,
    input::{Input, InputEvent, InputState},
};
use gpui_kit::*;
use prism_core::logs::{upload_log, LogFile};
use std::{
    ops::Range,
    sync::{Arc, Mutex},
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Source {
    Console,
    Other,
}

#[derive(Default)]
pub(super) struct LogsPages {
    console: Viewer,
    console_log: Option<Arc<Mutex<Vec<String>>>>,
    console_len: usize,
    other: Viewer,
    files: Vec<LogFile>,
    selected: Option<LogFile>,
    revision: u64,
    deleting: bool,
    files_status: String,
}

impl LogsPages {
    fn viewer(&self, source: Source) -> &Viewer {
        match source {
            Source::Console => &self.console,
            Source::Other => &self.other,
        }
    }

    fn viewer_mut(&mut self, source: Source) -> &mut Viewer {
        match source {
            Source::Console => &mut self.console,
            Source::Other => &mut self.other,
        }
    }
}

#[derive(Clone)]
struct LogMatch {
    line: usize,
    bytes: Range<usize>,
}

#[derive(Default)]
struct Viewer {
    lines: Arc<Vec<SharedString>>,
    exact_text: Option<String>,
    input: Option<Entity<InputState>>,
    scroll: UniformListScrollHandle,
    needle: String,
    matches: Arc<Vec<LogMatch>>,
    active: Option<usize>,
    status: String,
    url: Option<String>,
    uploading: bool,
}

impl Viewer {
    fn replace(&mut self, text: Option<String>) {
        self.lines = Arc::new(
            text.as_deref()
                .unwrap_or_default()
                .lines()
                .map(SharedString::from)
                .collect(),
        );
        self.exact_text = text;
        self.rebuild_search();
        if self.active.is_none() {
            self.scroll.scroll_to_item(0, ScrollStrategy::Top);
        }
    }

    fn text(&self) -> String {
        self.exact_text
            .clone()
            .unwrap_or_else(|| self.lines.join("\n"))
    }

    fn set_query(&mut self, query: String) {
        self.needle = query.to_ascii_lowercase();
        self.rebuild_search();
    }

    fn rebuild_search(&mut self) {
        self.matches = Arc::default();
        self.append_matches(0);
        self.active = (!self.matches.is_empty()).then_some(0);
        self.reveal();
    }

    fn append_matches(&mut self, start: usize) {
        if self.needle.is_empty() {
            return;
        }
        let matches = Arc::make_mut(&mut self.matches);
        for (index, line) in self.lines.iter().enumerate().skip(start) {
            for (start, matched) in line.to_ascii_lowercase().match_indices(&self.needle) {
                matches.push(LogMatch {
                    line: index,
                    bytes: start..start + matched.len(),
                });
            }
        }
    }

    fn step(&mut self, previous: bool) {
        let count = self.matches.len();
        if count != 0 {
            self.active = Some(match (self.active, previous) {
                (None | Some(0), true) => count - 1,
                (None, false) => 0,
                (Some(index), true) => index - 1,
                (Some(index), false) => (index + 1) % count,
            });
            self.reveal();
        }
    }

    fn reveal(&self) {
        if let Some(index) = self.active {
            self.scroll
                .scroll_to_item(self.matches[index].line, ScrollStrategy::Center);
        }
    }
}

impl InstanceWindow {
    pub(super) fn prepare_logs_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let source = if self.page == Page::Log {
            self.sync_console(cx);
            Source::Console
        } else {
            Source::Other
        };
        let viewer = self.logs.viewer_mut(source);
        if viewer.input.is_none() {
            let input = cx.new(|cx| InputState::new(window, cx).placeholder("Find in log…"));
            cx.subscribe(&input, move |this, input, event: &InputEvent, cx| {
                let viewer = this.logs.viewer_mut(source);
                match event {
                    InputEvent::Change => viewer.set_query(input.read(cx).value().to_string()),
                    InputEvent::PressEnter { shift, .. } => viewer.step(*shift),
                    _ => return,
                }
                cx.notify();
            })
            .detach();
            viewer.input = Some(input);
        }
    }

    fn sync_console(&mut self, cx: &Context<Self>) {
        let current = self
            .app
            .read(cx)
            .running
            .get(&self.instance_id)
            .map(|running| running.log.clone());
        let unchanged = match (&current, &self.logs.console_log) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        let viewer = &mut self.logs.console;
        if !unchanged {
            viewer.replace(None);
            viewer.status.clear();
            viewer.url = None;
            self.logs.console_len = 0;
            self.logs.console_log = current.clone();
        }
        if let Some(current) = current {
            let Ok(log) = current.lock() else { return };
            if self.logs.console_len == log.len() {
                return;
            }
            let offset = viewer.lines.len();
            Arc::make_mut(&mut viewer.lines).extend(
                log[self.logs.console_len..]
                    .iter()
                    .flat_map(|line| line.split('\n'))
                    .map(SharedString::from),
            );
            self.logs.console_len = log.len();
            drop(log);
            viewer.append_matches(offset);
            if viewer.active.is_none() && !viewer.matches.is_empty() {
                viewer.active = Some(0);
                viewer.reveal();
            }
        }
    }

    pub(super) fn refresh_other_logs(&mut self, cx: &mut Context<Self>) {
        let root = self
            .app
            .read(cx)
            .instances
            .get(&self.instance_id)
            .filter(|instance| !instance.is_horizon())
            .map(|instance| instance.game_root());
        let name = self
            .logs
            .selected
            .as_ref()
            .map(|file| file.name().to_owned());
        self.logs.revision += 1;
        let revision = self.logs.revision;
        self.logs.files.clear();
        self.logs.selected = None;
        self.logs.other.replace(None);
        self.logs.other.status.clear();
        let Some(root) = root else {
            self.logs.files_status = "The Java instance no longer exists.".into();
            return;
        };
        self.logs.files_status = "Loading logs…".into();
        let task = self
            .app
            .read(cx)
            .runtime
            .handle()
            .spawn_blocking(move || LogFile::list(&root));
        cx.spawn(async move |this, cx| {
            let result = task
                .await
                .map_err(|error| error.to_string())
                .and_then(|result| result.map_err(|error| error.to_string()));
            let _ = this.update(cx, |this, cx| {
                if revision != this.logs.revision {
                    return;
                }
                match result {
                    Ok(files) => {
                        let selected = name
                            .as_ref()
                            .and_then(|name| files.iter().find(|file| file.name() == name))
                            .cloned();
                        this.logs.files = files;
                        this.logs.files_status.clear();
                        if let Some(file) = selected {
                            this.select_log(file, cx);
                        }
                    }
                    Err(error) => this.logs.files_status = format!("Could not list logs: {error}"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn select_log(&mut self, file: LogFile, cx: &mut Context<Self>) {
        self.logs.revision += 1;
        let revision = self.logs.revision;
        self.logs.selected = Some(file.clone());
        self.logs.other.replace(None);
        self.logs.other.url = None;
        self.logs.other.status = "Reading log…".into();
        let task = self
            .app
            .read(cx)
            .runtime
            .handle()
            .spawn_blocking(move || file.read());
        cx.spawn(async move |this, cx| {
            let result = task
                .await
                .map_err(|error| error.to_string())
                .and_then(|result| result.map_err(|error| error.to_string()));
            let _ = this.update(cx, |this, cx| {
                if revision != this.logs.revision {
                    return;
                }
                match result {
                    Ok(text) => {
                        this.logs.other.status.clear();
                        this.logs.other.replace(Some(text));
                    }
                    Err(error) => this.logs.other.status = format!("Could not read log: {error}"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn confirm_log_upload(&mut self, source: Source, window: &mut Window, cx: &mut Context<Self>) {
        let viewer = self.logs.viewer_mut(source);
        if viewer.uploading || viewer.lines.is_empty() {
            return;
        }
        let text = viewer.text();
        viewer.uploading = true;
        viewer.url = None;
        let answer = window.prompt(
            PromptLevel::Warning,
            "Upload this log to mclo.gs?",
            Some("Anyone with the link can read it.\nKnown private data is redacted.\nOther private data may remain."),
            &[
                PromptButton::cancel("Cancel"),
                PromptButton::new("Upload"),
            ],
            cx,
        );
        let runtime = self.app.read(cx).runtime.clone();
        cx.spawn(async move |this, cx| {
            let result = if answer.await == Ok(1) {
                let _ = this.update(cx, |this, cx| {
                    this.logs.viewer_mut(source).status = "Uploading log…".into();
                    cx.notify();
                });
                Some(
                    runtime
                        .spawn(async move { upload_log(&text).await })
                        .await
                        .map_err(|error| error.to_string())
                        .and_then(|result| result),
                )
            } else {
                None
            };
            let _ = this.update(cx, |this, cx| {
                let viewer = this.logs.viewer_mut(source);
                viewer.uploading = false;
                match result {
                    Some(Ok(url)) => {
                        viewer.status = "Log uploaded to mclo.gs.".into();
                        viewer.url = Some(url.to_string());
                    }
                    Some(Err(error)) => viewer.status = format!("Upload failed: {error}"),
                    None => viewer.status = "Upload canceled.".into(),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn confirm_log_delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(file) = self.logs.selected.clone() else {
            return;
        };
        if self.logs.deleting || self.servers_running(cx) {
            return;
        }
        self.logs.deleting = true;
        let answer = window.prompt(
            PromptLevel::Warning,
            &format!("Delete {}?", file.name()),
            Some("This permanently removes\nthe selected log."),
            &[PromptButton::cancel("Cancel"), PromptButton::new("Delete")],
            cx,
        );
        cx.spawn(async move |this, cx| {
            if answer.await != Ok(1) {
                let _ = this.update(cx, |this, cx| {
                    this.logs.deleting = false;
                    cx.notify();
                });
                return;
            }
            let task = this.update(cx, |this, cx| -> Result<_, String> {
                let app = this.app.read(cx);
                let instance = app.instances.get(&this.instance_id)
                    .filter(|instance| !instance.is_horizon() && instance.game_root() == file.game_root())
                    .ok_or("The instance game folder changed.")?;
                if !instance.root.join("instance.cfg").is_file() || this.servers_running(cx) {
                    return Err("Stop Minecraft before deleting logs, and check that the instance still exists.".into());
                }
                let lock = crate::instance_lock::acquire(&app.paths, instance).map_err(|error| error.to_string())?;
                Ok(app.runtime.handle().spawn_blocking(move || {
                    let _lock = lock;
                    file.delete().map_err(|error| error.to_string())
                }))
            });
            let result = match task {
                Ok(Ok(task)) => task.await.map_err(|error| error.to_string()).and_then(|result| result),
                Ok(Err(error)) => Err(error),
                Err(_) => return,
            };
            let _ = this.update(cx, |this, cx| {
                this.logs.deleting = false;
                match result {
                    Ok(()) => this.refresh_other_logs(cx),
                    Err(error) => this.logs.files_status = format!("Could not delete log: {error}"),
                }
                cx.notify();
            });
        }).detach();
        cx.notify();
    }

    pub(super) fn render_logs_page(&self, cx: &mut Context<Self>) -> AnyElement {
        let source = if self.page == Page::Log {
            Source::Console
        } else {
            Source::Other
        };
        let viewer = self.logs.viewer(source);
        let searchable = !viewer.matches.is_empty();
        let empty = viewer.lines.is_empty();
        let mut toolbar = div()
            .flex()
            .items_center()
            .flex_wrap()
            .gap_2()
            .p_2()
            .child(
                div()
                    .w(px(155.))
                    .child(Input::new(viewer.input.as_ref().unwrap())),
            )
            .child(
                Button::new("previous-log-match")
                    .label("Previous")
                    .disabled(!searchable)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.logs.viewer_mut(source).step(true);
                        cx.notify();
                    })),
            )
            .child(
                Button::new("next-log-match")
                    .label("Next")
                    .disabled(!searchable)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.logs.viewer_mut(source).step(false);
                        cx.notify();
                    })),
            )
            .child(div().text_size(px(12.)).child(if viewer.needle.is_empty() {
                String::new()
            } else {
                format!(
                    "{} / {}",
                    viewer.active.map_or(0, |index| index + 1),
                    viewer.matches.len()
                )
            }))
            .child(
                Button::new("copy-log")
                    .label("Copy All")
                    .disabled(empty)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let viewer = this.logs.viewer_mut(source);
                        cx.write_to_clipboard(ClipboardItem::new_string(viewer.text()));
                        viewer.status = "Log copied to clipboard.".into();
                        cx.notify();
                    })),
            )
            .child(
                Button::new("upload-log")
                    .label("Upload")
                    .disabled(empty || viewer.uploading)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.confirm_log_upload(source, window, cx)
                    })),
            );
        if source == Source::Other {
            toolbar = toolbar
                .child(
                    Button::new("refresh-logs")
                        .label("Refresh")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.refresh_other_logs(cx);
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("delete-log")
                        .label("Delete")
                        .disabled(
                            self.logs.selected.is_none()
                                || self.logs.deleting
                                || self.servers_running(cx),
                        )
                        .on_click(
                            cx.listener(|this, _, window, cx| this.confirm_log_delete(window, cx)),
                        ),
                );
        }
        let mut page = div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                self.page_header(
                    cx,
                    if source == Source::Console {
                        "Console output"
                    } else {
                        "Other Logs · logs/"
                    }
                    .into(),
                ),
            )
            .child(toolbar);
        if !viewer.status.is_empty() {
            page = page.child(
                div()
                    .px_2()
                    .pb_1()
                    .text_size(px(12.))
                    .child(viewer.status.clone()),
            );
        }
        if let Some(url) = &viewer.url {
            let open = url.clone();
            let copy = url.clone();
            page =
                page.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .px_2()
                        .pb_2()
                        .child(
                            Button::new("open-log-url")
                                .label(url.clone())
                                .on_click(move |_, _, cx| cx.open_url(&open)),
                        )
                        .child(Button::new("copy-log-url").label("Copy Link").on_click(
                            cx.listener(move |this, _, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()));
                                this.logs.viewer_mut(source).status =
                                    "Link copied to clipboard.".into();
                                cx.notify();
                            }),
                        )),
                );
        }
        let document = if empty {
            div()
                .flex_1()
                .p_3()
                .child(if source == Source::Console {
                    "No console output yet. Launch the instance to see its log."
                } else if self.logs.selected.is_some() {
                    "No log contents to display."
                } else {
                    "Select a log to view it."
                })
                .into_any_element()
        } else {
            let lines = viewer.lines.clone();
            let matches = viewer.matches.clone();
            let active = viewer.active;
            uniform_list("log-document", lines.len(), move |range, _, _| {
                range
                    .map(|index| {
                        let start = matches.partition_point(|hit| hit.line < index);
                        let end = matches.partition_point(|hit| hit.line <= index);
                        let highlights = (start..end).map(|hit| {
                            (
                                matches[hit].bytes.clone(),
                                HighlightStyle {
                                    background_color: Some(
                                        if active == Some(hit) {
                                            rgb(0xf4ae44)
                                        } else {
                                            rgb(0x666027)
                                        }
                                        .into(),
                                    ),
                                    color: Some(
                                        if active == Some(hit) {
                                            rgb(0x1a1a1a)
                                        } else {
                                            rgb(0xffffff)
                                        }
                                        .into(),
                                    ),
                                    ..Default::default()
                                },
                            )
                        });
                        div().h(px(18.)).px_2().whitespace_nowrap().child(
                            StyledText::new(lines[index].clone()).with_highlights(highlights),
                        )
                    })
                    .collect()
            })
            .track_scroll(&viewer.scroll)
            .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
            .flex_1()
            .min_w_0()
            .min_h_0()
            .bg(rgb(0x1a1a1a))
            .font_family("monospace")
            .text_size(px(12.))
            .text_color(rgb(0xcccccc))
            .into_any_element()
        };
        if source == Source::Console {
            return page.child(document).into_any_element();
        }
        if !self.logs.files_status.is_empty() {
            page = page.child(
                div()
                    .px_2()
                    .pb_1()
                    .text_size(px(12.))
                    .child(self.logs.files_status.clone()),
            );
        }
        let files = div()
            .id("log-files")
            .w(px(200.))
            .flex_shrink_0()
            .overflow_y_scroll()
            .border_r_1()
            .border_color(rgb(0x1c1c1c))
            .children(
                self.logs
                    .files
                    .is_empty()
                    .then(|| div().p_2().child("No log files.")),
            )
            .children(self.logs.files.iter().enumerate().map(|(index, file)| {
                let selected = self.logs.selected.as_ref() == Some(file);
                let file = file.clone();
                div()
                    .id(("log-file", index))
                    .p_2()
                    .cursor_pointer()
                    .text_ellipsis()
                    .bg(if selected {
                        rgb(0x3d5a80)
                    } else {
                        rgb(0x292929)
                    })
                    .hover(|style| style.bg(rgb(0x333333)))
                    .child(file.name().to_owned())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.select_log(file.clone(), cx);
                        cx.notify();
                    }))
            }));
        page.child(div().flex_1().min_h_0().flex().child(files).child(document))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::Viewer;

    #[test]
    fn search_keeps_utf8_ranges_and_wraps_in_both_directions() {
        let mut viewer = Viewer::default();
        let text = "🌲 First error\r\nNext: ERROR, error\n";
        viewer.replace(Some(text.into()));
        viewer.set_query("Error".into());
        assert_eq!(viewer.text(), text);
        assert_eq!(viewer.matches.len(), 3);
        let first = &viewer.matches[0];
        assert_eq!(&viewer.lines[first.line][first.bytes.clone()], "error");
        assert_eq!(viewer.active, Some(0));
        viewer.step(true);
        assert_eq!(viewer.active, Some(2));
        viewer.step(false);
        assert_eq!(viewer.active, Some(0));
        viewer.set_query("missing".into());
        assert_eq!(viewer.active, None);
    }
}
