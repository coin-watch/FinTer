use gpui::{Context, IntoElement, ParentElement, Render, Styled, Window, div};

#[derive(Default)]
pub struct DashboardPage {}

impl Render for DashboardPage {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .child("Dashboard Page - Pane Grid Removed")
    }
}

impl DashboardPage {
    pub fn new() -> Self {
        Self {}
    }
}
