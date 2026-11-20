use crate::about_dialog;
use crate::theme::Icon;
use gpui_kit::component::button::ButtonVariant;
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::component::WindowExt as _;
use gpui_kit::*;
use prism_core::Paths;

pub(crate) fn button(paths: Paths, cx: &App) -> impl IntoElement {
    crate::theme::button("help", Icon::Help, "Help", cx)
        .dropdown_caret(true)
        .dropdown_menu(move |menu, _, _| {
            let paths = paths.clone();
            menu.item(
                PopupMenuItem::new("Clear Metadata Cache…")
                    .on_click(move |_, window, cx| confirm_clear_cache(paths.clone(), window, cx)),
            )
            .item(link(
                "Report a Bug",
                "https://github.com/Coobyk/prism-rs/issues",
            ))
            .separator()
            .item(link(
                "Prism Launcher Matrix",
                "https://prismlauncher.org/matrix",
            ))
            .item(link(
                "Prism Launcher Discord",
                "https://prismlauncher.org/discord",
            ))
            .item(link(
                "Prism Launcher Reddit",
                "https://prismlauncher.org/reddit",
            ))
            .separator()
            .item(link(
                "Prism Launcher News",
                "https://prismlauncher.org/news/",
            ))
            .item(link(
                "prism-rs Changelog",
                "https://github.com/Coobyk/prism-rs/commits/main/",
            ))
            .item(link(
                "prism-rs Help",
                "https://github.com/Coobyk/prism-rs#readme",
            ))
            .item(link(
                "Prism Launcher Wiki",
                "https://prismlauncher.org/wiki/",
            ))
            .separator()
            .item(
                PopupMenuItem::new("About prism-rs").on_click(|_, window, cx| {
                    window.defer(cx, about_dialog::open);
                }),
            )
        })
}

fn link(label: &'static str, url: &'static str) -> PopupMenuItem {
    PopupMenuItem::new(label).on_click(move |_, _, cx| cx.open_url(url))
}

fn confirm_clear_cache(paths: Paths, window: &mut Window, cx: &mut App) {
    window.defer(cx, move |window, cx| {
        window.open_alert_dialog(cx, move |alert, _, _| {
            let paths = paths.clone();
            alert
                .confirm()
                .title("Clear the metadata cache?")
                .description("Version metadata will be downloaded again as needed. Instances, game files, and settings stay intact.")
                .ok_text("Clear Cache")
                .ok_variant(ButtonVariant::Danger)
                .on_ok(move |_, window, cx| {
                    clear_cache(paths.clone(), window, cx);
                    true
                })
        });
    });
}

fn clear_cache(paths: Paths, window: &mut Window, cx: &mut App) {
    window
        .spawn(cx, async move |cx| {
            let result = cx
                .background_spawn(async move { paths.clear_metadata_cache() })
                .await;
            let _ = cx.update(|window, cx| {
                window.open_alert_dialog(cx, move |alert, _, _| match &result {
                    Ok(()) => alert.title("Metadata cache cleared"),
                    Err(error) => alert
                        .title("Could not clear the metadata cache")
                        .description(error.to_string()),
                });
            });
        })
        .detach();
}
