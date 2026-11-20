use crate::state::App as LauncherApp;
use gpui_kit::{
    base::{Disableable, Radio},
    component::{
        button::Button,
        input::{Input, InputState},
    },
    *,
};
use prism_core::skins::{MinecraftProfile, Skin, SkinClient, SkinError, SkinLibrary, SkinModel};
use std::{collections::HashMap, path::PathBuf, sync::Arc};

pub(crate) fn open(app: Entity<LauncherApp>, account_id: String, cx: &mut gpui::App) {
    let options = WindowOptions {
        titlebar: Some(TitlebarOptions {
            title: Some("Manage Skins".into()),
            ..Default::default()
        }),
        window_min_size: Some(Size {
            width: px(820.),
            height: px(680.),
        }),
        ..Default::default()
    };
    if let Err(error) = open_window(options, cx, |window, cx| {
        cx.new(|cx| SkinsDialog::new(app, account_id, window, cx))
    }) {
        tracing::error!("Could not open skin manager: {error}");
    }
}

struct Preview {
    classic: Arc<RenderImage>,
    slim: Arc<RenderImage>,
    classic_thumb: Arc<RenderImage>,
    slim_thumb: Arc<RenderImage>,
}

fn front(texture: &image::RgbaImage, model: SkinModel) -> image::RgbaImage {
    let mut output = image::RgbaImage::new(16, 32);
    let legacy = texture.height() == 32;
    let arm = if model == SkinModel::Slim { 3 } else { 4 };
    let mut part = |sx, sy, w, h, x, y, mirror| {
        let mut region = image::imageops::crop_imm(texture, sx, sy, w, h).to_image();
        if mirror {
            region = image::imageops::flip_horizontal(&region);
        }
        image::imageops::overlay(&mut output, &region, x, y);
    };
    part(8, 8, 8, 8, 4, 0, false);
    part(20, 20, 8, 12, 4, 8, false);
    part(44, 20, arm, 12, (4 - arm) as i64, 8, false);
    part(
        if legacy { 44 } else { 36 },
        if legacy { 20 } else { 52 },
        arm,
        12,
        12,
        8,
        legacy,
    );
    part(4, 20, 4, 12, 4, 20, false);
    part(
        if legacy { 4 } else { 20 },
        if legacy { 20 } else { 52 },
        4,
        12,
        8,
        20,
        legacy,
    );
    if !legacy {
        part(20, 36, 8, 12, 4, 8, false);
        part(44, 36, arm, 12, (4 - arm) as i64, 8, false);
        part(52, 52, arm, 12, 12, 8, false);
        part(4, 36, 4, 12, 4, 20, false);
        part(4, 52, 4, 12, 8, 20, false);
    }
    let has_hat = !legacy || (0..32).any(|y| (32..64).any(|x| texture.get_pixel(x, y)[3] < 128));
    if has_hat {
        part(40, 8, 8, 8, 4, 0, false);
    }
    output
}

fn preview(texture: &image::RgbaImage, model: SkinModel, scale: u32) -> Arc<RenderImage> {
    render_image(image::imageops::resize(
        &front(texture, model),
        16 * scale,
        32 * scale,
        image::imageops::FilterType::Nearest,
    ))
}

fn render_image(mut pixels: image::RgbaImage) -> Arc<RenderImage> {
    for pixel in pixels.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Arc::new(RenderImage::new([image::Frame::new(pixels)]))
}

fn cape_front(texture: &image::RgbaImage) -> Option<image::RgbaImage> {
    if texture.dimensions() != (64, 32) {
        return None;
    }
    Some(image::imageops::resize(
        &image::imageops::crop_imm(texture, 1, 1, 10, 16).to_image(),
        20,
        32,
        image::imageops::FilterType::Nearest,
    ))
}

struct Download {
    name: String,
    bytes: Vec<u8>,
    model: SkinModel,
    url: Option<String>,
}

fn skin_error(error: SkinError) -> String {
    match error {
        SkinError::Http(error) => skin_http_error(error.status().map(|s| s.as_u16())).into(),
        error => error.to_string(),
    }
}

fn skin_http_error(status: Option<u16>) -> &'static str {
    match status {
        Some(429) => "Minecraft's skin service is busy. Please try again shortly.",
        Some(404) => "That player or skin could not be found. Check the name or URL.",
        Some(401) => "Your session has expired. Please sign in again.",
        Some(403) => "The skin service denied this request. Please try again later.",
        _ => "Could not contact the skin service. Check your connection and try again.",
    }
}

struct SkinsDialog {
    app: Entity<LauncherApp>,
    account_id: String,
    profile: Option<MinecraftProfile>,
    selected: Option<String>,
    previews: HashMap<String, Preview>,
    cape_previews: HashMap<String, Arc<RenderImage>>,
    name_for: Option<String>,
    source: Entity<InputState>,
    name: Entity<InputState>,
    busy: bool,
    error: Option<String>,
    notice: Option<String>,
    confirm_delete: bool,
}

impl SkinsDialog {
    fn new(
        app: Entity<LauncherApp>,
        account_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let source =
            cx.new(|cx| InputState::new(window, cx).placeholder("Skin URL or Minecraft username"));
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("Skin name"));
        let error = app.update(cx, |app, _| {
            if app.skins.is_none() {
                match SkinLibrary::load(&app.paths) {
                    Ok(library) => app.skins = Some(library),
                    Err(e) => return Some(skin_error(e)),
                }
            }
            None
        });
        cx.observe(&app, |this, _, cx| {
            this.cache_missing(cx);
            cx.notify();
        })
        .detach();
        let mut this = Self {
            app,
            account_id,
            profile: None,
            selected: None,
            previews: HashMap::new(),
            cape_previews: HashMap::new(),
            name_for: None,
            source,
            name,
            busy: false,
            error,
            notice: None,
            confirm_delete: false,
        };
        this.cache_missing(cx);
        if this.error.is_none() {
            this.load_profile(cx);
        }
        this
    }

    fn account_exists(&self, cx: &Context<Self>) -> bool {
        self.app.read(cx).accounts.accounts.iter().any(|a| {
            a.id == self.account_id && a.account_type == prism_core::auth::AccountType::Msa
        })
    }

    fn ready(&mut self, cx: &mut Context<Self>) -> bool {
        if self.busy {
            return false;
        }
        if !self.account_exists(cx) {
            self.error = Some("This Microsoft account has been removed. Close this window.".into());
            cx.notify();
            return false;
        }
        self.error = None;
        self.notice = None;
        true
    }

    fn cache_missing(&mut self, cx: &Context<Self>) {
        let Some(library) = self.app.read(cx).skins.as_ref() else {
            return;
        };
        self.previews
            .retain(|name, _| library.list().iter().any(|s| &s.name == name));
        for skin in library.list() {
            if self.previews.contains_key(&skin.name) {
                continue;
            }
            let texture = library
                .path(&skin.name)
                .ok()
                .and_then(|path| image::open(path).ok())
                .map(|i| i.into_rgba8());
            if let Some(texture) =
                texture.filter(|t| t.width() == 64 && [32, 64].contains(&t.height()))
            {
                self.previews.insert(
                    skin.name.clone(),
                    Preview {
                        classic: preview(&texture, SkinModel::Classic, 8),
                        slim: preview(&texture, SkinModel::Slim, 8),
                        classic_thumb: preview(&texture, SkinModel::Classic, 2),
                        slim_thumb: preview(&texture, SkinModel::Slim, 2),
                    },
                );
            }
        }
    }

    fn image(&self, skin: &Skin, thumbnail: bool) -> Option<Arc<RenderImage>> {
        self.previews
            .get(&skin.name)
            .map(|p| match (skin.model, thumbnail) {
                (SkinModel::Classic, false) => p.classic.clone(),
                (SkinModel::Slim, false) => p.slim.clone(),
                (SkinModel::Classic, true) => p.classic_thumb.clone(),
                (SkinModel::Slim, true) => p.slim_thumb.clone(),
            })
    }

    fn skins(&self, cx: &Context<Self>) -> Vec<Skin> {
        self.app
            .read(cx)
            .skins
            .as_ref()
            .map(|l| l.list().to_vec())
            .unwrap_or_default()
    }

    fn selected_skin(&self, cx: &Context<Self>) -> Option<Skin> {
        self.skins(cx)
            .into_iter()
            .find(|s| Some(&s.name) == self.selected.as_ref())
    }

    fn edit<T>(
        &mut self,
        cx: &mut Context<Self>,
        edit: impl FnOnce(&mut SkinLibrary) -> Result<T, prism_core::skins::SkinError>,
    ) -> Option<T> {
        let result = self.app.update(cx, |app, cx| {
            let result = app.skins.as_mut().map(edit);
            cx.notify();
            result
        });
        match result {
            Some(Ok(value)) => {
                self.cache_missing(cx);
                Some(value)
            }
            Some(Err(e)) => {
                self.error = Some(skin_error(e));
                None
            }
            None => {
                self.error = Some("Skin library is unavailable".into());
                None
            }
        }
    }

    fn refresh_accounts(&self, cx: &mut Context<Self>) {
        self.app.update(cx, |app, cx| {
            app.accounts = prism_core::AccountList::load(&app.paths.accounts_file());
            cx.notify();
        });
    }

    fn load_profile(&mut self, cx: &mut Context<Self>) {
        if !self.ready(cx) {
            return;
        }
        self.busy = true;
        let path = self.app.read(cx).paths.accounts_file();
        let id = self.account_id.clone();
        let urls: Vec<_> = self.skins(cx).into_iter().filter_map(|s| s.url).collect();
        let cached_capes: Vec<_> = self.cape_previews.keys().cloned().collect();
        let task = self.app.read(cx).runtime.spawn(async move {
            let profile = SkinClient::new()
                .fetch_profile(&path, &id)
                .await
                .map_err(skin_error)?;
            let mut download = None;
            if let Some(skin) = profile.skins.iter().find(|s| s.state == "ACTIVE") {
                if !urls.contains(&skin.url) {
                    download = Some(
                        SkinLibrary::download_url(&skin.url)
                            .await
                            .map(|bytes| Download {
                                name: profile.name.clone(),
                                bytes,
                                model: skin.variant,
                                url: Some(skin.url.clone()),
                            })
                            .map_err(skin_error),
                    );
                }
            }
            let mut capes = Vec::new();
            for cape in &profile.capes {
                if !cached_capes.contains(&cape.url) {
                    if let Ok(bytes) = SkinLibrary::download_url(&cape.url).await {
                        if let Ok(texture) = image::load_from_memory(&bytes) {
                            if let Some(front) = cape_front(&texture.into_rgba8()) {
                                capes.push((cape.url.clone(), front));
                            }
                        }
                    }
                }
            }
            Ok::<_, String>((profile, download, capes))
        });
        cx.spawn(async move |this, cx| {
            let result = task.await.map_err(|e| e.to_string()).and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                this.refresh_accounts(cx);
                if !this.account_exists(cx) {
                    this.ready(cx);
                    return;
                }
                match result {
                    Ok((profile, download, capes)) => {
                        for (url, image) in capes {
                            this.cape_previews.insert(url, render_image(image));
                        }
                        if let Some(download) = download {
                            match download {
                                Ok(data) => {
                                    if !this.skins(cx).iter().any(|s| s.url == data.url) {
                                        this.import(data, cx);
                                    }
                                }
                                Err(e) => this.error = Some(e),
                            }
                        }
                        let active = profile.skins.iter().find(|s| s.state == "ACTIVE");
                        let selected = this.skins(cx).into_iter().find(|s| {
                            active.is_some_and(|a| s.url.as_deref() == Some(a.url.as_str()))
                        });
                        if let Some(skin) = selected {
                            let cape = profile
                                .capes
                                .iter()
                                .find(|c| c.state == "ACTIVE")
                                .map(|c| c.id.as_str())
                                .unwrap_or("");
                            this.edit(cx, |l| l.set_model(&skin.name, active.unwrap().variant));
                            this.edit(cx, |l| l.set_cape(&skin.name, cape));
                            this.selected = Some(skin.name);
                        }
                        this.profile = Some(profile);
                    }
                    Err(e) => this.error = Some(e),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn import(&mut self, data: Download, cx: &mut Context<Self>) {
        if let Some(skin) = self.edit(cx, |l| {
            l.import_bytes(&data.name, &data.bytes, data.model, data.url)
        }) {
            self.selected = Some(skin.name);
            self.confirm_delete = false;
        }
    }

    fn import_remote(&mut self, username: bool, cx: &mut Context<Self>) {
        if !self.ready(cx) {
            return;
        }
        let source = self.source.read(cx).value().trim().to_string();
        if source.is_empty() {
            self.error = Some("Enter a URL or username first".into());
            cx.notify();
            return;
        }
        self.busy = true;
        let task = self.app.read(cx).runtime.spawn(async move {
            if username {
                let (bytes, model, url) = SkinLibrary::download_username(&source)
                    .await
                    .map_err(skin_error)?;
                Ok(Download {
                    name: source,
                    bytes,
                    model,
                    url: Some(url),
                })
            } else {
                let bytes = SkinLibrary::download_url(&source)
                    .await
                    .map_err(skin_error)?;
                Ok::<_, String>(Download {
                    name: "Downloaded skin".into(),
                    bytes,
                    model: SkinModel::Classic,
                    url: Some(source),
                })
            }
        });
        cx.spawn(async move |this, cx| {
            let result = task.await.map_err(|e| e.to_string()).and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                if !this.ready(cx) {
                    return;
                }
                match result {
                    Ok(data) => this.import(data, cx),
                    Err(e) => this.error = Some(e),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn import_file(&mut self, cx: &mut Context<Self>) {
        if !self.ready(cx) {
            return;
        }
        self.busy = true;
        let paths = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Import Skin PNG".into()),
        });
        cx.spawn(async move |this, cx| {
            let result = paths.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                if !this.ready(cx) {
                    return;
                }
                match result {
                    Ok(Ok(Some(paths))) => {
                        if let Some(path) = paths.first() {
                            if let Some(skin) = this.edit(cx, |l| l.import_file(path)) {
                                this.selected = Some(skin.name);
                            }
                        }
                    }
                    Ok(Ok(None)) => {}
                    Ok(Err(e)) => this.error = Some(e.to_string()),
                    Err(e) => this.error = Some(e.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn apply(&mut self, close: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !self.ready(cx) {
            return;
        }
        let (Some(skin), Some(profile)) = (self.selected_skin(cx), self.profile.as_ref()) else {
            return;
        };
        let active = profile.skins.iter().find(|s| s.state == "ACTIVE");
        let upload = !active.is_some_and(|a| {
            skin.url.as_deref() == Some(a.url.as_str()) && skin.model == a.variant
        });
        let active_cape = profile
            .capes
            .iter()
            .find(|c| c.state == "ACTIVE")
            .map(|c| c.id.as_str())
            .unwrap_or("");
        let change_cape = active_cape != skin.cape_id;
        let file = self
            .app
            .read(cx)
            .skins
            .as_ref()
            .and_then(|l| l.path(&skin.name).ok());
        let Some(file) = file else { return };
        self.busy = true;
        let path = self.app.read(cx).paths.accounts_file();
        let id = self.account_id.clone();
        let name = skin.name.clone();
        let task = self.app.read(cx).runtime.spawn(async move {
            let client = SkinClient::new();
            if upload {
                client
                    .upload_skin(&path, &id, &skin, &file)
                    .await
                    .map_err(skin_error)?;
            }
            if change_cape {
                if skin.cape_id.is_empty() {
                    client.hide_cape(&path, &id).await
                } else {
                    client.set_active_cape(&path, &id, &skin.cape_id).await
                }
                .map_err(skin_error)?;
            }
            client.fetch_profile(&path, &id).await.map_err(skin_error)
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await.map_err(|e| e.to_string()).and_then(|r| r);
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = false;
                this.refresh_accounts(cx);
                if !this.ready(cx) {
                    return;
                }
                match result {
                    Ok(profile) => {
                        if let Some(active) = profile.skins.iter().find(|s| s.state == "ACTIVE") {
                            this.edit(cx, |l| l.set_texture(&name, &active.url, &active.id));
                        }
                        this.profile = Some(profile);
                        this.notice = Some("Skin and cape applied".into());
                        if close && this.error.is_none() {
                            window.remove_window();
                        }
                    }
                    Err(e) => {
                        this.error = Some(e);
                        this.profile = None;
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn rename(&mut self, cx: &mut Context<Self>) {
        if !self.ready(cx) {
            return;
        }
        let Some(skin) = self.selected_skin(cx) else {
            return;
        };
        let name = self.name.read(cx).value().trim().to_string();
        if name.is_empty() {
            self.error = Some("Enter a new skin name".into());
        } else if let Some(name) = self.edit(cx, |l| l.rename(&skin.name, &name)) {
            self.selected = Some(name);
        }
        self.confirm_delete = false;
        cx.notify();
    }

    fn delete(&mut self, cx: &mut Context<Self>) {
        if !self.ready(cx) {
            return;
        }
        let Some(skin) = self.selected_skin(cx) else {
            return;
        };
        if self.profile.as_ref().is_some_and(|p| {
            p.skins
                .iter()
                .any(|a| a.state == "ACTIVE" && skin.url.as_deref() == Some(a.url.as_str()))
        }) {
            self.error = Some("Cannot delete the skin currently in use".into());
        } else if !self.confirm_delete {
            self.confirm_delete = true;
        } else if self.edit(cx, |l| l.delete(&skin.name)).is_some() {
            self.selected = None;
            self.confirm_delete = false;
        }
        cx.notify();
    }
}

fn choice(
    id: String,
    label: String,
    checked: bool,
    busy: bool,
    thumbnail: Option<Div>,
    click: impl Fn(&bool, &mut Window, &mut gpui::App) + 'static,
) -> Radio {
    Radio::new(SharedString::from(id))
        .accessibility_label(label.clone())
        .checked(checked)
        .disabled(busy)
        .flex()
        .items_center()
        .gap_2()
        .p_2()
        .rounded_md()
        .border_1()
        .border_color(if checked {
            rgb(0x789ac6)
        } else {
            rgb(0x505050)
        })
        .bg(if checked {
            rgb(0x344b69)
        } else {
            rgb(0x292929)
        })
        .child(
            div()
                .size(px(16.))
                .flex_shrink_0()
                .rounded_full()
                .border_1()
                .border_color(rgb(0xb0b0b0))
                .flex()
                .items_center()
                .justify_center()
                .children(checked.then(|| div().size(px(8.)).rounded_full().bg(rgb(0xffffff)))),
        )
        .children(thumbnail)
        .child(div().flex_1().min_w_0().child(label))
        .on_change(move |checked, _, window, cx| click(&checked, window, cx))
}

impl Render for SkinsDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let removed = !self.account_exists(cx);
        let busy = self.busy || removed || self.app.read(cx).skins.is_none();
        let selected = self.selected_skin(cx);
        let selected_name = selected.as_ref().map(|s| s.name.clone());
        if self.name_for != selected_name {
            self.name_for = selected_name.clone();
            self.name.update(cx, |input, cx| {
                input.set_value(selected_name.unwrap_or_default(), window, cx);
            });
        }
        let mut list = div()
            .id("skin-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_1();
        for (i, skin) in self.skins(cx).into_iter().enumerate() {
            let name = skin.name.clone();
            let active = self.selected.as_ref() == Some(&name);
            let mut row = div()
                .id(("skin", i))
                .flex()
                .items_center()
                .gap_2()
                .p_2()
                .flex_shrink_0()
                .rounded_md()
                .bg(if active { rgb(0x3d5a80) } else { rgb(0x2b2b2b) });
            if let Some(image) = self.image(&skin, true) {
                row = row.child(img(ImageSource::Render(image)).w(px(32.)).h(px(64.)));
            }
            list = list.child(
                row.child(div().flex_1().child(name.clone()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.busy {
                            this.selected = Some(name.clone());
                            this.confirm_delete = false;
                            cx.notify();
                        }
                    })),
            );
        }
        let mut detail = div()
            .id("skin-details")
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_y_scroll()
            .p_3()
            .rounded_md()
            .bg(rgb(0x252525))
            .flex()
            .flex_col()
            .items_center()
            .gap_3();
        let mut settings = div()
            .w_full()
            .max_w(px(400.))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap_2();
        if let Some(skin) = &selected {
            if let Some(image) = self.image(skin, false) {
                detail = detail.child(
                    div()
                        .flex()
                        .justify_center()
                        .flex_shrink_0()
                        .child(img(ImageSource::Render(image)).w(px(128.)).h(px(256.))),
                );
            }
            let mut models = div().flex().gap_2();
            for (label, model) in [("Classic", SkinModel::Classic), ("Slim", SkinModel::Slim)] {
                let name = skin.name.clone();
                models = models.child(
                    choice(
                        format!("model-{label}"),
                        label.into(),
                        skin.model == model,
                        busy,
                        None,
                        cx.listener(move |this, _, _, cx| {
                            if this.ready(cx) {
                                this.edit(cx, |l| l.set_model(&name, model));
                                cx.notify();
                            }
                        }),
                    )
                    .flex_1(),
                );
            }
            settings = settings
                .child(div().font_weight(FontWeight::SEMIBOLD).child("Model"))
                .child(models)
                .child(div().mt_1().font_weight(FontWeight::SEMIBOLD).child("Cape"));
            let capes: Vec<_> =
                std::iter::once((String::new(), "No Cape".to_string(), String::new()))
                    .chain(self.profile.as_ref().into_iter().flat_map(|p| {
                        p.capes
                            .iter()
                            .map(|c| (c.id.clone(), c.alias.clone(), c.url.clone()))
                    }))
                    .collect();
            for (i, (cape, alias, url)) in capes.into_iter().enumerate() {
                let name = skin.name.clone();
                let mut thumbnail = div().w(px(20.)).h(px(32.)).flex_shrink_0();
                if let Some(image) = self.cape_previews.get(&url) {
                    thumbnail = thumbnail.child(
                        img(ImageSource::Render(image.clone()))
                            .w(px(20.))
                            .h(px(32.)),
                    );
                }
                settings = settings.child(
                    choice(
                        format!("cape-{i}"),
                        alias,
                        skin.cape_id == cape,
                        busy,
                        Some(thumbnail),
                        cx.listener(move |this, _, _, cx| {
                            if this.ready(cx) {
                                this.edit(cx, |l| l.set_cape(&name, &cape));
                                cx.notify();
                            }
                        }),
                    )
                    .w_full(),
                );
            }
            detail = detail.child(settings);
        } else {
            detail = detail.child("Select or import a skin");
        }
        let mut panel = div()
            .size_full()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .bg(rgb(0x202020))
            .text_color(rgb(0xdddddd))
            .child(div().text_size(px(20.)).child("Manage Skins"));
        if self.busy {
            panel = panel.child("Loading / working…");
        }
        if removed {
            panel = panel.child(
                div()
                    .text_color(rgb(0xff8888))
                    .child("This Microsoft account has been removed. Close this window."),
            );
        }
        if let Some(error) = &self.error {
            panel = panel.child(div().text_color(rgb(0xff8888)).child(error.clone()));
        }
        if let Some(notice) = &self.notice {
            panel = panel.child(div().child(notice.clone()));
        }
        panel = panel
            .child(
                div()
                    .id("skin-content")
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .gap_4()
                    .child(
                        div()
                            .w(relative(0.4))
                            .min_w(px(260.))
                            .max_w(px(460.))
                            .min_h_0()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .p_3()
                            .rounded_md()
                            .bg(rgb(0x252525))
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("Skin Library"),
                            )
                            .child(list),
                    )
                    .child(detail),
            )
            .child(
                div()
                    .flex()
                    .gap_4()
                    .pt_3()
                    .border_t_1()
                    .border_color(rgb(0x404040))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child("Import from URL or username")
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(Input::new(&self.source).flex_1().min_w_0())
                                    .child(
                                        Button::new("skin-url")
                                            .label("URL")
                                            .disabled(busy)
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.import_remote(false, cx)
                                            })),
                                    )
                                    .child(
                                        Button::new("skin-user")
                                            .label("Username")
                                            .disabled(busy)
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.import_remote(true, cx)
                                            })),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .w(relative(0.45))
                            .min_w(px(300.))
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child("Selected skin name")
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(Input::new(&self.name).flex_1().min_w_0())
                                    .child(
                                        Button::new("skin-rename")
                                            .label("Rename")
                                            .disabled(busy || selected.is_none())
                                            .on_click(
                                                cx.listener(|this, _, _, cx| this.rename(cx)),
                                            ),
                                    )
                                    .child(
                                        Button::new("skin-delete")
                                            .label(if self.confirm_delete {
                                                "Confirm Delete"
                                            } else {
                                                "Delete"
                                            })
                                            .disabled(busy || selected.is_none())
                                            .on_click(
                                                cx.listener(|this, _, _, cx| this.delete(cx)),
                                            ),
                                    ),
                            ),
                    ),
            );
        if self.confirm_delete {
            panel = panel.child(
                div()
                    .flex()
                    .gap_2()
                    .child("Permanently delete the selected skin?")
                    .child(
                        Button::new("skin-keep")
                            .label("Keep Skin")
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.confirm_delete = false;
                                cx.notify();
                            })),
                    ),
            );
        }
        panel.child(
            div()
                .flex()
                .justify_end()
                .items_center()
                .gap_2()
                .child(
                    Button::new("skin-file")
                        .label("Import File…")
                        .disabled(busy)
                        .on_click(cx.listener(|this, _, _, cx| this.import_file(cx))),
                )
                .child(
                    Button::new("skin-folder")
                        .label("Open Folder")
                        .disabled(busy)
                        .on_click(cx.listener(|this, _, _, cx| {
                            let dir = this.app.read(cx).paths.skins_dir();
                            if let Err(error) = open_folder(dir) {
                                this.error = Some(format!("Could not open skin folder: {error}"));
                                cx.notify();
                            }
                        })),
                )
                .child(div().flex_1())
                .child(
                    Button::new("skin-reload")
                        .label("Refresh Profile")
                        .disabled(busy)
                        .on_click(cx.listener(|this, _, _, cx| this.load_profile(cx))),
                )
                .child(
                    Button::new("skin-apply")
                        .label("Apply")
                        .disabled(busy || selected.is_none() || self.profile.is_none())
                        .on_click(cx.listener(|this, _, window, cx| this.apply(false, window, cx))),
                )
                .child(
                    Button::new("skin-ok")
                        .label("OK")
                        .disabled(busy || selected.is_none() || self.profile.is_none())
                        .on_click(cx.listener(|this, _, window, cx| this.apply(true, window, cx))),
                )
                .child(
                    Button::new("skin-cancel")
                        .label("Cancel")
                        .disabled(self.busy)
                        .on_click(|_, window, _| window.remove_window()),
                ),
        )
    }
}

fn open_folder(path: PathBuf) -> std::io::Result<()> {
    #[cfg(target_os = "linux")]
    let child = std::process::Command::new("xdg-open").arg(path).spawn();
    #[cfg(target_os = "macos")]
    let child = std::process::Command::new("open").arg(path).spawn();
    #[cfg(target_os = "windows")]
    let child = std::process::Command::new("explorer").arg(path).spawn();
    child.map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::{cape_front, front, skin_http_error, SkinModel};

    #[test]
    fn cape_front_crops_and_upscales_without_blending() {
        let mut texture = image::RgbaImage::new(64, 32);
        assert!(cape_front(&texture).unwrap().pixels().all(|p| p[3] == 0));
        texture.put_pixel(1, 1, image::Rgba([1, 2, 3, 255]));
        texture.put_pixel(10, 16, image::Rgba([4, 5, 6, 255]));
        let front = cape_front(&texture).unwrap();
        assert_eq!(front.dimensions(), (20, 32));
        assert_eq!(front.get_pixel(0, 0).0, [1, 2, 3, 255]);
        assert_eq!(front.get_pixel(1, 1).0, [1, 2, 3, 255]);
        assert_eq!(front.get_pixel(19, 31).0, [4, 5, 6, 255]);
        assert!(cape_front(&image::RgbaImage::new(64, 64)).is_none());
        assert!(cape_front(&image::RgbaImage::new(0, 0)).is_none());
    }

    #[test]
    fn skin_http_errors_are_actionable_without_raw_urls() {
        assert!(skin_http_error(Some(429)).contains("try again shortly"));
        assert!(skin_http_error(Some(404)).contains("Check the name or URL"));
        assert!(skin_http_error(Some(401)).contains("sign in again"));
        assert!(skin_http_error(Some(403)).contains("denied"));
        assert_eq!(skin_http_error(None), skin_http_error(Some(500)));
        assert!(!skin_http_error(None).contains("http"));
    }

    #[test]
    fn skin_front_places_layers_and_slim_arms() {
        let mut texture = image::RgbaImage::new(64, 64);
        texture.put_pixel(8, 8, image::Rgba([255, 0, 0, 255]));
        texture.put_pixel(40, 8, image::Rgba([0, 255, 0, 255]));
        texture.put_pixel(44, 20, image::Rgba([0, 0, 255, 255]));
        texture.put_pixel(20, 36, image::Rgba([1, 2, 3, 255]));
        let classic = front(&texture, SkinModel::Classic);
        assert_eq!(classic.dimensions(), (16, 32));
        assert_eq!(classic.get_pixel(4, 0).0, [0, 255, 0, 255]);
        assert_eq!(classic.get_pixel(0, 8).0, [0, 0, 255, 255]);
        assert_eq!(classic.get_pixel(4, 8).0, [1, 2, 3, 255]);
        let slim = front(&texture, SkinModel::Slim);
        assert_eq!(slim.get_pixel(0, 8)[3], 0);
        assert_eq!(slim.get_pixel(1, 8).0, [0, 0, 255, 255]);
    }

    #[test]
    fn skin_legacy_mirrors_limbs_and_empty_texture_is_transparent() {
        let mut texture = image::RgbaImage::new(64, 32);
        assert!(front(&texture, SkinModel::Classic)
            .pixels()
            .all(|p| p[3] == 0));
        texture.put_pixel(44, 20, image::Rgba([1, 2, 3, 255]));
        texture.put_pixel(4, 20, image::Rgba([4, 5, 6, 255]));
        let rendered = front(&texture, SkinModel::Classic);
        assert_eq!(rendered.get_pixel(15, 8).0, [1, 2, 3, 255]);
        assert_eq!(rendered.get_pixel(11, 20).0, [4, 5, 6, 255]);
    }
}
