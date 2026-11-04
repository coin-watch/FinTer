#![recursion_limit = "1024"]
#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

// Tie the binary's fingerprint to the baked update endpoint so a URL change
// relinks the executable even when the library comes from a restored cache.
include!(concat!(env!("OUT_DIR"), "/version_url.rs"));
const _: &str = DEFAULT_VERSION_URL;

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
    // CI uses this to verify the endpoint baked into the release binary.
    if std::env::args().any(|arg| arg == "--print-update-url") {
        println!("{}", finter::data::version::version_check_url());
        return;
    }

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
    use std::path::Path;

    let Some(data_dir) = dirs::data_local_dir() else { return; };

    // Write only when the content actually changed so we don't churn the
    // desktop caches on every launch.
    fn write_if_changed(path: &Path, data: &[u8]) -> bool {
        if std::fs::read(path).ok().as_deref() == Some(data) {
            return false;
        }
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        std::fs::write(path, data).is_ok()
    }

    let mut changed = false;

    // 1. Install icons.
    //
    // The source asset is 1024x1024. Older builds copied it into the
    // hicolor/512x512 directory, which makes some icon themes reject it
    // because the directory size does not match the actual image size.
    let icon_png = crate::assets::CustomAssets::get("app-icon.png")
        .or_else(|| crate::assets::CustomAssets::get("icons/app-icon.png"));
    if let Some(icon) = icon_png.as_ref() {
        changed |= write_if_changed(
            &data_dir.join("icons/hicolor/1024x1024/apps/finter.png"),
            &icon.data,
        );
        changed |= write_if_changed(&data_dir.join("pixmaps/finter.png"), &icon.data);

        let stale = data_dir.join("icons/hicolor/512x512/apps/finter.png");
        if stale.exists() {
            changed |= std::fs::remove_file(&stale).is_ok();
        }
    }

    // Some icon themes do not read scalable SVGs and cap their lookup at
    // 256px, so install a correctly-sized 256px PNG as well.
    if let Some(icon) = crate::assets::CustomAssets::get("app-icon-256.png") {
        changed |= write_if_changed(
            &data_dir.join("icons/hicolor/256x256/apps/finter.png"),
            &icon.data,
        );
    }

    // A scalable icon lets the desktop render the correct size for docks,
    // application menus, and file managers that support SVG.
    if let Some(icon) = crate::assets::CustomAssets::get("icons/app-icon.svg") {
        changed |= write_if_changed(
            &data_dir.join("icons/hicolor/scalable/apps/finter.svg"),
            &icon.data,
        );
    }

    // 2. Install the desktop entry.
    //
    // When running from an AppImage, `current_exe()` points inside the
    // temporary mount (`/tmp/.mount_*`), which disappears when the app exits.
    // Point the launcher at the AppImage file instead.
    let launch_path = std::env::var_os("APPIMAGE")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::current_exe().ok());
    let exec_path = launch_path
        .as_ref()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|| "finter".to_string());

    // Quote/escape the path for the desktop entry Exec key.
    let exec = {
        let mut escaped = String::with_capacity(exec_path.len() + 2);
        escaped.push('"');
        for ch in exec_path.chars() {
            match ch {
                '\\' => escaped.push_str("\\\\"),
                '"' => escaped.push_str("\\\""),
                '`' => escaped.push_str("\\`"),
                '$' => escaped.push_str("\\$"),
                '%' => escaped.push_str("%%"),
                _ => escaped.push(ch),
            }
        }
        escaped.push('"');
        escaped
    };

    let desktop_content = format!(
        "[Desktop Entry]\n\
        Version=1.0\n\
        Type=Application\n\
        Name=FinTer\n\
        GenericName=Financial Terminal\n\
        Comment=Real-time crypto and financial market data terminal\n\
        Exec={exec} %u\n\
        Icon=finter\n\
        Terminal=false\n\
        Categories=Finance;Office;Network;\n\
        StartupWMClass=finter\n\
        StartupNotify=true\n\
        Keywords=crypto;trading;finance;terminal;bitcoin;\n"
    );

    changed |= write_if_changed(
        &data_dir.join("applications/finter.desktop"),
        desktop_content.as_bytes(),
    );

    // 3. Give the downloaded AppImage/binary itself an icon in file managers
    // that honour GVFS metadata (e.g. GNOME Files). Best-effort: `gio` may not
    // be installed on non-GNOME desktops.
    if let Some(artifact) = launch_path.as_ref() {
        let icon_path = data_dir.join("icons/hicolor/256x256/apps/finter.png");
        if icon_path.exists() {
            if let Ok(icon_uri) = url::Url::from_file_path(&icon_path) {
                let _ = std::process::Command::new("gio")
                    .arg("set")
                    .arg(artifact)
                    .arg("metadata::custom-icon")
                    .arg(icon_uri.as_str())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status();
            }
        }
    }

    // 4. Refresh the caches so the new icon/launcher is visible immediately.
    if changed {
        let _ = std::process::Command::new("gtk-update-icon-cache")
            .arg("-f")
            .arg("-t")
            .arg(data_dir.join("icons/hicolor"))
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        let _ = std::process::Command::new("update-desktop-database")
            .arg(data_dir.join("applications"))
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}
