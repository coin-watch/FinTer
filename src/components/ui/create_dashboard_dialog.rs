//! Create Dashboard Dialog component.
//!
//! A dialog-based component for creating new dashboards with a name input.
//! Uses the GPUI Dialog system for proper modal behavior.

use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement, ParentElement, Render,
    Styled, Window, div, prelude::*, px,
};
use gpui_component::WindowExt;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::theme::ActiveTheme;

/// Events emitted by the CreateDashboardDialog
pub enum CreateDashboardDialogEvent {
    /// User confirmed creation with the dashboard name
    Create(String),
    /// User cancelled the dialog
    Cancel,
}

/// Dialog for creating a new dashboard
pub struct CreateDashboardDialog {
    focus_handle: FocusHandle,
    /// Input state for the dashboard name
    input_state: Entity<InputState>,
}

impl CreateDashboardDialog {
    /// Create a new CreateDashboardDialog
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input_state = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Enter dashboard name")
                .default_value("")
        });

        // Subscribe to Enter key press
        cx.subscribe(&input_state, |this, _entity, event: &InputEvent, cx| {
            if let InputEvent::PressEnter { .. } = event {
                this.submit(cx);
            }
        })
        .detach();

        Self {
            focus_handle: cx.focus_handle(),
            input_state,
        }
    }

    /// Submit the dashboard name
    fn submit(&mut self, cx: &mut Context<Self>) {
        let name = self
            .input_state
            .read(cx)
            .value()
            .to_string()
            .trim()
            .to_string();
        if !name.is_empty() {
            cx.emit(CreateDashboardDialogEvent::Create(name));
        }
    }

    /// Cancel the dialog
    fn cancel(&mut self, cx: &mut Context<Self>) {
        cx.emit(CreateDashboardDialogEvent::Cancel);
    }
}

impl EventEmitter<CreateDashboardDialogEvent> for CreateDashboardDialog {}

impl Focusable for CreateDashboardDialog {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for CreateDashboardDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let colors = &theme.colors;

        div()
            .flex()
            .flex_col()
            .gap(px(16.0))
            .child(
                div()
                    .text_sm()
                    .text_color(colors.muted_foreground)
                    .child("Enter a name for your new dashboard:"),
            )
            .child(Input::new(&self.input_state))
    }
}

/// Helper function to open the create dashboard dialog.
/// Returns the dialog entity for event subscription.
pub fn open_create_dashboard_dialog(
    window: &mut Window,
    cx: &mut App,
) -> Entity<CreateDashboardDialog> {
    let dialog_entity = cx.new(|cx| CreateDashboardDialog::new(window, cx));
    let dialog_entity_clone = dialog_entity.clone();
    let dialog_entity_for_ok = dialog_entity.clone();
    let dialog_entity_for_cancel = dialog_entity.clone();

    window.open_dialog(cx, move |dialog, _window, _cx| {
        let ok_entity = dialog_entity_for_ok.clone();
        let cancel_entity = dialog_entity_for_cancel.clone();

        dialog
            .title("Create New Dashboard")
            .w(px(400.0))
            .child(dialog_entity_clone.clone())
            .on_ok(move |_, _, cx| {
                ok_entity.update(cx, |this, cx| {
                    this.submit(cx);
                });
                true
            })
            .on_cancel(move |_, _, cx| {
                cancel_entity.update(cx, |this, cx| {
                    this.cancel(cx);
                });
                true
            })
            .confirm()
    });

    dialog_entity
}
