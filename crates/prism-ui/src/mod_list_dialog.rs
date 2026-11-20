use gpui_kit::base::Disableable;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use prism_core::mod_list::{self, Fields, Format, ModListEntry};
use prism_core::Instance;
use std::{fs::OpenOptions, io::Write, path::PathBuf};

pub(super) fn open(instance: Instance, cx: &mut App) {
    let options = WindowOptions {
        window_min_size: Some(Size {
            width: px(610.),
            height: px(470.),
        }),
        ..WindowOptions::default()
    };
    open_window(options, cx, |window, cx| {
        cx.new(|cx| ModListDialog::new(instance, window, cx))
    })
    .ok();
}

struct ModListDialog {
    name: String,
    entries: Option<Vec<ModListEntry>>,
    format: Format,
    fields: Fields,
    template: Entity<TextareaState>,
    destination: Entity<InputState>,
    suggested_path: String,
    status: String,
}

impl ModListDialog {
    fn new(instance: Instance, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mods = instance.game_root().join("mods");
        let load = cx.background_spawn(async move { mod_list::collect(&mods) });
        cx.spawn(async move |this, cx| {
            let entries = load.await;
            let _ = this.update(cx, |this, cx| {
                this.entries = Some(entries);
                cx.notify();
            });
        })
        .detach();

        let name = instance.name;
        let suggested_path = Self::default_path(&name, Format::Html);
        let destination = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Destination path")
                .default_value(suggested_path.clone())
        });
        let template = cx.new(|cx| {
            TextareaState::new(window, cx)
                .rows(2)
                .default_value("{name} ({url}) [{version}] by {authors}")
        });
        cx.subscribe(&template, |_, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        })
        .detach();
        Self {
            name,
            entries: None,
            format: Format::Html,
            fields: Fields::default(),
            template,
            destination,
            suggested_path,
            status: String::new(),
        }
    }

    fn default_path(name: &str, format: Format) -> String {
        let name = name.replace(['/', '\\', '<', '>', ':', '"', '|', '?', '*'], "_");
        std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map_or_else(|| PathBuf::from("."), PathBuf::from)
            .join(format!("{name}-mods.{}", format.extension()))
            .to_string_lossy()
            .into_owned()
    }

    fn set_format(&mut self, format: Format, window: &mut Window, cx: &mut Context<Self>) {
        let new_path = Self::default_path(&self.name, format);
        if self.destination.read(cx).value().as_str() == self.suggested_path {
            self.destination.update(cx, |state, cx| {
                state.set_value(new_path.clone(), window, cx);
            });
        }
        self.suggested_path = new_path;
        self.format = format;
        self.status.clear();
        cx.notify();
    }

    fn text(&self, cx: &Context<Self>) -> Option<String> {
        Some(mod_list::render(
            self.entries.as_deref()?,
            self.format,
            self.fields,
            self.template.read(cx).value().as_str(),
        ))
    }

    fn browse(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let path = PathBuf::from(self.destination.read(cx).value().as_str());
        let directory = path.parent().filter(|dir| !dir.as_os_str().is_empty());
        let name = path.file_name().and_then(|name| name.to_str());
        let answer =
            cx.prompt_for_new_path(directory.unwrap_or_else(|| std::path::Path::new(".")), name);
        cx.spawn_in(window, async move |this, cx| {
            let result = answer.await;
            let _ = this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(Ok(Some(path))) => {
                        this.destination.update(cx, |state, cx| {
                            state.set_value(path.to_string_lossy().into_owned(), window, cx);
                        });
                        this.status.clear();
                    }
                    Ok(Ok(None)) => return,
                    Ok(Err(_)) | Err(_) => {
                        this.status =
                            "Could not open the file picker. Enter a path instead.".into();
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let Some(text) = self.text(cx) else { return };
        let value = self.destination.read(cx).value();
        let path = value.as_str();
        if path.is_empty() {
            self.status = "Choose a destination path.".into();
        } else {
            self.status = match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
                .and_then(|mut file| file.write_all(text.as_bytes()))
            {
                Ok(()) => format!("Saved: {path}"),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    "That file already exists. Choose a different destination.".into()
                }
                Err(error) => format!("Could not save: {error}"),
            };
        }
        cx.notify();
    }
}

impl Render for ModListDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = crate::theme::Palette::get(cx);
        let formats = div()
            .flex()
            .flex_wrap()
            .gap_2()
            .children(Format::ALL.map(|format| {
                Button::new(format.label())
                    .label(format.label())
                    .when(self.format == format, |button| button.primary())
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.set_format(format, window, cx)),
                    )
            }));
        let options = if self.format == Format::Custom {
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child("Placeholders: {name}, {mod_id}, {version}, {authors}, {url}, {filename}")
                .child(Textarea::new(&self.template))
        } else {
            let fields: [(&str, fn(&mut Fields) -> &mut bool); 4] = [
                ("Version", |fields| &mut fields.version),
                ("Authors", |fields| &mut fields.authors),
                ("URL", |fields| &mut fields.url),
                ("File name", |fields| &mut fields.filename),
            ];
            div()
                .flex()
                .flex_wrap()
                .gap_2()
                .children(fields.map(|(label, field)| {
                    let mut values = self.fields;
                    let selected = *field(&mut values);
                    Button::new(label)
                        .label(if selected {
                            format!("✓ {label}")
                        } else {
                            label.to_owned()
                        })
                        .when(selected, |button| button.primary())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let value = field(&mut this.fields);
                            *value = !*value;
                            this.status.clear();
                            cx.notify();
                        }))
                }))
        };
        let text = self
            .text(cx)
            .unwrap_or_else(|| "Reading installed mods…".into());
        let ready = self.entries.is_some();
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .bg(palette.window)
            .text_color(palette.text)
            .child(
                div()
                    .font_weight(FontWeight::BOLD)
                    .child(format!("Export Mod List — {}", self.name)),
            )
            .child(formats)
            .child(options)
            .child(div().text_size(px(12.)).child("Preview"))
            .child(
                div()
                    .id("mod-list-preview")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .bg(palette.base)
                    .rounded_md()
                    .p_3()
                    .font_family("monospace")
                    .text_size(px(13.))
                    .child(text),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Input::new(&self.destination)),
                    )
                    .child(
                        Button::new("mod-list-browse")
                            .label("Browse…")
                            .on_click(cx.listener(|this, _, window, cx| this.browse(window, cx))),
                    ),
            )
            .child(
                div()
                    .flex()
                    .justify_between()
                    .items_center()
                    .gap_2()
                    .child(div().text_size(px(12.)).child(self.status.clone()))
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                Button::new("mod-list-copy")
                                    .label("Copy")
                                    .disabled(!ready)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if let Some(text) = this.text(cx) {
                                            cx.write_to_clipboard(ClipboardItem::new_string(text));
                                            this.status = "Copied to clipboard.".into();
                                            cx.notify();
                                        }
                                    })),
                            )
                            .child(
                                Button::new("mod-list-save")
                                    .label("Save")
                                    .primary()
                                    .disabled(!ready)
                                    .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                            )
                            .child(
                                Button::new("mod-list-close")
                                    .label("Close")
                                    .on_click(|_, window, _| window.remove_window()),
                            ),
                    ),
            )
    }
}
