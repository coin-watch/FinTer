//! Delete Dashboard Dialog component.
//!
//! A confirmation dialog for deleting dashboards.
//! Uses the GPUI Dialog system for proper modal behavior.

use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement, ParentElement, Render,
    SharedString, Styled, Window, div, prelude::*, px,
};
use gpui_component::WindowExt;
use gpui_component::button::ButtonVariant;
use gpui_component::dialog::DialogButtonProps;
use gpui_component::theme::ActiveTheme;

/// Events emitted by the DeleteDashboardDialog
pub enum DeleteDashboardDialogEvent {
    /// User confirmed deletion (module_id)
    Confirm(String),
    /// User cancelled the dialog
    Cancel,
}

/// Dialog for confirming dashboard deletion
pub struct DeleteDashboardDialog {
    focus_handle: FocusHandle,
    /// Relative path to delete
    module_id: String,
    /// Display name for the entry
    module_name: String,
    is_folder: bool,
    child_count: usize,
}

impl DeleteDashboardDialog {
    /// Create a new DeleteDashboardDialog
    pub fn new(
        module_id: String,
        module_name: String,
        is_folder: bool,
        child_count: usize,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            module_id,
            module_name,
            is_folder,
            child_count,
        }
    }

    /// Confirm the deletion
    fn confirm(&mut self, cx: &mut Context<Self>) {
        cx.emit(DeleteDashboardDialogEvent::Confirm(self.module_id.clone()));
    }

    /// Cancel the dialog
    fn cancel(&mut self, cx: &mut Context<Self>) {
        cx.emit(DeleteDashboardDialogEvent::Cancel);
    }
}

impl EventEmitter<DeleteDashboardDialogEvent> for DeleteDashboardDialog {}

impl Focusable for DeleteDashboardDialog {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for DeleteDashboardDialog {
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
                    .text_color(colors.foreground)
                    .child(if self.is_folder {
                        if self.child_count > 0 {
                            format!(
                                "Delete the folder \"{}\" and its {} layout{}?",
                                self.module_name,
                                self.child_count,
                                if self.child_count == 1 { "" } else { "s" }
                            )
                        } else {
                            format!("Delete the empty folder \"{}\"?", self.module_name)
                        }
                    } else {
                        format!("Delete the layout \"{}\"?", self.module_name)
                    }),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(colors.muted_foreground)
                    .child(if self.is_folder {
                        "The folder and everything inside it will be removed from disk."
                    } else {
                        "The layout file will be removed from disk."
                    }),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(colors.muted_foreground)
                    .child("This action cannot be undone."),
            )
    }
}

/// Helper function to open the delete dashboard dialog.
/// Returns the dialog entity for event subscription.
pub fn open_delete_dashboard_dialog(
    module_id: String,
    module_name: String,
    is_folder: bool,
    child_count: usize,
    window: &mut Window,
    cx: &mut App,
) -> Entity<DeleteDashboardDialog> {
    let dialog_entity = cx.new(|cx| {
        DeleteDashboardDialog::new(module_id, module_name.clone(), is_folder, child_count, cx)
    });
    let dialog_entity_clone = dialog_entity.clone();
    let dialog_entity_for_ok = dialog_entity.clone();
    let dialog_entity_for_cancel = dialog_entity.clone();

    window.open_dialog(cx, move |dialog, _window, _cx| {
        let ok_entity = dialog_entity_for_ok.clone();
        let cancel_entity = dialog_entity_for_cancel.clone();

        dialog
            .title(SharedString::from(format!("Delete \"{}\"", module_name)))
            .w(px(400.0))
            .child(dialog_entity_clone.clone())
            .button_props(
                DialogButtonProps::default()
                    .ok_text("Delete")
                    .ok_variant(ButtonVariant::Danger)
                    .cancel_text("Cancel"),
            )
            .on_ok(move |_, _, cx| {
                ok_entity.update(cx, |this, cx| {
                    this.confirm(cx);
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
