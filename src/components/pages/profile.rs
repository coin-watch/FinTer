use gpui_component::StyledExt;

use gpui::*;

pub struct ProfilePage;

impl ProfilePage {
    pub fn build_element() -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .size_full()
            .bg(green().opacity(0.1))
            .child(div().text_3xl().font_bold().child("Profile Page"))
    }
}

impl Render for ProfilePage {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        Self::build_element()
    }
}
