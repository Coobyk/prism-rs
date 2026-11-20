use gpui_kit::base::StyledExt as _;
use crate::theme::{Button, Palette};
use gpui_kit::component::link::Link;
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::WindowExt as _;
use gpui_kit::*;

const SOURCE: &str = "https://github.com/Coobyk/prism-rs";
const UPSTREAM_CREDITS: &str = "https://prismlauncher.org/about/";
const UPSTREAM_LICENSE: &str =
    "https://github.com/PrismLauncher/PrismLauncher/blob/develop/COPYING.md";
const LICENSE: &str = include_str!("../../../LICENSE");

pub(crate) fn open(window: &mut Window, cx: &mut App) {
    let content = cx.new(|_| AboutDialog { selected_tab: 0 });
    window.open_dialog(cx, move |dialog, _, cx| {
        dialog
            .bg(Palette::get(cx).window)
            .text_color(Palette::get(cx).text)
            .title("About prism-rs")
            .width(px(540.))
            .overlay_closable(false)
            .child(content.clone())
            .footer(
                div().flex().justify_end().child(
                    Button::new("close-about")
                        .label("Close")
                        .on_click(|_, window, cx| window.close_dialog(cx)),
                ),
            )
    });
}

struct AboutDialog {
    selected_tab: usize,
}

impl Render for AboutDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tab = self.selected_tab;
        let content = match tab {
            1 => credits(),
            2 => license(),
            _ => overview(cx),
        };
        let body_height = (window.viewport_size().height - px(200.)).clamp(px(80.), px(290.));

        div()
            .text_color(Palette::get(cx).text)
            .flex()
            .flex_col()
            .gap_3()
            .child(
                TabBar::new("about-tabs")
                    .selected_index(tab)
                    .children(["About", "Credits", "License"].map(|label| Tab::new().label(label)))
                    .on_click(cx.listener(|this, index: &usize, _, cx| {
                        this.selected_tab = *index;
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .h(body_height)
                    .pr_2()
                    .text_size(px(14.))
                    .child(content)
                    .overflow_y_scrollbar()
                    .id(("about-tab-content", tab)),
            )
    }
}

fn overview(cx: &App) -> Div {
    let platform = format!("{} ({})", std::env::consts::OS, std::env::consts::ARCH);
    let build = if cfg!(debug_assertions) {
        "Debug assertions enabled"
    } else {
        "Debug assertions disabled"
    };
    div()
        .flex()
        .flex_col()
        .gap_4()
        .child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(div().text_xl().font_semibold().child("prism-rs"))
                .child(div().text_color(Palette::get(cx).secondary_text).child(
                    "A Minecraft launcher inspired by Prism Launcher, built in Rust and GPUI.",
                )),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(detail("Version", env!("CARGO_PKG_VERSION")))
                .child(detail("Platform", platform))
                .child(detail("Build", build)),
        )
        .child(
            Link::new("prism-rs-source")
                .href(SOURCE)
                .child("prism-rs on GitHub"),
        )
}

fn detail(label: &'static str, value: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .gap_3()
        .child(
            div()
                .w(px(92.))
                .flex_shrink_0()
                .whitespace_nowrap()
                .font_semibold()
                .child(label),
        )
        .child(div().min_w_0().child(value.into()))
}

fn credits() -> Div {
    div()
        .flex()
        .flex_col()
        .gap_4()
        .child(
            section(
                "prism-rs",
                "Created by Coobyk and open-source contributors.",
            )
            .child(
                Link::new("prism-rs-contributors")
                    .href("https://github.com/Coobyk/prism-rs/graphs/contributors")
                    .child("prism-rs contributors"),
            ),
        )
        .child(
            section(
                "Prism Launcher",
                "Thanks to the team and community that inspired prism-rs.",
            )
            .child(
                Link::new("prism-contributors")
                    .href(UPSTREAM_CREDITS)
                    .child("Prism Launcher team and credits"),
            ),
        )
        .child(
            section(
                "MultiMC and PolyMC",
                "Thanks to the projects Prism Launcher builds on.",
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_3()
                    .child(
                        Link::new("multimc")
                            .href("https://multimc.org/")
                            .child("MultiMC"),
                    )
                    .child(
                        Link::new("polymc")
                            .href("https://polymc.org/")
                            .child("PolyMC"),
                    ),
            ),
        )
}

fn license() -> Div {
    let mut paragraphs = LICENSE.trim().split("\n\n");
    let title = paragraphs.next().unwrap_or("MIT License");
    div()
        .flex()
        .flex_col()
        .gap_3()
        .child(div().font_semibold().child(title))
        .children(paragraphs.map(|paragraph| div().child(paragraph.replace('\n', " "))))
        .child(section(
            "Upstream licensing",
            "Prism Launcher is licensed separately under GPL-3.0-only and carries additional notices for work from MultiMC and other projects.",
        ).child(Link::new("upstream-license").href(UPSTREAM_LICENSE).child("Prism Launcher license and notices")))
}

fn section(title: &'static str, description: &'static str) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(div().font_semibold().child(title))
        .child(div().child(description))
}
