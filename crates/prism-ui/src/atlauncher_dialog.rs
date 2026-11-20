use crate::{
    dialogs::{finish_create, CreateInstanceState},
    main_window::MainWindow,
    theme::Button,
};
use gpui_kit::{
    base::Disableable,
    component::{
        button::ButtonVariants,
        checkbox::Checkbox,
        input::{Input, InputEvent, InputState},
        text::TextView,
    },
    *,
};
use prism_core::modplatform::atlauncher::{self as atl, IndexedPack, Manifest, Selection};
use std::sync::Arc;

pub struct AtLauncherDialog {
    query: Entity<InputState>,
    _query_subscription: Subscription,
    packs: Vec<IndexedPack>,
    pack: Option<IndexedPack>,
    version: String,
    manifest: Option<Manifest>,
    selection: Selection,
    error: Option<String>,
    loading: bool,
    installing: bool,
    warning: Option<(String, String)>,
    pending_selection: Option<Selection>,
}
pub fn open(
    this: &mut MainWindow,
    _: &ClickEvent,
    window: &mut Window,
    cx: &mut Context<MainWindow>,
) {
    let query = cx.new(|cx| InputState::new(window, cx).placeholder("Search ATLauncher packs…"));
    let query_subscription = cx.subscribe(&query, |_, _, _: &InputEvent, cx| cx.notify());
    let Some(d) = this.create_dialog.as_mut() else {
        return;
    };
    d.atlauncher = Some(AtLauncherDialog {
        query,
        _query_subscription: query_subscription,
        packs: vec![],
        pack: None,
        version: String::new(),
        manifest: None,
        selection: Selection::default(),
        error: None,
        loading: true,
        installing: false,
        warning: None,
        pending_selection: None,
    });
    let task = this.app.read(cx).runtime.handle().spawn(async {
        atl::list(&prism_core::net::Downloader::new())
            .await
            .map_err(|e| e.to_string())
    });
    cx.spawn(async move |this, cx| {
        let result = task.await.map_err(|e| e.to_string()).and_then(|r| r);
        let _ = this.update(cx, |this, cx| {
            if let Some(d) = this
                .create_dialog
                .as_mut()
                .and_then(|d| d.atlauncher.as_mut())
            {
                d.loading = false;
                match result {
                    Ok(p) => d.packs = p,
                    Err(e) => d.error = Some(e),
                }
            }
            cx.notify();
        });
    })
    .detach();
    cx.notify();
}
fn load(this: &mut MainWindow, pack: IndexedPack, version: String, cx: &mut Context<MainWindow>) {
    let Some(d) = this
        .create_dialog
        .as_mut()
        .and_then(|d| d.atlauncher.as_mut())
    else {
        return;
    };
    d.pack = Some(pack.clone());
    d.version = version.clone();
    d.manifest = None;
    d.loading = true;
    d.error = None;
    d.warning = None;
    d.pending_selection = None;
    let key = (pack.name.clone(), version.clone());
    let task = this.app.read(cx).runtime.handle().spawn(async move {
        atl::manifest(&prism_core::net::Downloader::new(), &pack.name, &version)
            .await
            .map_err(|e| e.to_string())
    });
    cx.spawn(async move |this, cx| {
        let result = task.await.map_err(|e| e.to_string()).and_then(|r| r);
        let _ = this.update(cx, |this, cx| {
            if let Some(d) = this
                .create_dialog
                .as_mut()
                .and_then(|d| d.atlauncher.as_mut())
                .filter(|d| d.pack.as_ref().is_some_and(|p| p.name == key.0) && d.version == key.1)
            {
                d.loading = false;
                match result {
                    Ok(m) => match Selection::defaults(&m) {
                        Ok(s) => {
                            d.selection = s;
                            d.manifest = Some(m);
                        }
                        Err(e) => d.error = Some(e.to_string()),
                    },
                    Err(e) => d.error = Some(e),
                }
            }
            cx.notify();
        });
    })
    .detach();
    cx.notify();
}
fn rich(id: impl Into<SharedString>, text: String) -> AnyElement {
    TextView::html(id.into(), text)
        .image_source(|_| gpui::ImageSource::Custom(Arc::new(|_, _| None)))
        .on_link_click(|url, _, _, cx| {
            let u = url.as_str();
            if (u.starts_with("https://") || u.starts_with("http://"))
                && !u.chars().any(char::is_control)
            {
                cx.open_url(u);
            }
        })
        .into_any_element()
}
fn modify(this: &mut MainWindow, name: &str, confirmed: bool, cx: &mut Context<MainWindow>) {
    if let Some(d) = this
        .create_dialog
        .as_mut()
        .and_then(|d| d.atlauncher.as_mut())
    {
        if !confirmed {
            d.pending_selection = None;
        }
        if confirmed {
            if let Some(selection) = d.pending_selection.take() {
                d.selection = selection;
                cx.notify();
                return;
            }
        }
        if let Some(m) = &d.manifest {
            let enable = !d.selection.enabled.contains(name);
            if enable && !confirmed {
                let mut next = d.selection.clone();
                if let Err(e) = next.set(m, name, true) {
                    d.error = Some(e.to_string());
                    cx.notify();
                    return;
                }
                let warnings: Vec<_> = m
                    .mods
                    .iter()
                    .filter(|e| {
                        next.enabled.contains(&e.name) && !d.selection.enabled.contains(&e.name)
                    })
                    .filter_map(|e| m.warnings.get(&e.warning))
                    .cloned()
                    .collect();
                if !warnings.is_empty() {
                    d.warning = Some((name.into(), warnings.join("\n\n")));
                    cx.notify();
                    return;
                }
            }
            if let Err(e) = d.selection.set(m, name, enable) {
                d.error = Some(e.to_string());
            }
        }
    }
    cx.notify();
}
fn install(this: &mut MainWindow, _: &ClickEvent, _: &mut Window, cx: &mut Context<MainWindow>) {
    let Some(dialog) = this.create_dialog.as_mut() else {
        return;
    };
    let name = dialog.name_input.read(cx).value().trim().to_string();
    let group = dialog.group_input.read(cx).value().trim().to_string();
    let Some(d) = dialog.atlauncher.as_mut() else {
        return;
    };
    if name.is_empty() {
        d.error = Some("Enter an instance name".into());
        cx.notify();
        return;
    }
    let (Some(p), Some(m)) = (d.pack.clone(), d.manifest.clone()) else {
        return;
    };
    if d.installing {
        return;
    }
    d.installing = true;
    d.error = None;
    let s = d.selection.clone();
    let v = d.version.clone();
    let paths = this.app.read(cx).paths.clone();
    let task = this.app.read(cx).runtime.handle().spawn(async move {
        atl::install(
            &paths,
            &prism_core::net::Downloader::new(),
            &p.name,
            &v,
            &name,
            &m,
            &s,
        )
        .await
        .map_err(|e| e.to_string())
    });
    cx.spawn(async move |this, cx| {
        let result = task.await.map_err(|e| e.to_string()).and_then(|r| r);
        let _ = this.update(cx, |this, cx| match result {
            Ok(i) => finish_create(this, &group, Ok(i), cx),
            Err(e) => {
                if let Some(d) = this
                    .create_dialog
                    .as_mut()
                    .and_then(|d| d.atlauncher.as_mut())
                {
                    d.installing = false;
                    d.error = Some(e);
                }
                cx.notify();
            }
        });
    })
    .detach();
    cx.notify();
}
pub fn render(
    dialog: &mut CreateInstanceState,
    _: &mut Window,
    cx: &mut Context<MainWindow>,
) -> AnyElement {
    let palette = crate::theme::Palette::get(cx);
    let d = dialog.atlauncher.as_mut().unwrap();
    let query = d.query.read(cx).value().to_lowercase();
    let busy = d.installing || d.warning.is_some();
    let mut body = div()
        .id("atl-body")
        .flex_1()
        .min_w_0()
        .min_h_0()
        .overflow_y_scroll()
        .flex()
        .flex_col()
        .gap_2();
    if let Some(p) = &d.pack {
        body = body.child(div().text_lg().child(p.name.clone())).child(
            div()
                .text_sm()
                .text_color(palette.secondary_text)
                .child("Pack version"),
        );
        let mut versions = div().flex().flex_wrap().gap_1();
        for r in &p.versions {
            let pack = p.clone();
            let v = r.version.clone();
            let button = Button::new(SharedString::from(format!("atl-version-{v}")))
                .label(v.clone())
                .disabled(busy || d.loading);
            versions = versions.child(
                (if d.version == v {
                    button.primary()
                } else {
                    button
                })
                .on_click(cx.listener(move |this: &mut MainWindow, _, _, cx| {
                    load(this, pack.clone(), v.clone(), cx)
                })),
            );
        }
        body = body.child(versions);
        body = body.child(
            div()
                .text_sm()
                .text_color(palette.secondary_text)
                .child("About this pack · scroll to read more"),
        );
        body = body.child(
            div()
                .id("atl-description-scroll")
                .max_h(px(160.))
                .p_3()
                .bg(palette.panel)
                .rounded_md()
                .flex_shrink_0()
                .overflow_y_scroll()
                .child(rich("atl-description", p.description.clone())),
        );
        if let Some(m) = &d.manifest {
            let optional_count = m
                .mods
                .iter()
                .filter(|e| e.client && e.optional && !e.hidden && !e.library)
                .count();
            body = body.child(div().text_sm().text_color(palette.secondary_text).child(format!(
                    "Minecraft {} · Optional mods selected: {}",
                    m.minecraft,
                    m.mods
                        .iter()
                        .filter(|e| e.client
                            && e.optional
                            && !e.hidden
                            && !e.library
                            && d.selection.enabled.contains(&e.name))
                        .count()
                )));
            if let Some(message) = m.messages.get("install") {
                body = body.child(rich("atl-message", message.clone()));
            }
            if optional_count == 0 {
                body = body.child(
                    div()
                        .text_color(palette.secondary_text)
                        .child("No optional mods. All required client mods will be installed."),
                );
            } else {
                body = body.child(div().text_lg().child("Optional mods"));
                body = body.child(
                    div()
                        .flex()
                        .gap_2()
                        .child(
                            Button::new("atl-recommended")
                                .label("Recommended")
                                .disabled(busy)
                                .on_click(cx.listener(|this: &mut MainWindow, _, _, cx| {
                                    if let Some(d) = this
                                        .create_dialog
                                        .as_mut()
                                        .and_then(|d| d.atlauncher.as_mut())
                                    {
                                        if let Some(m) = &d.manifest {
                                            match Selection::recommended(m) {
                                                Ok(s) => {
                                                    let warnings = m
                                                        .mods
                                                        .iter()
                                                        .filter(|e| {
                                                            s.enabled.contains(&e.name)
                                                                && !d
                                                                    .selection
                                                                    .enabled
                                                                    .contains(&e.name)
                                                        })
                                                        .filter_map(|e| m.warnings.get(&e.warning))
                                                        .cloned()
                                                        .collect::<Vec<_>>();
                                                    if warnings.is_empty() {
                                                        d.selection = s;
                                                    } else {
                                                        d.pending_selection = Some(s);
                                                        d.warning = Some((
                                                            String::new(),
                                                            warnings.join("\n\n"),
                                                        ));
                                                    }
                                                }
                                                Err(e) => d.error = Some(e.to_string()),
                                            }
                                        }
                                    }
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("atl-clear")
                                .label("Clear optional")
                                .disabled(busy)
                                .on_click(cx.listener(|this: &mut MainWindow, _, _, cx| {
                                    if let Some(d) = this
                                        .create_dialog
                                        .as_mut()
                                        .and_then(|d| d.atlauncher.as_mut())
                                    {
                                        if let Some(m) = &d.manifest {
                                            match Selection::clear(m) {
                                                Ok(s) => d.selection = s,
                                                Err(e) => d.error = Some(e.to_string()),
                                            }
                                        }
                                    }
                                    cx.notify();
                                })),
                        ),
                );
            }
            for (i, e) in m
                .mods
                .iter()
                .enumerate()
                .filter(|(_, e)| e.client && e.optional && !e.hidden && !e.library)
            {
                let name = e.name.clone();
                let checked = d.selection.enabled.contains(&e.name);
                let colour = m
                    .colours
                    .get(&e.colour)
                    .and_then(|s| u32::from_str_radix(s.trim_start_matches('#'), 16).ok())
                    .map(|value| Hsla::from(rgb(value)))
                    .unwrap_or(palette.text);
                body = body.child(
                    div()
                        .relative()
                        .children((!checked).then(|| {
                            div()
                                .absolute()
                                .left(px(-1.))
                                .top(px(1.))
                                .size(px(18.))
                                .rounded(px(4.))
                                .border_1()
                                .border_color(palette.muted_text)
                        }))
                        .child(
                            Checkbox::new(SharedString::from(format!("atl-mod-{i}")))
                                .accessibility_label(e.name.clone())
                                .checked(checked)
                                .child(div().text_color(colour).child(e.name.clone()))
                                .disabled(busy)
                                .on_click(cx.listener(
                                    move |this: &mut MainWindow, _: &bool, _, cx| {
                                        modify(this, &name, false, cx)
                                    },
                                )),
                        ),
                );
                let mut details = Vec::new();
                if !e.group.is_empty() {
                    details.push(format!("Group: {} (choose one)", e.group));
                }
                if !e.depends.is_empty() {
                    details.push(format!("Requires: {}", e.depends.join(", ")));
                }
                if !details.is_empty() {
                    body = body.child(
                        div()
                            .pl_6()
                            .text_sm()
                            .text_color(palette.secondary_text)
                            .child(details.join(" · ")),
                    );
                }
                if !e.description.is_empty() {
                    body = body.child(div().pl_6().child(rich(
                        format!("atl-mod-description-{i}"),
                        e.description.clone(),
                    )));
                }
            }
        }
    } else {
        body = body.child(div().text_lg().child("Choose a pack"))
            .child(div().text_color(palette.secondary_text).child("Browse public ATLauncher packs or search by name. Select a pack to see its versions and optional mods."));
    }
    let mut packs = div()
        .id("atl-packs-list")
        .min_h_0()
        .flex_1()
        .overflow_y_scroll()
        .flex()
        .flex_col()
        .gap_1();
    let mut count = 0;
    for (i, p) in d
        .packs
        .iter()
        .enumerate()
        .filter(|(_, p)| p.name.to_lowercase().contains(&query))
    {
        count += 1;
        let pack = p.clone();
        let version = p.versions[0].version.clone();
        let button = Button::new(SharedString::from(format!("atl-pack-{i}")))
            .label(p.name.clone())
            .disabled(busy);
        packs = packs.child(
            (if d
                .pack
                .as_ref()
                .is_some_and(|selected| selected.name == p.name)
            {
                button.primary()
            } else {
                button
            })
            .on_click(cx.listener(move |this: &mut MainWindow, _, _, cx| {
                load(this, pack.clone(), version.clone(), cx)
            })),
        );
    }
    if count == 0 && !d.loading {
        packs = packs.child(
            div()
                .p_2()
                .text_color(palette.secondary_text)
                .child("No packs match your search."),
        );
    }
    if d.loading {
        body = body.child("Loading ATLauncher…");
    }
    if d.installing {
        body = body.child(div().p_3().border_1().border_color(palette.muted_text).rounded_md().child("Installing… Downloading and verifying pack files. This may take a few minutes."));
    }
    if let Some((name, warning)) = d.warning.clone() {
        body = body.child(rich("atl-warning", warning)).child(
            div()
                .flex()
                .gap_2()
                .child(
                    Button::new("atl-warning-yes")
                        .label("Enable anyway")
                        .on_click(cx.listener(move |this: &mut MainWindow, _, _, cx| {
                            if let Some(d) = this
                                .create_dialog
                                .as_mut()
                                .and_then(|d| d.atlauncher.as_mut())
                            {
                                d.warning = None;
                            }
                            modify(this, &name, true, cx);
                        })),
                )
                .child(
                    Button::new("atl-warning-no")
                        .label("Cancel warning")
                        .on_click(cx.listener(|this: &mut MainWindow, _, _, cx| {
                            if let Some(d) = this
                                .create_dialog
                                .as_mut()
                                .and_then(|d| d.atlauncher.as_mut())
                            {
                                d.warning = None;
                                d.pending_selection = None;
                            }
                            cx.notify();
                        })),
                ),
        );
    }
    let can_install = d.manifest.is_some() && !d.loading && d.warning.is_none();
    let panel = div()
        .w(px(1120.))
        .h(px(760.))
        .max_w(relative(0.94))
        .max_h(relative(0.90))
        .bg(palette.base)
        .p_4()
        .rounded_lg()
        .flex()
        .flex_col()
        .gap_2()
        .child(div().text_lg().child("Create Instance — ATLauncher"))
        .child(
            div()
                .flex()
                .gap_3()
                .child(
                    div()
                        .flex_1()
                        .child(div().text_sm().child("Instance name"))
                        .child(Input::new(&dialog.name_input)),
                )
                .child(
                    div()
                        .flex_1()
                        .child(div().text_sm().child("Group (optional)"))
                        .child(Input::new(&dialog.group_input)),
                ),
        )
        .child(
            div()
                .flex()
                .flex_1()
                .min_h_0()
                .gap_4()
                .child(
                    div()
                        .w(px(288.))
                        .flex_shrink_0()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(Input::new(&d.query))
                        .child(div().text_sm().text_color(palette.secondary_text).child(format!(
                            "{count} public {}",
                            if count == 1 { "pack" } else { "packs" }
                        )))
                        .child(packs),
                )
                .child(body),
        )
        .children(
            d.error
                .as_ref()
                .map(|e| div().text_color(palette.error).child(e.clone())),
        )
        .child(
            div()
                .flex()
                .gap_2()
                .child(
                    Button::new("atl-back")
                        .label("Back")
                        .disabled(busy)
                        .on_click(cx.listener(|this: &mut MainWindow, _, _, cx| {
                            if let Some(d) = this.create_dialog.as_mut() {
                                d.atlauncher = None;
                            }
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("atl-packs")
                        .label("Packs")
                        .disabled(busy)
                        .on_click(cx.listener(|this: &mut MainWindow, _, _, cx| {
                            if let Some(d) = this
                                .create_dialog
                                .as_mut()
                                .and_then(|d| d.atlauncher.as_mut())
                            {
                                d.pack = None;
                                d.manifest = None;
                                d.loading = false;
                            }
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("atl-install")
                        .primary()
                        .label(if d.installing {
                            "Installing…"
                        } else {
                            "Install"
                        })
                        .disabled(busy || !can_install)
                        .on_click(cx.listener(install)),
                ),
        );
    div()
        .absolute()
        .size_full()
        .top_0()
        .left_0()
        .bg(gpui_kit::black().opacity(0.5))
        .flex()
        .items_center()
        .justify_center()
        .child(panel)
        .into_any_element()
}
