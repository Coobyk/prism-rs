use super::InstanceWindow;
use gpui_kit::base::Disableable;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::text::TextView;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use prism_core::managed_pack::{self, ManagedPack, Snapshot, Version};
use std::{borrow::Cow, sync::Arc};

pub(crate) fn changelog_markdown(text: &str) -> Cow<'_, str> {
    let is_item = |line: &str| {
        let line = line.trim_end();
        line.starts_with("<li>") && line.ends_with("</li>")
    };
    if !text.lines().any(is_item) {
        return Cow::Borrowed(text);
    }

    let mut markdown = String::with_capacity(text.len());
    let mut lines = text.split_inclusive('\n').peekable();
    let mut block_start = true;
    while let Some(line) = lines.next() {
        if block_start && is_item(line) {
            markdown.push_str("<ul>\n");
            markdown.push_str(line.trim_end());
            markdown.push('\n');
            while lines.peek().is_some_and(|line| is_item(line)) {
                markdown.push_str(lines.next().unwrap().trim_end());
                markdown.push('\n');
            }
            markdown.push_str("</ul>\n\n");
        } else {
            markdown.push_str(line);
        }
        block_start = line.trim().is_empty();
    }
    Cow::Owned(markdown)
}

fn render_changelog(text: &str) -> AnyElement {
    TextView::markdown(
        "managed-pack-changelog",
        changelog_markdown(text).into_owned(),
    )
    .image_source(|_| gpui::ImageSource::Custom(Arc::new(|_, _| None)))
    .on_link_click(|target, _, _, cx| {
        let target = target.as_str();
        if (target.starts_with("https://") || target.starts_with("http://"))
            && !target.chars().any(char::is_control)
        {
            cx.open_url(target);
        }
    })
    .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::changelog_markdown;

    #[test]
    fn groups_bare_html_items_for_the_markdown_renderer() {
        let source = "Updated to 1.20.4\n\nAdded:\n\n<li><a href=\"https://modrinth.com/mod/example\">Example &amp; Friends</a></li>\n<li>Another mod</li>\n\n**Notes**";
        let rendered = changelog_markdown(source);
        assert!(rendered.contains("<ul>\n<li><a href="));
        assert!(rendered.contains("<li>Another mod</li>\n</ul>\n\n"));
        assert!(rendered.ends_with("**Notes**"));
    }

    #[test]
    fn leaves_markdown_and_wrapped_html_lists_intact() {
        let source = "## Changes\n\n- First\n\n<ul>\n<li>Second</li>\n</ul>";
        assert_eq!(changelog_markdown(source), source);
    }
}

#[derive(Default)]
pub(super) struct ManagedPackPage {
    pub available: bool,
    pub loaded: bool,
    snapshot: Option<Arc<Snapshot>>,
    current: Option<ManagedPack>,
    versions: Vec<Version>,
    selected: Option<usize>,
    busy: bool,
    status: String,
}

impl ManagedPackPage {
    pub fn new(available: bool) -> Self {
        Self {
            available,
            ..Self::default()
        }
    }
}

impl InstanceWindow {
    pub(super) fn refresh_managed_pack(&mut self, cx: &mut Context<Self>) {
        if self.managed_pack.busy {
            return;
        }
        let app = self.app.read(cx);
        let Some(instance) = app.instances.get(&self.instance_id).cloned() else {
            self.managed_pack.status = "The instance no longer exists.".into();
            cx.notify();
            return;
        };
        let downloader = app.runtime.downloader.clone();
        let task = app.runtime.spawn(async move {
            let snapshot = Snapshot::load(instance)?;
            let versions = managed_pack::versions(&downloader, &snapshot.pack.project_id).await;
            Ok::<_, prism_core::import::ImportError>((snapshot, versions))
        });
        self.managed_pack.loaded = true;
        self.managed_pack.busy = true;
        self.managed_pack.snapshot = None;
        self.managed_pack.versions.clear();
        self.managed_pack.selected = None;
        self.managed_pack.status = "Fetching pack releases…".into();
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                let page = &mut this.managed_pack;
                page.busy = false;
                match result {
                    Ok(Ok((snapshot, versions))) => {
                        page.current = Some(snapshot.pack.clone());
                        page.snapshot = Some(Arc::new(snapshot));
                        page.status = match versions {
                            Ok(versions) if !versions.is_empty() => {
                                page.selected = Some(0);
                                page.versions = versions;
                                String::new()
                            }
                            Ok(_) => "No releases found.".into(),
                            Err(error) => format!("Could not load versions: {error}"),
                        };
                    }
                    Ok(Err(error)) => page.status = format!("Could not load pack: {error}"),
                    Err(error) => page.status = format!("Could not load pack: {error}"),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn confirm_pack_update(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let page = &self.managed_pack;
        let Some(snapshot) = page.snapshot.clone() else {
            return;
        };
        let Some(version) = page
            .selected
            .and_then(|index| page.versions.get(index))
            .cloned()
        else {
            return;
        };
        if page.busy
            || version.id == snapshot.pack.version_id
            || self.app.read(cx).is_running(&self.instance_id)
        {
            return;
        }
        let answer = window.prompt(
            gpui::PromptLevel::Warning,
            &format!(
                "Change {} from {} to {}?\n\nPack-owned files and overrides may change or be removed. Existing saves and other user files are kept.",
                snapshot.pack.name, snapshot.pack.version, version.version_number
            ),
            None,
            &[gpui::PromptButton::cancel("Cancel"), gpui::PromptButton::new("Update Pack")],
            cx,
        );
        cx.spawn(async move |this, cx| {
            if answer.await == Ok(1) {
                let _ = this.update(cx, |this, cx| {
                    this.update_managed_pack(snapshot, version, cx)
                });
            }
        })
        .detach();
    }

    fn update_managed_pack(
        &mut self,
        snapshot: Arc<Snapshot>,
        version: Version,
        cx: &mut Context<Self>,
    ) {
        let page = &mut self.managed_pack;
        if page.busy
            || !page
                .snapshot
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, &snapshot))
        {
            return;
        }
        let state = self.app.read(cx);
        let Some(instance) = state.instances.get(&self.instance_id) else {
            page.status = "The instance no longer exists.".into();
            cx.notify();
            return;
        };
        if state.is_running(&self.instance_id) {
            page.status = "Close Minecraft before updating this pack.".into();
            cx.notify();
            return;
        }
        let lock = match crate::instance_lock::acquire(&state.paths, instance) {
            Ok(lock) => lock,
            Err(error) => {
                page.status = format!("Could not lock instance: {error}");
                cx.notify();
                return;
            }
        };
        page.busy = true;
        page.status = format!("Installing {}…", version.version_number);
        let downloader = state.runtime.downloader.clone();
        let instance = instance.clone();
        let task = state.runtime.spawn(async move {
            let _lock = lock;
            let updated = snapshot.update(&downloader, &version).await?;
            Ok::<_, prism_core::import::ImportError>((updated, Snapshot::load(instance)))
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                let page = &mut this.managed_pack;
                page.busy = false;
                match result {
                    Ok(Ok((updated, refreshed))) => {
                        page.status = format!("Updated to {}.", updated.version);
                        page.current = Some(updated);
                        page.snapshot = refreshed.ok().map(Arc::new);
                    }
                    Ok(Err(error)) => page.status = format!("Update failed: {error}"),
                    Err(error) => page.status = format!("Update failed: {error}"),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn render_managed_pack(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = crate::theme::Palette::get(cx);
        let page = &self.managed_pack;
        let running = self.app.read(cx).is_running(&self.instance_id);
        let current = page.current.as_ref();
        let selected = page.selected.and_then(|index| page.versions.get(index));
        let can_update = !running
            && !page.busy
            && page.snapshot.is_some()
            && current
                .zip(selected)
                .is_some_and(|(current, selected)| current.version_id != selected.id);
        let choices = page.versions.iter().enumerate().map(|(index, version)| {
            let installed = current.is_some_and(|pack| pack.version_id == version.id);
            let label = if installed {
                format!("{} (installed)", version.version_number)
            } else {
                version.version_number.clone()
            };
            div().child(
                Button::new(SharedString::from(format!("pack-release-{index}")))
                    .label(label)
                    .when(page.selected == Some(index), |button| button.primary())
                    .disabled(page.busy)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.managed_pack.selected = Some(index);
                        cx.notify();
                    })),
            )
        });
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .min_w_0()
            .min_h_0()
            .child(
                div()
                    .text_size(px(18.))
                    .font_weight(FontWeight::BOLD)
                    .child(
                        current
                            .map(|pack| pack.name.clone())
                            .unwrap_or_else(|| "Managed Pack".into()),
                    ),
            )
            .child(
                div().text_color(palette.secondary_text).child(
                    current
                        .map(|pack| format!("Modrinth  •  Installed: {}", pack.version))
                        .unwrap_or_default(),
                ),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        Button::new("refresh-managed-pack")
                            .label("Refresh")
                            .disabled(page.busy)
                            .on_click(cx.listener(|this, _, _, cx| this.refresh_managed_pack(cx))),
                    )
                    .child(
                        Button::new("update-managed-pack")
                            .label("Update Pack…")
                            .primary()
                            .disabled(!can_update)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.confirm_pack_update(window, cx)
                            })),
                    ),
            )
            .child(div().text_size(px(12.)).child(if running {
                "Close Minecraft before updating this pack.".to_owned()
            } else {
                page.status.clone()
            }))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .gap_3()
                    .child(
                        div()
                            .w(px(180.))
                            .flex_shrink_0()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child("Pack versions")
                            .child(
                                div()
                                    .id("pack-release-list")
                                    .flex_1()
                                    .min_h_0()
                                    .overflow_y_scroll()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .children(choices),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                selected
                                    .map(|version| format!("Changelog — {}", version.name))
                                    .unwrap_or_else(|| "Changelog".into()),
                            )
                            .child(
                                div()
                                    .id("pack-changelog")
                                    .flex_1()
                                    .min_h_0()
                                    .overflow_y_scroll()
                                    .p_3()
                                    .bg(palette.base)
                                    .rounded_md()
                                    .text_size(px(13.))
                                    .child(render_changelog(
                                        selected
                                            .and_then(|version| version.changelog.as_deref())
                                            .filter(|text| !text.is_empty())
                                            .unwrap_or("No changelog available."),
                                    )),
                            ),
                    ),
            )
    }
}
