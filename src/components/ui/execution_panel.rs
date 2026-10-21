//! Read-only trade ticket preview. No exchange order API is available in FinTer.
//! Prices are sourced from the configured ticker bridge; nothing is submitted.
use gpui::prelude::FluentBuilder;
use gpui::{App, Context, EventEmitter, FocusHandle, Focusable, FontWeight, InteractiveElement, IntoElement, MouseButton, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Window, div, px};
use gpui_component::button::Button;
use gpui_component::dock::{Panel, PanelEvent, PanelInfo, PanelState};
use gpui_component::theme::ActiveTheme;
use gpui_component::{h_flex, v_flex};
use crate::data::feed::TickFeed;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Side { Long, Short }

pub struct ExecutionPanel {
    feed: TickFeed,
    title: String,
    symbol: String,
    focus_handle: FocusHandle,
    width: f32,
    height: f32,
    side: Side,
    leverage: u32,
    margin: f64,
}
impl ExecutionPanel {
    pub fn new(feed: TickFeed, title: impl Into<String>, symbol: impl Into<String>, cx: &mut App) -> Self {
        Self { feed, title: title.into(), symbol: symbol.into(), focus_handle: cx.focus_handle(), width: 0.0, height: 0.0, side: Side::Long, leverage: 10, margin: 100.0 }
    }
    pub fn set_size(&mut self, width: f32, height: f32) { self.width = width; self.height = height; }
    pub fn has_size(&self, width: f32, height: f32) -> bool { (self.width-width).abs() < 0.5 && (self.height-height).abs() < 0.5 }
    fn price(&self) -> Option<f64> { self.feed.snapshot().last().map(|t| t.price).filter(|p| p.is_finite() && *p > 0.0) }
    fn fmt(v: f64) -> String { if v >= 1000.0 { format!("{v:.2}") } else if v >= 1.0 { format!("{v:.4}") } else { format!("{v:.6}") } }
    fn side_button(&self, side: Side, label: &'static str, cx: &Context<Self>) -> gpui::Stateful<gpui::Div> {
        let c = &cx.theme().colors;
        let active = self.side == side;
        div().id(label).flex_1().py_1p5().rounded(px(3.0)).cursor_pointer().text_center().text_xs().font_weight(FontWeight::BOLD)
            .bg(if active { if side == Side::Long { c.bullish } else { c.bearish } } else { c.muted.opacity(0.35) })
            .text_color(if active { c.primary_foreground } else { c.muted_foreground })
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| { this.side = side; cx.notify(); })).child(label)
    }
    fn leverage_button(&self, lev: u32, cx: &Context<Self>) -> gpui::Stateful<gpui::Div> {
        let c = &cx.theme().colors;
        let active = self.leverage == lev;
        div().id(gpui::ElementId::Name(format!("preview-lev-{lev}").into())).px_2().py_1().rounded(px(3.0)).cursor_pointer().text_xs()
            .bg(if active { c.primary } else { c.muted.opacity(0.35) }).text_color(if active { c.primary_foreground } else { c.muted_foreground })
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| { this.leverage = lev; cx.notify(); })).child(format!("{lev}x"))
    }
    fn margin_button(&self, margin: f64, cx: &Context<Self>) -> gpui::Stateful<gpui::Div> {
        let c = &cx.theme().colors;
        let active = self.margin == margin;
        div().id(gpui::ElementId::Name(format!("preview-margin-{margin}").into())).px_2().py_1().rounded(px(3.0)).cursor_pointer().text_xs()
            .bg(if active { c.primary } else { c.muted.opacity(0.35) }).text_color(if active { c.primary_foreground } else { c.muted_foreground })
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| { this.margin = margin; cx.notify(); })).child(format!("${margin:.0}"))
    }
}
impl Render for ExecutionPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        window.request_animation_frame();
        let c = &cx.theme().colors;
        let mark = self.price();
        let narrow = self.width < 310.0;
        let notional = self.margin * self.leverage as f64;
        v_flex().size_full().overflow_hidden().bg(c.background)
            .child(h_flex().w_full().px_3().py_2().items_center().justify_between().bg(c.table_head).border_b_1().border_color(c.border)
                .child(h_flex().gap_2().items_center()
                    .child(div().px_2().py(px(2.0)).rounded(px(3.0)).bg(c.warning.opacity(0.16)).text_color(c.warning).text_xs().font_weight(FontWeight::BOLD).child("PREVIEW"))
                    .child(div().text_sm().font_weight(FontWeight::BOLD).text_color(c.foreground).child(format!("${}", mark.map(Self::fmt).unwrap_or_else(|| "—".into()))))
                .when(!narrow, |el| el.child(div().text_xs().text_color(c.muted_foreground).child("MARK PRICE")))))
            .child(v_flex().id("trade-desk-scroll").flex_1().min_h_0().overflow_y_scroll().p_3().gap_3()
                .child(div().p_2().rounded(px(4.0)).border_1().border_color(c.warning.opacity(0.4)).bg(c.warning.opacity(0.08)).text_xs().text_color(c.warning)
                    .child("READ ONLY · No live or simulated orders are submitted. Market prices are indicative, not fills."))
                .child(h_flex().gap_1().child(self.side_button(Side::Long, "BUY / LONG", cx)).child(self.side_button(Side::Short, "SELL / SHORT", cx)))
                .child(v_flex().gap_1().child(div().text_xs().font_weight(FontWeight::BOLD).text_color(c.muted_foreground).child("LEVERAGE · PREVIEW"))
                    .child(h_flex().flex_wrap().gap_1().children([1, 5, 10, 20, 50].into_iter().map(|n| self.leverage_button(n, cx)))))
                .child(v_flex().gap_1().child(div().text_xs().font_weight(FontWeight::BOLD).text_color(c.muted_foreground).child("MARGIN · PREVIEW"))
                    .child(h_flex().flex_wrap().gap_1().children([50.0, 100.0, 500.0, 1000.0].into_iter().map(|n| self.margin_button(n, cx)))))
                .child(v_flex().gap_2().p_3().rounded(px(4.0)).bg(c.muted.opacity(0.25))
                    .child(h_flex().justify_between().text_xs().text_color(c.muted_foreground).child("Indicative notional").child(format!("${notional:.2}")))
                    .child(h_flex().justify_between().text_xs().text_color(c.muted_foreground).child("Estimated quantity")
                        .child(mark.map(|p| format!("{} {}", Self::fmt(notional / p), self.symbol.split('/').next().unwrap_or("BASE")))
                            .unwrap_or_else(|| "Waiting for live mark".into()))))
                .child(div().w_full().py_2().rounded(px(4.0)).bg(c.muted.opacity(0.4)).text_center().text_xs().font_weight(FontWeight::BOLD)
                    .text_color(c.muted_foreground).child("ORDER SUBMISSION UNAVAILABLE")))
    }
}
impl Panel for ExecutionPanel {
    fn panel_name(&self) -> &'static str { "ExecutionPanel" }
    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement { SharedString::from(self.title.clone()) }
    fn toolbar_buttons(&mut self, _: &mut Window, _: &mut Context<Self>) -> Option<Vec<Button>> { None }
    fn inner_padding(&self, _: &App) -> bool { false }
    fn closable(&self, _: &App) -> bool { false }
    fn dump(&self, _: &App) -> PanelState { PanelState { panel_name: "ExecutionPanel".into(), children: vec![], info: PanelInfo::Panel(serde_json::json!({"title": self.title})) } }
}
impl EventEmitter<PanelEvent> for ExecutionPanel {}
impl Focusable for ExecutionPanel { fn focus_handle(&self, _: &App) -> FocusHandle { self.focus_handle.clone() } }
