#![recursion_limit = "1024"]
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

/// CCXT's generated exchange cores signal API errors with `panic!`; its typed
/// wrappers catch those panics and return `Result`. The default panic hook
/// still prints them, which makes ordinary auth/network failures look like
/// crashes. Suppress only panics that originate inside the ccxt crates.
fn install_ccxt_panic_filter() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let from_ccxt = info
            .location()
            .map(|location| location.file().contains("ccxt-"))
            .unwrap_or(false);
        if !from_ccxt {
            default_hook(info);
        }
    }));
}

fn main() {
    install_ccxt_panic_filter();

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
        std::env::remove_var("WAYLAND_DISPLAY");
    }

    #[cfg(target_os = "linux")]
    install_linux_desktop_entry();

    let app = Application::new()
        .with_assets(CombinedAssets::new())
        .with_http_client(ReqwestHttpClient::new());

    app.run(move |cx| {
        gpui_component::init(cx);

        // Register pane grid keyboard bindings

        cx.spawn(async move |cx| {
            let main_window = cx.open_window(
                WindowOptions {
                    #[cfg(target_os = "linux")]
                    titlebar: None,
                    #[cfg(not(target_os = "linux"))]
                    titlebar: Some(TitleBar::title_bar_options()),
                    window_decorations: Some(WindowDecorations::Client),
                    app_id: Some("finter".to_string()),
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

            // Observe actual removal, not just native close requests: the custom
            // title bar closes windows directly with Window::remove_window().
            cx.update(move |cx| {
                cx.on_window_closed(move |cx| {
                    let main_is_open = cx.windows().iter().any(|window| {
                        window.window_id() == main_window.window_id()
                    });
                    if !main_is_open {
                        cx.quit();
                    }
                })
                .detach();
            })?;

            Ok::<_, anyhow::Error>(())
        })
        .detach();
    });
}

#[cfg(target_os = "linux")]
fn install_linux_desktop_entry() {
    let Some(data_dir) = dirs::data_local_dir() else { return; };

    // 1. Install icons
    let icon_data = crate::assets::CustomAssets::get("app-icon.png")
        .or_else(|| crate::assets::CustomAssets::get("icons/app-icon.png"));

    if let Some(icon) = icon_data {
        // ~/.local/share/icons/hicolor/512x512/apps/finter.png
        let icon_hicolor = data_dir.join("icons/hicolor/512x512/apps/finter.png");
        if !icon_hicolor.exists() {
            if let Some(parent) = icon_hicolor.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&icon_hicolor, &icon.data);
        }

        // ~/.local/share/pixmaps/finter.png
        let pixmap = data_dir.join("pixmaps/finter.png");
        if !pixmap.exists() {
            if let Some(parent) = pixmap.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&pixmap, &icon.data);
        }
    }

    // 2. Install desktop entry ~/.local/share/applications/finter.desktop
    let desktop_path = data_dir.join("applications/finter.desktop");
    let current_exe = std::env::current_exe()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "finter".to_string());

    let desktop_content = format!(
        "[Desktop Entry]\n\
        Version=1.0\n\
        Type=Application\n\
        Name=FinTer\n\
        GenericName=Financial Terminal\n\
        Comment=Real-time crypto and financial market data terminal\n\
        Exec=\"{}\"\n\
        Icon=finter\n\
        Terminal=false\n\
        Categories=Finance;Office;Network;\n\
        StartupWMClass=finter\n\
        Keywords=crypto;trading;finance;terminal;bitcoin;\n",
        current_exe
    );

    if let Some(parent) = desktop_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&desktop_path, desktop_content);
}
