use gpui_component::button::Button;
use gpui_component::StyledExt;

use gpui::*;

/// Account page for CoinWatch billing. Credit balances and checkout are managed
/// by the hosted CoinWatch account UI, so the desktop client never handles
/// payment credentials.
pub struct ProfilePage;

impl ProfilePage {
    pub fn build_element() -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .size_full()
            .gap(px(12.0))
            .bg(green().opacity(0.1))
            .child(div().text_3xl().font_bold().child("Profile"))
            .child(div().text_sm().child("Market data uses CoinWatch credits (ticker 1 · charts, depth and trades 2)."))
            .child(
                Button::new("manage-credits")
                    .label("Manage credits & subscription")
                    .on_click(|_, _, _| {
                        let _ = webbrowser::open("https://coinwat.ch/account/credits");
                    }),
            )
    }
}

impl Render for ProfilePage {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        Self::build_element()
    }
}
