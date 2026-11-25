//! Registry and openers for chart feature tool windows.
//!
//! Chart features that used to live in the 34 px in-chart toolbar now open as
//! transient floating OS windows. This module owns the per-parent-window
//! registry so repeated clicks focus the existing window instead of stacking
//! duplicates.

mod color_window;
mod indicator_window;

use std::collections::HashMap;
use std::time::{Duration, Instant};

use gpui::{
    AnyWindowHandle, App, AppContext, BorrowAppContext, Entity, Global, Window, WindowId, px,
};
use gpui_component::PixelsExt;

use crate::components::ui::pane_grid::renderer::PaneGridRenderer;

pub use color_window::ColorWindow;
pub use indicator_window::IndicatorWindow;

/// Which floating chart tool to open.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ChartToolKind {
    Indicators,
    Color,
}

impl ChartToolKind {
    fn title(self) -> &'static str {
        match self {
            Self::Indicators => "Indicators",
            Self::Color => "Drawing color",
        }
    }

    fn default_size(self) -> (f32, f32) {
        match self {
            Self::Indicators => (440.0, 600.0),
            Self::Color => (240.0, 340.0),
        }
    }

    fn min_size(self) -> Option<(f32, f32)> {
        match self {
            Self::Indicators => Some((360.0, 400.0)),
            Self::Color => None,
        }
    }

    fn resizable(self) -> bool {
        matches!(self, Self::Indicators)
    }
}

/// Binding from a floating tool window back to the chart pane that opened it.
#[derive(Clone)]
pub struct ChartPaneBinding {
    pub grid: Entity<PaneGridRenderer>,
    pub pane_id: String,
}

type ToolWindowKey = (WindowId, String, ChartToolKind);

fn tool_window_key(parent: WindowId, pane_id: &str, kind: ChartToolKind) -> ToolWindowKey {
    (parent, pane_id.to_string(), kind)
}

#[derive(Default)]
struct ChartToolRegistry {
    handles: HashMap<ToolWindowKey, AnyWindowHandle>,
    /// Reserved while the async `open_window` call is in flight. Without this,
    /// two clicks before the handle is registered would open duplicate windows.
    opening: HashMap<ToolWindowKey, Instant>,
}

impl ChartToolRegistry {
    fn reserve_open(&mut self, key: ToolWindowKey) -> bool {
        self.opening
            .retain(|_, started| started.elapsed() < Duration::from_secs(15));
        if self.opening.contains_key(&key) {
            return false;
        }
        self.opening.insert(key, Instant::now());
        true
    }

    fn finish_open(&mut self, key: ToolWindowKey, handle: AnyWindowHandle) {
        self.opening.remove(&key);
        self.handles.insert(key, handle);
    }

    fn cancel_open(&mut self, key: &ToolWindowKey) {
        self.opening.remove(key);
    }
}

impl Global for ChartToolRegistry {}

/// Open or focus the floating tool window for a chart pane.
pub fn open_chart_tool_window(
    kind: ChartToolKind,
    binding: ChartPaneBinding,
    window: &mut Window,
    cx: &mut App,
) {
    let parent_window = window.window_handle().window_id();
    let key = tool_window_key(parent_window, &binding.pane_id, kind);

    if !cx.has_global::<ChartToolRegistry>() {
        cx.set_global(ChartToolRegistry::default());
    }

    prune_stale_handles(cx);

    let existing =
        cx.update_global::<ChartToolRegistry, _>(|registry, _| registry.handles.get(&key).copied());
    if let Some(handle) = existing {
        if handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
        {
            cx.update_global::<ChartToolRegistry, _>(|registry, _| {
                registry.opening.remove(&key);
            });
            return;
        }
        cx.update_global::<ChartToolRegistry, _>(|registry, _| {
            registry.handles.remove(&key);
            registry.opening.remove(&key);
        });
    }

    let reserved =
        cx.update_global::<ChartToolRegistry, _>(|registry, _| registry.reserve_open(key.clone()));
    if !reserved {
        // Another opener is already in flight for this exact pane/tool pair.
        return;
    }

    let bounds = centered_bounds(window.bounds(), kind.default_size());
    let min_size = kind.min_size();
    let icon = crate::data::app_icon::png().map(|bytes| bytes.to_vec());
    let options = gpui::WindowOptions {
        window_bounds: Some(gpui::WindowBounds::Windowed(bounds)),
        #[cfg(target_os = "linux")]
        titlebar: None,
        #[cfg(not(target_os = "linux"))]
        titlebar: Some(gpui_component::TitleBar::title_bar_options()),
        focus: true,
        show: true,
        kind: gpui::WindowKind::Floating,
        is_minimizable: false,
        is_resizable: kind.resizable(),
        window_decorations: Some(gpui::WindowDecorations::Client),
        app_id: Some("ch.coinwat.finter".to_string()),
        icon,
        window_min_size: min_size.map(|(width, height)| gpui::size(px(width), px(height))),
        ..Default::default()
    };

    let bound_pane_id = binding.pane_id.clone();
    let open_key = key.clone();
    cx.spawn(async move |cx| {
        let result = cx.open_window(options, move |window, cx| match kind {
            ChartToolKind::Indicators => {
                let view = cx.new(|cx| IndicatorWindow::new(binding, window, cx));
                cx.new(|cx| gpui_component::Root::new(view, window, cx))
            }
            ChartToolKind::Color => {
                let view = cx.new(|cx| ColorWindow::new(binding, window, cx));
                cx.new(|cx| gpui_component::Root::new(view, window, cx))
            }
        });

        match result {
            Ok(window_handle) => {
                let handle: AnyWindowHandle = window_handle.into();
                let _ = cx.update(|cx| {
                    cx.update_global::<ChartToolRegistry, _>(|registry, _| {
                        registry.finish_open(open_key, handle);
                    });
                });
            }
            Err(error) => {
                let _ = cx.update(|cx| {
                    cx.update_global::<ChartToolRegistry, _>(|registry, _| {
                        registry.cancel_open(&open_key);
                    });
                });
                tracing::error!(
                    pane_id = %bound_pane_id,
                    kind = kind.title(),
                    "failed to open chart tool window: {error}"
                );
            }
        }
    })
    .detach();
}

fn prune_stale_handles(cx: &mut App) {
    let keys: Vec<_> = cx.update_global::<ChartToolRegistry, _>(|registry, _| {
        registry.handles.keys().cloned().collect()
    });
    for key in keys {
        let handle = cx.update_global::<ChartToolRegistry, _>(|registry, _| {
            registry.handles.get(&key).copied()
        });
        let Some(handle) = handle else {
            continue;
        };
        if handle.update(cx, |_, _, _| ()).is_err() {
            cx.update_global::<ChartToolRegistry, _>(|registry, _| {
                registry.handles.remove(&key);
            });
        }
    }
}

fn centered_bounds(
    parent: gpui::Bounds<gpui::Pixels>,
    (width, height): (f32, f32),
) -> gpui::Bounds<gpui::Pixels> {
    let parent_x = parent.origin.x.as_f32();
    let parent_y = parent.origin.y.as_f32();
    let parent_w = parent.size.width.as_f32();
    let parent_h = parent.size.height.as_f32();

    let min_offset = 24.0;
    let min_x = parent_x + min_offset;
    let max_x = (parent_x + parent_w - width - min_offset).max(min_x);
    let min_y = parent_y + min_offset;
    let max_y = (parent_y + parent_h - height - min_offset).max(min_y);

    let x = (parent_x + (parent_w - width) * 0.5).clamp(min_x, max_x);
    let y = (parent_y + (parent_h - height) * 0.5).clamp(min_y, max_y);

    gpui::Bounds {
        origin: gpui::point(px(x), px(y)),
        size: gpui::size(px(width), px(height)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_windows_are_centered_over_the_parent() {
        let parent = gpui::Bounds {
            origin: gpui::point(px(10.0), px(20.0)),
            size: gpui::size(px(1000.0), px(800.0)),
        };
        let bounds = centered_bounds(parent, (240.0, 340.0));
        assert_eq!(bounds.origin.x, px(390.0));
        assert_eq!(bounds.origin.y, px(250.0));

        let cramped = gpui::Bounds {
            origin: gpui::point(px(0.0), px(0.0)),
            size: gpui::size(px(100.0), px(50.0)),
        };
        let bounds = centered_bounds(cramped, (240.0, 340.0));
        assert_eq!(bounds.origin.x, px(24.0));
        assert_eq!(bounds.origin.y, px(24.0));
    }

    #[test]
    fn tool_window_sizes_match_the_design() {
        assert_eq!(ChartToolKind::Indicators.default_size(), (440.0, 600.0));
        assert_eq!(ChartToolKind::Indicators.min_size(), Some((360.0, 400.0)));
        assert!(ChartToolKind::Indicators.resizable());
        assert_eq!(ChartToolKind::Color.default_size(), (240.0, 340.0));
        assert_eq!(ChartToolKind::Color.min_size(), None);
        assert!(!ChartToolKind::Color.resizable());
    }

    #[test]
    fn duplicate_opening_is_reserved_once_per_pane_and_tool() {
        let mut registry = ChartToolRegistry::default();
        let key = tool_window_key(WindowId::from(7), "pane-a", ChartToolKind::Indicators);
        assert!(registry.reserve_open(key.clone()));
        let first_reservation = *registry
            .opening
            .get(&key)
            .expect("first click reserves the opener");
        assert!(!registry.reserve_open(key.clone()));
        assert_eq!(registry.opening.get(&key), Some(&first_reservation));
        assert!(!registry.reserve_open(key.clone()));
        assert_eq!(registry.opening.get(&key), Some(&first_reservation));
        registry.cancel_open(&key);
        assert!(registry.reserve_open(key.clone()));

        let other_pane = tool_window_key(WindowId::from(7), "pane-b", ChartToolKind::Indicators);
        let other_tool = tool_window_key(WindowId::from(7), "pane-a", ChartToolKind::Color);
        assert!(registry.reserve_open(other_pane));
        assert!(registry.reserve_open(other_tool));
    }
}
