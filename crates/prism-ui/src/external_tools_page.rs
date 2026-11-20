use crate::theme::Button;
use gpui_kit::{
    component::input::{Input, InputState},
    *,
};
use prism_core::{settings::Settings, tools::Tool};

pub struct ExternalToolsPage {
    fields: Vec<Entity<InputState>>,
    results: Vec<String>,
}

impl ExternalToolsPage {
    pub fn new(settings: &Settings, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let fields = Tool::ALL
            .into_iter()
            .map(|tool| {
                cx.new(|cx| {
                    let mut input = InputState::new(window, cx).placeholder("Not configured");
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
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(div().text_size(px(12.)).text_color(palette.link).child(
                "External Tools · Apply to save paths. Profilers attach after Minecraft starts.",
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
                                    .label("Browse…")
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        let paths = cx.prompt_for_paths(PathPromptOptions {
                                            files: tool == Tool::JVisualVM,
                                            directories: tool != Tool::JVisualVM,
                                            multiple: false,
                                            prompt: Some(format!("Select {}", tool.name()).into()),
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
                            .child(Button::new(("check-tool", index)).label("Check").on_click(
                                cx.listener(move |this, _, _, cx| {
                                    this.results[index] = match tool.check(std::path::Path::new(
                                        this.fields[index].read(cx).value().trim(),
                                    )) {
                                        Ok(()) => format!("{} is ready to use.", tool.name()),
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
