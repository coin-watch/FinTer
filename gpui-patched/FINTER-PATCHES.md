# FinTer GPUI patch

This directory vendors the published `gpui` 0.2.2 crate (Apache-2.0).
The root Cargo.toml selects it through `[patch.crates-io]`.

## X11 position-only ConfigureNotify

`src/platform/linux/x11/window.rs`, `X11Window::set_bounds`:

The published implementation unconditionally queries the render extent,
updates the GPU drawable, and emits the resize callback (`if true`) on
ConfigureNotify, including when only the window origin changes during a
window-manager move. Only perform those operations when `is_resize` is true.
Position-only moves still update bounds, acknowledge the X11 sync counter,
and dispatch the moved callback. Genuine resizes retain the original drawable
update path. This avoids redundant drawable updates during title-bar dragging.

Manual regression checks on Linux/X11:

- Drag a window with multiple live modules: contents should not flash/blank.
- Resize by every edge: graphics must fill the new content area.
- Maximize and restore, including while live feeds are updating.
- Move between monitors with different scale factors.

When upgrading GPUI, check whether upstream fixes this condition before
carrying this patch forward. No files in the Cargo registry cache are modified.
