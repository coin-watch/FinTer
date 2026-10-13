use gpui::InteractiveElement;
use gpui::*;
use gpui_component::ActiveTheme;

#[derive(Clone)]
pub enum DragContent {
    Chart(Entity<crate::components::ui::chart_panel::ChartPanel>),
    Ticker(Entity<crate::components::ui::ticker_panel::TickerPanel>),
    Orderbook(Entity<crate::components::ui::orderbook_panel::OrderBookPanel>),
    Newsfeed(Entity<crate::components::ui::newsfeed_panel::NewsfeedPanel>),
    Clock(Entity<crate::components::ui::clock_panel::ClockPanel>),
    FearGreed(Entity<crate::components::ui::fear_greed_panel::FearGreedPanel>),
    Placeholder(String),
}

impl DragContent {
    fn element(&self) -> AnyElement {
        match self {
            Self::Chart(entity) => entity.clone().into_any_element(),
            Self::Ticker(entity) => entity.clone().into_any_element(),
            Self::Orderbook(entity) => entity.clone().into_any_element(),
            Self::Newsfeed(entity) => entity.clone().into_any_element(),
            Self::Clock(entity) => entity.clone().into_any_element(),
            Self::FearGreed(entity) => entity.clone().into_any_element(),
            Self::Placeholder(text) => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(text.clone())
                .into_any_element(),
        }
    }
}

pub struct DragView {
    pub module_type: String,
    pub content: DragContent,
    pub width: f32,
    pub height: f32,
    pub on_drop: std::rc::Rc<dyn Fn()>,
}

impl Drop for DragView {
    fn drop(&mut self) {
        (self.on_drop)();
    }
}

impl Render for DragView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();

        div()
            .w(px(self.width))
            .h(px(self.height))
            .p(px(4.0))
            .child(
                div()
                    .size_full()
                    .bg(theme.colors.background)
                    .border_2()
                    .border_color(theme.colors.primary)
                    .shadow_lg()
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .w_full()
                            .h(px(32.0))
                            .px(px(8.0))
                            .flex()
                            .items_center()
                            .bg(theme.colors.primary)
                            .text_color(theme.colors.primary_foreground)
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::BOLD)
                                    .child(self.module_type.clone()),
                            ),
                    )
                    .child(
                        div()
                            .relative()
                            .flex_1()
                            .w_full()
                            .min_h_0()
                            .overflow_hidden()
                            .child(self.content.element())
                            .child(div().absolute().inset_0().occlude()),
                    ),
            )
    }
}
