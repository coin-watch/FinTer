pub mod agent_chart;
pub mod agent_panel;
pub mod alerts_panel;
pub mod avatar;
pub mod chart_panel;
pub mod clock_panel;
pub mod credit_alert;
pub mod dashboard_manager;
pub mod delete_dashboard_dialog;
pub mod execution_panel;
pub mod fear_greed_panel;
pub mod market_cap_panel;
pub mod newsfeed_panel;
pub mod orderbook_panel;
pub mod pane_grid;
pub mod positions_panel;
pub mod sidebar;
pub mod tape_panel;
pub mod ticker_panel;
pub mod title_bar;
pub mod update_banner;
pub mod watchlist_panel;

use gpui::SharedString;
use gpui_component::select::SelectItem;

#[derive(Debug, Clone, PartialEq)]
pub struct ExchangeSelectItem {
    pub id: SharedString,
    pub name: SharedString,
}

impl ExchangeSelectItem {
    pub fn new(id: impl Into<SharedString>, name: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
        }
    }
}

impl SelectItem for ExchangeSelectItem {
    type Value = SharedString;

    fn title(&self) -> SharedString {
        self.name.clone()
    }

    fn value(&self) -> &Self::Value {
        &self.id
    }
}

