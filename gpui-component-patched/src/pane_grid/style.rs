use gpui::{px, Hsla, Pixels};
use serde::{Deserialize, Serialize};

/// Style for the pane grid.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaneGridStyle {
    /// Style for dividers (split handles).
    pub divider: DividerStyle,
    /// Style for the active pane highlight.
    pub active_pane: ActivePaneStyle,
    /// Style for drop target highlights.
    pub drop_target: DropTargetStyle,
}

impl Default for PaneGridStyle {
    fn default() -> Self {
        Self {
            divider: DividerStyle::default(),
            active_pane: ActivePaneStyle::default(),
            drop_target: DropTargetStyle::default(),
        }
    }
}

/// Style for dividers between panes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DividerStyle {
    /// Color of the divider.
    pub color: Hsla,
    /// Width/height of the divider.
    pub thickness: Pixels,
    /// Color when hovering over the divider.
    pub hover_color: Option<Hsla>,
}

impl Default for DividerStyle {
    fn default() -> Self {
        Self {
            color: gpui::rgb(0x555555).into(),
            thickness: px(4.0),
            hover_color: Some(gpui::rgb(0x888888).into()),
        }
    }
}

/// Style for the active pane.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivePaneStyle {
    /// Border color for the active pane.
    pub border_color: Hsla,
    /// Border width for the active pane.
    pub border_width: Pixels,
}

impl Default for ActivePaneStyle {
    fn default() -> Self {
        Self {
            border_color: gpui::rgb(0x0078d4).into(),
            border_width: px(2.0),
        }
    }
}

/// Style for drop target highlights.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DropTargetStyle {
    /// Color of the drop target highlight.
    pub color: Hsla,
    /// Border width of the drop target.
    pub border_width: Pixels,
}

impl Default for DropTargetStyle {
    fn default() -> Self {
        Self {
            color: gpui::rgb(0x0078d4).into(),
            border_width: px(2.0),
        }
    }
}
