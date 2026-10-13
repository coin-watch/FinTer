#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

mod assets;
mod combined_assets;

use crate::combined_assets::CombinedAssets;
use finter::components::mainview::MainView;

use finter::http_client::ReqwestHttpClient;
use gpui::*;
use gpui_component::Root;

#[cfg(not(target_os = "linux"))]
use gpui_component::TitleBar;

fn main() {
    // Initialise tracing so bridge logs (info!, warn!, debug!) are visible.
    // Respects RUST_LOG env var; defaults to info level.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("finter=info")),
        )
        .init();

    #[cfg(target_os = "linux")]
    unsafe {
        std::env::remove_var("WAYLAND_DISPLAY")
    };

    let app = Application::new()
        .with_assets(CombinedAssets::new())
        .with_http_client(ReqwestHttpClient::new());

    app.run(move |cx| {
        gpui_component::init(cx);

        // Register pane grid keyboard bindings

        cx.spawn(async move |cx| {
            cx.open_window(
                WindowOptions {
                    #[cfg(target_os = "linux")]
                    titlebar: None,
                    #[cfg(not(target_os = "linux"))]
                    titlebar: Some(TitleBar::title_bar_options()),
                    window_decorations: Some(WindowDecorations::Client),
                    ..Default::default()
                },
                |window, cx| {
                    let view = cx.new(|cx| {
                        let mut mv = MainView::default();
                        mv.start_restore(cx);
                        mv
                    });
                    cx.new(|cx| Root::new(view, window, cx))
                },
            )?;

            Ok::<_, anyhow::Error>(())
        })
        .detach();
    });
}
