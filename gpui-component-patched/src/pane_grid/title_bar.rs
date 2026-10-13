use super::Pane;
use gpui::{
    div, prelude::FluentBuilder, px, InteractiveElement, IntoElement, MouseButton, ParentElement,
    Styled,
};

/// A title bar for a pane with optional controls.
pub struct TitleBar {
    pane: Pane,
    title: String,
    on_close: Option<Box<dyn Fn(Pane) + Send + Sync>>,
}

impl TitleBar {
    /// Creates a new title bar.
    pub fn new(pane: Pane, title: impl Into<String>) -> Self {
        Self {
            pane,
            title: title.into(),
            on_close: None,
        }
    }

    /// Sets the close callback.
    pub fn on_close<F>(mut self, f: F) -> Self
    where
        F: Fn(Pane) + Send + Sync + 'static,
    {
        self.on_close = Some(Box::new(f));
        self
    }

    /// Renders the title bar as a div.
    pub fn render_div(self) -> impl IntoElement {
        let pane = self.pane;
        let title = self.title;
        let on_close = self.on_close;

        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .px(px(8.0))
            .py(px(4.0))
            .bg(gpui::rgb(0x2d2d2d))
            .border_b_1()
            .border_color(gpui::rgb(0x3d3d3d))
            .child(
                div()
                    .flex()
                    .items_center()
                    .text_sm()
                    .text_color(gpui::rgb(0xcccccc))
                    .child(title),
            )
            .when_some(on_close, |this, on_close| {
                this.child(
                    div()
                        .flex()
                        .items_center()
                        .justify_center()
                        .w(px(20.0))
                        .h(px(20.0))
                        .rounded(px(4.0))
                        .hover(|style| style.bg(gpui::rgb(0x3d3d3d)))
                        .cursor_pointer()
                        .on_mouse_down(MouseButton::Left, move |_event, _window, _cx| {
                            on_close(pane);
                        })
                        .child(div().text_xs().text_color(gpui::rgb(0xcccccc)).child("×")),
                )
            })
    }
}
