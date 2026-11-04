//! Process-wide PNG icon used for Linux window icons.
//!
//! The binary crate owns the embedded assets, while secondary windows are
//! opened from the library crate. Storing the icon once here lets both paths
//! pass the same bytes to `WindowOptions::icon`.

use std::sync::OnceLock;

static APP_ICON: OnceLock<Vec<u8>> = OnceLock::new();

/// Store the PNG icon bytes. Subsequent calls are ignored.
pub fn set_png(bytes: Vec<u8>) {
    let _ = APP_ICON.set(bytes);
}

/// PNG icon bytes, if the application set them.
pub fn png() -> Option<&'static [u8]> {
    APP_ICON.get().map(Vec::as_slice)
}
