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

## Linux restart and AppImage paths

`src/platform/linux/platform.rs`, `Platform::restart` and `Platform::app_path`:

- The restart script now receives the pid and executable path as positional
  arguments (`$0`/`$1`) instead of interpolating them into the shell script.
  Paths containing spaces or shell metacharacters therefore restart correctly.
- `app_path()` returns `$APPIMAGE` when the process is running from an
  AppImage. Without this, `current_exe()` points at the temporary mount
  (`/tmp/.mount_*`), which no longer exists after the process exits.

Manual regression checks on Linux:

- Install an update, click "Restart Now", and confirm the new version launches.
- Run the AppImage, restart the app, and confirm it relaunches from the
  AppImage file rather than the transient mount directory.

## Windows application manifest

`resources/windows/gpui.manifest.xml`:

The upstream manifest only declares DPI awareness and the Common-Controls
dependency. FinTer extends it with the declarations that Windows security and
compatibility tooling expect from a desktop application:

- `requestedExecutionLevel level="asInvoker"` so the process never asks for
  administrator rights.
- `supportedOS` for Windows 10/11 so compatibility shims and SmartScreen use
  the correct behavior.
- `longPathAware` and `activeCodePage` (UTF-8) for correct path and text
  handling.
- A stable `assemblyIdentity` for the application.

This manifest is embedded by GPUI's `windows-manifest` build feature. The
application's own `build.rs` only adds the icon and `VERSIONINFO` metadata, so
there is exactly one `RT_MANIFEST` resource in the final executable.

Manual regression checks on Windows:

- Launch the executable and confirm no UAC prompt appears.
- Confirm the window renders correctly on a high-DPI display.
- Right-click the executable, open Properties, and confirm the Details tab
  shows FinTer, CoinWatch, and the Cargo package version.
