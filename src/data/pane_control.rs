//! Agent-driven pane-grid control: the wire format for `ui` directives.
//!
//! The Agent module emits fenced `ui` blocks in its replies. Each block is one
//! [`UiAction`] addressed at a dashboard (by id, name, `active`, or `all`). The
//! component layer resolves the dashboard through the cross-window registry and
//! applies the action, returning a [`UiActionResult`] receipt that is persisted
//! with the message exactly like a trade receipt.
//!
//! This module is deliberately free of UI types so the data layer keeps its
//! dependency direction: `data` never imports `crate::components`.

use serde::{Deserialize, Serialize};

/// A single pane-grid command emitted by the Agent as a fenced `ui` block.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum UiAction {
    /// Replace a pane's module type, optionally merging a new config.
    SetModule {
        #[serde(default)]
        dashboard: Option<String>,
        pane: String,
        #[serde(alias = "type", alias = "module")]
        module_type: String,
        #[serde(default)]
        config: serde_json::Value,
    },
    /// Merge keys into a pane's existing config (exchange, symbol, interval…).
    UpdateConfig {
        #[serde(default)]
        dashboard: Option<String>,
        pane: String,
        #[serde(default)]
        config: serde_json::Value,
    },
    /// Split one pane in two along an axis (`horizontal` = stacked rows,
    /// `vertical` = side-by-side columns).
    Split {
        #[serde(default)]
        dashboard: Option<String>,
        pane: String,
        #[serde(default)]
        axis: Option<String>,
    },
    /// Replace a pane with an evenly distributed rows × cols matrix.
    SplitMatrix {
        #[serde(default)]
        dashboard: Option<String>,
        pane: String,
        rows: usize,
        cols: usize,
    },
    /// Remove a pane and collapse its parent split.
    Close {
        #[serde(default)]
        dashboard: Option<String>,
        pane: String,
    },
    /// Exchange the contents of two panes.
    Swap {
        #[serde(default)]
        dashboard: Option<String>,
        pane: String,
        #[serde(rename = "with", alias = "other", alias = "target")]
        with_pane: String,
    },
    /// Expand one pane over the whole dashboard.
    Maximize {
        #[serde(default)]
        dashboard: Option<String>,
        pane: String,
    },
    /// Leave the maximized view.
    Restore {
        #[serde(default)]
        dashboard: Option<String>,
    },
    /// Make a pane the active (focused) pane.
    Focus {
        #[serde(default)]
        dashboard: Option<String>,
        pane: String,
    },
    /// Move a split divider to `ratio` (0.05–0.95).
    Resize {
        #[serde(default)]
        dashboard: Option<String>,
        #[serde(alias = "pane", alias = "node")]
        split: String,
        ratio: f32,
    },
    /// Replace the entire dashboard layout with a complete pane tree.
    SetLayout {
        #[serde(default)]
        dashboard: Option<String>,
        layout: serde_json::Value,
    },
}

impl UiAction {
    /// Wire name of the action, used for receipts and error reporting.
    pub fn action_name(&self) -> &'static str {
        match self {
            UiAction::SetModule { .. } => "set_module",
            UiAction::UpdateConfig { .. } => "update_config",
            UiAction::Split { .. } => "split",
            UiAction::SplitMatrix { .. } => "split_matrix",
            UiAction::Close { .. } => "close",
            UiAction::Swap { .. } => "swap",
            UiAction::Maximize { .. } => "maximize",
            UiAction::Restore { .. } => "restore",
            UiAction::Focus { .. } => "focus",
            UiAction::Resize { .. } => "resize",
            UiAction::SetLayout { .. } => "set_layout",
        }
    }

    /// The dashboard selector carried by the action, if any.
    pub fn dashboard(&self) -> Option<&str> {
        match self {
            UiAction::SetModule { dashboard, .. }
            | UiAction::UpdateConfig { dashboard, .. }
            | UiAction::Split { dashboard, .. }
            | UiAction::SplitMatrix { dashboard, .. }
            | UiAction::Close { dashboard, .. }
            | UiAction::Swap { dashboard, .. }
            | UiAction::Maximize { dashboard, .. }
            | UiAction::Restore { dashboard }
            | UiAction::Focus { dashboard, .. }
            | UiAction::Resize { dashboard, .. }
            | UiAction::SetLayout { dashboard, .. } => dashboard.as_deref(),
        }
    }

    /// The pane or split this action targets, when it targets one.
    pub fn pane(&self) -> Option<&str> {
        match self {
            UiAction::SetModule { pane, .. }
            | UiAction::UpdateConfig { pane, .. }
            | UiAction::Split { pane, .. }
            | UiAction::SplitMatrix { pane, .. }
            | UiAction::Close { pane, .. }
            | UiAction::Swap { pane, .. }
            | UiAction::Maximize { pane, .. }
            | UiAction::Focus { pane, .. } => Some(pane),
            UiAction::Resize { split, .. } => Some(split),
            UiAction::Restore { .. } | UiAction::SetLayout { .. } => None,
        }
    }
}

/// The authoritative outcome of applying one [`UiAction`] to one dashboard.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UiActionResult {
    pub action: String,
    pub ok: bool,
    #[serde(default)]
    pub dashboard: Option<String>,
    #[serde(default)]
    pub pane: Option<String>,
    pub message: String,
}

impl UiActionResult {
    pub fn success(action: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            action: action.into(),
            ok: true,
            dashboard: None,
            pane: None,
            message: message.into(),
        }
    }

    pub fn failure(action: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            action: action.into(),
            ok: false,
            dashboard: None,
            pane: None,
            message: message.into(),
        }
    }

    pub fn with_dashboard(mut self, dashboard: impl Into<String>) -> Self {
        self.dashboard = Some(dashboard.into());
        self
    }

    pub fn with_pane(mut self, pane: impl Into<String>) -> Self {
        self.pane = Some(pane.into());
        self
    }
}

/// One parsed `ui` fence: either a usable action or the reason it was rejected.
#[derive(Clone, Debug, PartialEq)]
pub enum ParsedUiAction {
    Action(UiAction),
    Invalid(String),
}

/// Parses every complete fenced `ui` block in `text`, in order.
///
/// Unclosed fences are ignored, matching the trade/chart directive parser.
pub fn parse_ui_actions_in_text(text: &str) -> Vec<ParsedUiAction> {
    let mut parsed = Vec::new();
    let mut lines = text.split_inclusive('\n');
    while let Some(line) = lines.next() {
        if !is_ui_fence_opener(line) {
            continue;
        }
        let mut body = String::new();
        let mut closed = false;
        for inner in lines.by_ref() {
            if inner.trim_start().starts_with("```") {
                closed = true;
                break;
            }
            body.push_str(inner);
        }
        if !closed {
            continue;
        }
        match serde_json::from_str::<UiAction>(body.trim()) {
            Ok(action) => parsed.push(ParsedUiAction::Action(action)),
            Err(error) => parsed.push(ParsedUiAction::Invalid(format!(
                "malformed ui directive: {error}"
            ))),
        }
    }
    parsed
}

/// True when `line` opens a `ui` directive fence.
pub fn is_ui_fence_opener(line: &str) -> bool {
    let Some(rest) = line.trim_start().strip_prefix("```") else {
        return false;
    };
    let language = rest.trim().to_ascii_lowercase();
    let language = language.split_whitespace().next().unwrap_or("");
    matches!(language, "ui" | "finter-ui" | "pane" | "finter-pane")
}

/// Normalises an axis name from the model into a pane-tree axis.
pub fn parse_axis(raw: Option<&str>) -> Option<crate::data::pane_tree::Axis> {
    use crate::data::pane_tree::Axis;
    match raw?.trim().to_ascii_lowercase().as_str() {
        "horizontal" | "h" | "row" | "rows" | "top" | "bottom" | "stack" | "stacked" => {
            Some(Axis::Horizontal)
        }
        "vertical" | "v" | "column" | "columns" | "col" | "left" | "right" | "side" => {
            Some(Axis::Vertical)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_set_module_action() {
        let text = "Here you go.\n```ui\n{\"action\":\"set_module\",\"pane\":\"p1\",\"module_type\":\"chart\",\"config\":{\"symbol\":\"BTC/USDT\"}}\n```\nDone.";
        let parsed = parse_ui_actions_in_text(text);
        assert_eq!(parsed.len(), 1);
        match &parsed[0] {
            ParsedUiAction::Action(action) => {
                assert_eq!(action.action_name(), "set_module");
                assert_eq!(action.pane(), Some("p1"));
                assert_eq!(action.dashboard(), None);
            }
            other => panic!("expected an action, got {other:?}"),
        }
    }

    #[test]
    fn parses_multiple_actions_and_dashboard_selectors() {
        let text = "```ui\n{\"action\":\"split\",\"pane\":\"p1\",\"axis\":\"vertical\",\"dashboard\":\"Crypto\"}\n```\n```ui\n{\"action\":\"swap\",\"pane\":\"p1\",\"with\":\"p2\",\"dashboard\":\"all\"}\n```";
        let parsed = parse_ui_actions_in_text(text);
        assert_eq!(parsed.len(), 2);
        match &parsed[1] {
            ParsedUiAction::Action(action) => {
                assert_eq!(action.action_name(), "swap");
                assert_eq!(action.dashboard(), Some("all"));
                assert_eq!(action.pane(), Some("p1"));
                assert!(matches!(action, UiAction::Swap { with_pane, .. } if with_pane == "p2"));
            }
            other => panic!("expected an action, got {other:?}"),
        }
    }

    #[test]
    fn reports_malformed_blocks_instead_of_dropping_them() {
        let text = "```ui\n{\"action\":\"nope\"}\n```";
        let parsed = parse_ui_actions_in_text(text);
        assert_eq!(parsed.len(), 1);
        assert!(matches!(parsed[0], ParsedUiAction::Invalid(_)));
    }

    #[test]
    fn ignores_unclosed_fences() {
        let text = "```ui\n{\"action\":\"close\",\"pane\":\"p1\"}";
        assert!(parse_ui_actions_in_text(text).is_empty());
    }

    #[test]
    fn axis_names_normalise() {
        use crate::data::pane_tree::Axis;
        assert_eq!(parse_axis(Some("vertical")), Some(Axis::Vertical));
        assert_eq!(parse_axis(Some("rows")), Some(Axis::Horizontal));
        assert_eq!(parse_axis(Some("nonsense")), None);
        assert_eq!(parse_axis(None), None);
    }

    #[test]
    fn round_trips_through_json() {
        let action = UiAction::SetLayout {
            dashboard: Some("Crypto".into()),
            layout: serde_json::json!({"kind":"pane","id":"p1","data":{"type":"chart"}}),
        };
        let encoded = serde_json::to_string(&action).unwrap();
        let decoded: UiAction = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, action);
        assert_eq!(decoded.action_name(), "set_layout");
    }
}
