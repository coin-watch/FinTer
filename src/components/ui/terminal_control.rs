//! Applies Agent `terminal` directives to the workspace.
//!
//! The pane format ([`crate::data::pane_control`]) is scoped to a dashboard's
//! pane grid. This executor is scoped to the application: it reaches the
//! `MainView` through [`GlobalMainView`] so a conversation in any window can
//! open, create, rename, duplicate, or close layouts, broadcast a market, and
//! change terminal settings.

use gpui::App;

use crate::components::mainview::GlobalMainView;
use crate::data::pane_control::UiActionResult;
use crate::data::terminal_control::{ParsedTerminalAction, parse_terminal_actions_in_text};

/// Parses and applies every `terminal` directive in an assistant reply.
///
/// `caller_dashboard` is the dashboard hosting the conversation, used to refuse
/// commands that would close or delete the surface the Agent is talking from.
pub fn execute_terminal_actions_in_reply(
    text: &str,
    caller_dashboard: Option<&str>,
    caller_pane: Option<&str>,
    cx: &mut App,
) -> Vec<UiActionResult> {
    let Some(main) = cx
        .try_global::<GlobalMainView>()
        .and_then(|global| global.view.upgrade())
    else {
        return vec![UiActionResult::failure(
            "terminal",
            "The workspace is not available yet.",
        )];
    };

    let mut results = Vec::new();
    for parsed in parse_terminal_actions_in_text(text) {
        match parsed {
            ParsedTerminalAction::Action(action) => {
                let result = main.update(cx, |main, cx| {
                    main.apply_terminal_action(&action, caller_dashboard, caller_pane, cx)
                });
                results.push(result);
            }
            ParsedTerminalAction::Invalid(message) => {
                results.push(UiActionResult::failure("terminal", message));
            }
        }
    }
    results
}
