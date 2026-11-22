//! Agent-driven terminal control: the wire format for `terminal` directives.
//!
//! Pane-grid actions live in [`crate::data::pane_control`] and only rearrange
//! panes. A `terminal` directive is addressed at the application as a whole:
//! opening and closing layouts, creating or deleting them, broadcasting a market
//! to every pane, and changing terminal settings.
//!
//! Like the pane format, this module is deliberately free of UI types so the
//! data layer never imports `crate::components`.

use serde::{Deserialize, Serialize};

/// A single workspace command emitted by the Agent as a fenced `terminal`
/// block.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum TerminalAction {
    /// Open (or focus) a saved dashboard. Omit `dashboard` for the active one.
    OpenDashboard {
        #[serde(default)]
        dashboard: Option<String>,
    },
    /// Close a dashboard tab without deleting the layout file.
    CloseTab {
        #[serde(default)]
        dashboard: Option<String>,
    },
    /// Create a new layout. `parent` is a workspace-relative folder, empty for
    /// the workspace root.
    CreateDashboard {
        name: String,
        #[serde(default)]
        parent: Option<String>,
        #[serde(default)]
        open: Option<bool>,
    },
    /// Rename a layout file or folder.
    RenameDashboard {
        dashboard: String,
        name: String,
    },
    /// Duplicate a layout next to the original.
    DuplicateDashboard {
        dashboard: String,
    },
    /// Delete a layout file or folder. Requires `confirm: true`.
    DeleteDashboard {
        dashboard: String,
        #[serde(default)]
        confirm: bool,
    },
    /// Push one market configuration to every matching pane.
    ///
    /// `scope` is `all` (default), `dashboard`, `charts`, `tickers`,
    /// `orderbooks`, or `trades`.
    SetMarket {
        #[serde(default)]
        exchange: Option<String>,
        #[serde(default)]
        symbol: Option<String>,
        #[serde(default)]
        timeframe: Option<String>,
        #[serde(default)]
        scope: Option<String>,
        #[serde(default)]
        dashboard: Option<String>,
    },
    /// Set the terminal timezone (for example `Europe/Zurich`).
    SetTimezone {
        timezone: String,
    },
    /// Switch the visual theme. `dark` defaults to the current mode.
    SetTheme {
        theme: String,
        #[serde(default)]
        dark: Option<bool>,
    },
    /// Enable or mute terminal alert sounds.
    SetAlertSound {
        enabled: bool,
    },
}

impl TerminalAction {
    /// Wire name of the action, used for receipts and error reporting.
    pub fn action_name(&self) -> &'static str {
        match self {
            TerminalAction::OpenDashboard { .. } => "open_dashboard",
            TerminalAction::CloseTab { .. } => "close_tab",
            TerminalAction::CreateDashboard { .. } => "create_dashboard",
            TerminalAction::RenameDashboard { .. } => "rename_dashboard",
            TerminalAction::DuplicateDashboard { .. } => "duplicate_dashboard",
            TerminalAction::DeleteDashboard { .. } => "delete_dashboard",
            TerminalAction::SetMarket { .. } => "set_market",
            TerminalAction::SetTimezone { .. } => "set_timezone",
            TerminalAction::SetTheme { .. } => "set_theme",
            TerminalAction::SetAlertSound { .. } => "set_alert_sound",
        }
    }

    /// The dashboard selector carried by the action, if any.
    pub fn dashboard(&self) -> Option<&str> {
        match self {
            TerminalAction::OpenDashboard { dashboard }
            | TerminalAction::CloseTab { dashboard }
            | TerminalAction::SetMarket { dashboard, .. } => dashboard.as_deref(),
            TerminalAction::RenameDashboard { dashboard, .. }
            | TerminalAction::DuplicateDashboard { dashboard }
            | TerminalAction::DeleteDashboard { dashboard, .. } => Some(dashboard),
            TerminalAction::CreateDashboard { .. }
            | TerminalAction::SetTimezone { .. }
            | TerminalAction::SetTheme { .. }
            | TerminalAction::SetAlertSound { .. } => None,
        }
    }

    /// True when the action destroys data and therefore requires confirmation.
    pub fn is_destructive(&self) -> bool {
        matches!(self, TerminalAction::DeleteDashboard { .. })
    }
}

/// One parsed `terminal` fence: either a usable action or the reason it was
/// rejected.
#[derive(Clone, Debug, PartialEq)]
pub enum ParsedTerminalAction {
    Action(TerminalAction),
    Invalid(String),
}

/// Parses every complete fenced `terminal` block in `text`, in order.
///
/// Unclosed fences are ignored, matching the trade/chart/pane parsers.
pub fn parse_terminal_actions_in_text(text: &str) -> Vec<ParsedTerminalAction> {
    let mut parsed = Vec::new();
    let mut lines = text.split_inclusive('\n');
    while let Some(line) = lines.next() {
        if !is_terminal_fence_opener(line) {
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
        match serde_json::from_str::<TerminalAction>(body.trim()) {
            Ok(action) => parsed.push(ParsedTerminalAction::Action(action)),
            Err(error) => parsed.push(ParsedTerminalAction::Invalid(format!(
                "malformed terminal directive: {error}"
            ))),
        }
    }
    parsed
}

/// True when `line` opens a `terminal` directive fence.
pub fn is_terminal_fence_opener(line: &str) -> bool {
    let Some(rest) = line.trim_start().strip_prefix("```") else {
        return false;
    };
    let language = rest.trim().to_ascii_lowercase();
    let language = language.split_whitespace().next().unwrap_or("");
    matches!(language, "terminal" | "finter-terminal")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_set_market_action() {
        let text = "```terminal\n{\"action\":\"set_market\",\"symbol\":\"ETH/USDT\",\"timeframe\":\"15m\",\"scope\":\"all\"}\n```";
        let parsed = parse_terminal_actions_in_text(text);
        assert_eq!(parsed.len(), 1);
        match &parsed[0] {
            ParsedTerminalAction::Action(action) => {
                assert_eq!(action.action_name(), "set_market");
                assert_eq!(action.dashboard(), None);
                assert!(!action.is_destructive());
            }
            other => panic!("expected an action, got {other:?}"),
        }
    }

    #[test]
    fn parses_dashboard_actions_and_destructiveness() {
        let text = r#"```terminal
{"action":"delete_dashboard","dashboard":"Macro","confirm":true}
```
```terminal
{"action":"open_dashboard","dashboard":"Crypto"}
```"#;
        let parsed = parse_terminal_actions_in_text(text);
        assert_eq!(parsed.len(), 2);
        match &parsed[0] {
            ParsedTerminalAction::Action(action) => {
                assert_eq!(action.action_name(), "delete_dashboard");
                assert!(action.is_destructive());
                assert_eq!(action.dashboard(), Some("Macro"));
            }
            other => panic!("expected an action, got {other:?}"),
        }
    }

    #[test]
    fn reports_malformed_blocks_instead_of_dropping_them() {
        let text = "```terminal\n{\"action\":\"nope\"}\n```";
        let parsed = parse_terminal_actions_in_text(text);
        assert_eq!(parsed.len(), 1);
        assert!(matches!(parsed[0], ParsedTerminalAction::Invalid(_)));
    }

    #[test]
    fn ignores_unclosed_fences() {
        let text = "```terminal\n{\"action\":\"open_dashboard\"}";
        assert!(parse_terminal_actions_in_text(text).is_empty());
    }

    #[test]
    fn round_trips_through_json() {
        let action = TerminalAction::CreateDashboard {
            name: "Macro".into(),
            parent: Some("Research".into()),
            open: Some(true),
        };
        let encoded = serde_json::to_string(&action).unwrap();
        let decoded: TerminalAction = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, action);
        assert_eq!(decoded.action_name(), "create_dashboard");
    }
}
