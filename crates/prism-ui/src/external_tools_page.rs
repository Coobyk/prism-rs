use crate::theme::Button;
use gpui_kit::{
    component::input::{Input, InputState},
    *,
};
use prism_core::{settings::Settings, tools::Tool};

pub struct ExternalToolsPage {
    app: Entity<crate::state::App>,
    fields: Vec<Entity<InputState>>,
    results: Vec<String>,
}

impl ExternalToolsPage {
    pub fn new(app: Entity<crate::state::App>, settings: &Settings, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let locale = app.read(cx).locale;
        let mut previous_locale = locale;
        cx.observe_in(&app, window, move |this, app, window, cx| {
            let locale = app.read(cx).locale;
            if locale == previous_locale {
                return;
            }
            previous_locale = locale;
            for field in &this.fields {
                field.update(cx, |input, cx| {
                    input.set_placeholder(locale.text("Not configured"), window, cx);
                });
            }
            this.results.iter_mut().for_each(String::clear);
            cx.notify();
        })
        .detach();
        let fields = Tool::ALL
            .into_iter()
            .map(|tool| {
                cx.new(|cx| {
                    let mut input = InputState::new(window, cx).placeholder(locale.text("Not configured"));
                    input.set_value(
                        tool.path(settings).to_string_lossy().into_owned(),
                        window,
                        cx,
                    );
                    input
                })
            })
            .collect();
        Self {
            app,
            fields,
            results: vec![String::new(); 3],
        }
    }
    pub fn save(&self, settings: &mut Settings, cx: &App) {
        for (tool, field) in Tool::ALL.into_iter().zip(&self.fields) {
            settings.set(tool.key(), field.read(cx).value().trim().to_owned());
        }
    }
}

impl Render for ExternalToolsPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = crate::theme::Palette::get(cx);
        let locale = self.app.read(cx).locale;
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(div().text_size(px(12.)).text_color(palette.link).child(
                locale.text("External Tools · Apply to save paths. Profilers attach after Minecraft starts."),
            ))
            .children(Tool::ALL.into_iter().enumerate().map(|(index, tool)| {
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(div().text_size(px(13.)).child(tool.name()))
                    .child(Input::new(&self.fields[index]))
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                Button::new(("browse-tool", index))
                                    .label(locale.text("Browse…"))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        let paths = cx.prompt_for_paths(PathPromptOptions {
                                            files: tool == Tool::JVisualVM,
                                            directories: tool != Tool::JVisualVM,
                                            multiple: false,
                                            prompt: Some(this.app.read(cx).locale.format("Select {0}", &[tool.name()]).into()),
                                        });
                                        cx.spawn(async move |this, cx| {
                                            if let Ok(Ok(Some(paths))) = paths.await {
                                                if let Some(path) = paths.first() {
                                                    let value = path.to_string_lossy().into_owned();
                                                    let _ =
                                                        this.update_in(cx, |this, window, cx| {
                                                            this.fields[index].update(
                                                                cx,
                                                                |input, cx| {
                                                                    input.set_value(
                                                                        value, window, cx,
                                                                    )
                                                                },
                                                            );
                                                            this.results[index].clear();
                                                            cx.notify();
                                                        });
                                                }
                                            }
                                        })
                                        .detach();
                                    })),
                            )
                            .child(Button::new(("check-tool", index)).label(locale.text("Check")).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    this.results[index] = match tool.check(std::path::Path::new(
                                        this.fields[index].read(cx).value().trim(),
                                    )) {
                                        Ok(()) => this.app.read(cx).locale.format("{0} is ready to use.", &[tool.name()]),
                                        Err(error) => error.into(),
                                    };
                                    cx.notify();
                                }),
                            ))
                            .child(div().text_size(px(12.)).child(self.results[index].clone())),
                    )
            }))
    }
}
