//! prism-rs: PrismLauncher rewritten in Rust with GPUI.

mod dialogs;
mod main_window;
mod state;

use gpui_kit::*;
use state::App;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(|cx| {
            gpui_kit::init(cx);

            let app = cx.new(|_| App::new());
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| main_window::MainWindow::new(app.clone(), window, cx))
            })
            .expect("failed to open main window");
        });
}
