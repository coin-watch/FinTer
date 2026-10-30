use gpui::InteractiveElement;
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::ActiveTheme;

#[derive(Clone)]
pub enum DragContent {
    Chart(Entity<crate::components::ui::chart_panel::ChartPanel>),
    Ticker(Entity<crate::components::ui::ticker_panel::TickerPanel>),
    Orderbook(Entity<crate::components::ui::orderbook_panel::OrderBookPanel>),
    Agent(Entity<crate::components::ui::agent_panel::AgentPanel>),
    Tape(Entity<crate::components::ui::tape_panel::TapePanel>),
    Execution(Entity<crate::components::ui::execution_panel::ExecutionPanel>),
    Positions(Entity<crate::components::ui::positions_panel::PositionsPanel>),
    Watchlist(Entity<crate::components::ui::watchlist_panel::WatchlistPanel>),
    MarketCap(Entity<crate::components::ui::market_cap_panel::MarketCapPanel>),
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
            Self::Agent(entity) => entity.clone().into_any_element(),
            Self::Tape(entity) => entity.clone().into_any_element(),
            Self::Execution(entity) => entity.clone().into_any_element(),
            Self::Positions(entity) => entity.clone().into_any_element(),
            Self::Watchlist(entity) => entity.clone().into_any_element(),
            Self::MarketCap(entity) => entity.clone().into_any_element(),
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
    pub exchange: String,
    pub symbol: String,
    pub timeframe: String,
    pub timezone: String,
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
        let is_market_module = matches!(
            self.module_type.to_lowercase().as_str(),
            "chart" | "ticker" | "orderbook" | "agent" | "trades" | "tape" | "execution" | "tradedesk"
        );
        let suffix = match self.module_type.to_lowercase().as_str() {
            "orderbook" => Some("OB"),
            "agent" => Some("AGENT"),
            "trades" | "tape" => Some("TRADES"),
            "execution" | "tradedesk" => Some("DESK"),
            _ => None,
        };

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
                            .h(px(33.0))
                            .px(px(8.0))
                            .flex()
                            .items_center()
                            .justify_between()
                            .bg(theme.colors.primary)
                            .text_color(theme.colors.primary_foreground)
                            .border_b_1()
                            .border_color(theme.colors.border)
                            .child(
                                gpui_component::h_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap(px(6.0))
                                    .items_center()
                                    .overflow_hidden()
                                    .when(is_market_module, |this| {
                                        this.child(crate::components::ui::pane_grid::renderer::render_coinwatch_icons(
                                            "drag",
                                            &self.exchange,
                                            &self.symbol,
                                        ))
                                        .when_some(suffix, |this, s| {
                                            this.child(
                                                div()
                                                    .flex_shrink_0()
                                                    .px(px(4.0))
                                                    .py(px(1.0))
                                                    
                                                    .bg(theme.colors.background.opacity(0.2))
                                                    .border_1()
                                                    .border_color(theme.colors.primary_foreground.opacity(0.4))
                                                    .text_size(px(10.0))
                                                    .font_weight(FontWeight::BOLD)
                                                    .text_color(theme.colors.primary_foreground)
                                                    .child(s),
                                            )
                                        })
                                        .when(self.module_type.to_lowercase() == "chart", |this| {
                                            this.child(
                                                div()
                                                    .flex_shrink_0()
                                                    .w(px(1.0))
                                                    .h(px(14.0))
                                                    .bg(theme.colors.border.opacity(0.6))
                                                    .mx(px(2.0)),
                                            )
                                            .child(
                                                div()
                                                    .px(px(8.0))
                                                    .py(px(1.0))
                                                    
                                                    .bg(theme.colors.background)
                                                    .text_color(theme.colors.primary)
                                                    .text_size(px(11.0))
                                                    .font_weight(FontWeight::BOLD)
                                                    .child(self.timeframe.clone()),
                                            )
                                        })
                                    })
                                    .when(!is_market_module, |this| {
                                        this.child(
                                            div()
                                                .min_w_0()
                                                .truncate()
                                                .text_sm()
                                                .font_weight(FontWeight::MEDIUM)
                                                .child(
                                                    if self.module_type.to_lowercase() == "time" || self.module_type.to_lowercase() == "clock" {
                                                        self.timezone.replace('/', ", ").replace('_', " ")
                                                    } else if self.module_type.to_lowercase() == "marketcap" {
                                                        "Market Cap".to_string()
                                                    } else if self.module_type.to_lowercase() == "watchlist" {
                                                        "Watchlist".to_string()
                                                    } else if self.module_type.to_lowercase() == "feargreed" {
                                                        "Fear & Greed Index".to_string()
                                                    } else if self.module_type.to_lowercase() == "execution" || self.module_type.to_lowercase() == "tradedesk" {
                                                        "Trade Desk".to_string()
                                                    } else if self.module_type.to_lowercase() == "positions" {
                                                        "Positions".to_string()
                                                    } else {
                                                        self.module_type.clone()
                                                    },
                                                ),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(4.0))
                                    .text_color(theme.colors.primary_foreground.opacity(0.85))
                                    .child(gpui_component::Icon::default().path("icons/layout-dashboard.svg").size(px(14.0)))
                                    .child(gpui_component::Icon::default().path("icons/settings.svg").size(px(14.0)))
                                    .child(gpui_component::Icon::default().path("icons/maximize.svg").size(px(14.0)))
                                    .child(gpui_component::Icon::default().path("icons/close.svg").size(px(14.0))),
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
