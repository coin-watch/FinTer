//! Empty Module Selector — A bold, distinctive UI for selecting module types.
//!
//! Design: Industrial Terminal aesthetic with:
//! - Grid-based card layout for module type selection
//! - Geometric iconography and sharp lines
//! - Hover glow effects with accent colors
//! - Monospace typography for that authentic terminal feel

use gpui::{
    Context, ElementId, EventEmitter, Hsla, InteractiveElement, IntoElement, ParentElement, Render,
    SharedString, StatefulInteractiveElement, Styled, Window, div, px,
};
use gpui::prelude::FluentBuilder;
use gpui_component::theme::ActiveTheme;
use gpui_component::{h_flex, v_flex};

use super::pane_grid::{ChangeModuleTypeEvent, ModuleType};
use super::pane_tree::Pane;

// ---------------------------------------------------------------------------
// Module Card Metadata
// ---------------------------------------------------------------------------

struct ModuleCardInfo {
    module_type: ModuleType,
    icon: &'static str,
    label: &'static str,
    description: &'static str,
    accent_role: AccentRole,
}

#[derive(Clone, Copy)]
enum AccentRole {
    Chart,
    Tape,
    Book,
    Watchlist,
    Positions,
}

fn module_cards() -> Vec<ModuleCardInfo> {
    vec![
        ModuleCardInfo {
            module_type: ModuleType::Chart,
            icon: "OHLC",
            label: "CHART",
            description: "Candlestick price action",
            accent_role: AccentRole::Chart,
        },
        ModuleCardInfo {
            module_type: ModuleType::Tape,
            icon: "T&S",
            label: "TAPE",
            description: "Time & sales feed",
            accent_role: AccentRole::Tape,
        },
        ModuleCardInfo {
            module_type: ModuleType::OrderBook,
            icon: "L2",
            label: "BOOK",
            description: "Level 2 market depth",
            accent_role: AccentRole::Book,
        },
        ModuleCardInfo {
            module_type: ModuleType::Watchlist,
            icon: "WL",
            label: "WATCHLIST",
            description: "Multi-symbol monitor",
            accent_role: AccentRole::Watchlist,
        },
        ModuleCardInfo {
            module_type: ModuleType::Positions,
            icon: "POS",
            label: "POSITIONS",
            description: "Open position P&L",
            accent_role: AccentRole::Positions,
        },
    ]
}

// ---------------------------------------------------------------------------
// EmptyModuleSelector
// ---------------------------------------------------------------------------

/// A visually striking module type selector for empty panes.
/// Renders a grid of cards, each representing a module type with
/// distinctive iconography and hover effects.
pub struct EmptyModuleSelector {
    pane_id: Pane,
    hovered_type: Option<ModuleType>,
}

impl EmptyModuleSelector {
    fn module_accent(&self, role: AccentRole, cx: &Context<Self>) -> Hsla {
        let colors = &cx.theme().colors;
        match role {
            AccentRole::Chart => colors.chart_1,
            AccentRole::Tape => colors.bullish,
            AccentRole::Book => colors.warning,
            AccentRole::Watchlist => colors.cyan,
            AccentRole::Positions => colors.primary,
        }
    }

    pub fn new(pane_id: Pane) -> Self {
        Self {
            pane_id,
            hovered_type: None,
        }
    }

    fn render_module_card(
        &self,
        info: &ModuleCardInfo,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let is_hovered = self.hovered_type == Some(info.module_type);
        let module_type = info.module_type;
        let pane_id = self.pane_id;
        let accent = self.module_accent(info.accent_role, cx);
        let colors = &cx.theme().colors;

        // Colors
        let card_bg = if is_hovered {
            Hsla { a: 0.18, ..accent }
        } else {
            colors.tiles
        };

        let border_color = if is_hovered { accent } else { colors.border };

        let icon_bg = if is_hovered {
            Hsla { a: 0.22, ..accent }
        } else {
            colors.muted
        };

        let icon_border = if is_hovered {
            Hsla { a: 0.4, ..accent }
        } else {
            colors.border
        };

        let icon_text = if is_hovered {
            accent
        } else {
            colors.muted_foreground
        };

        let label_color = if is_hovered {
            colors.foreground
        } else {
            colors.muted_foreground
        };

        let desc_color = Hsla {
            a: 0.85,
            ..colors.muted_foreground
        };

        div()
            .id(ElementId::Name(SharedString::from(format!(
                "module-card-{}",
                info.label
            ))))
            .w(px(140.0))
            .h(px(120.0))
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_2()
            .rounded_sm()
            .cursor_pointer()
            .bg(card_bg)
            .border_1()
            .border_color(border_color)
            .on_mouse_move(cx.listener(move |this, _, _, cx| {
                if this.hovered_type != Some(module_type) {
                    this.hovered_type = Some(module_type);
                    cx.notify();
                }
            }))
            .on_click(cx.listener(move |_this, _, _window, cx| {
                cx.emit(ChangeModuleTypeEvent {
                    pane_id,
                    new_type: module_type,
                });
            }))
            // Icon badge
            .child(
                div()
                    .w(px(48.0))
                    .h(px(32.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_sm()
                    .bg(icon_bg)
                    .border_1()
                    .border_color(icon_border)
                    .child(
                        div()
                            .text_xs()
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(icon_text)
                            .child(info.icon),
                    ),
            )
            // Label
            .child(
                div()
                    .text_xs()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(label_color)
                    .child(info.label),
            )
            // Description
            .child(
                div()
                    .text_color(desc_color)
                    .child(div().text_xs().child(info.description)),
            )
    }
}

impl EventEmitter<ChangeModuleTypeEvent> for EmptyModuleSelector {}

impl Render for EmptyModuleSelector {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let cards = module_cards();
        let colors = &cx.theme().colors;

        // Colors
        let bg = colors.background;
        let overlay_bg = Hsla {
            a: 0.08,
            ..colors.foreground
        };
        let corner_color = colors.border;
        let header_color = colors.muted_foreground;
        let divider_color = colors.border;
        let hint_bg = colors.secondary;
        let hint_border = colors.border;
        let hint_text = colors.muted_foreground;

        v_flex()
            .id("empty-module-selector")
            .size_full()
            .bg(bg)
            .items_center()
            .justify_center()
            .gap_6()
            .relative()
            // Subtle grid pattern overlay
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .bg(overlay_bg)
                    // Decorative corner accents - top left
                    .child(
                        div()
                            .absolute()
                            .top_4()
                            .left_4()
                            .w(px(24.0))
                            .h(px(24.0))
                            .border_l_2()
                            .border_t_2()
                            .border_color(corner_color),
                    )
                    // Top right corner
                    .child(
                        div()
                            .absolute()
                            .top_4()
                            .right_4()
                            .w(px(24.0))
                            .h(px(24.0))
                            .border_r_2()
                            .border_t_2()
                            .border_color(corner_color),
                    )
                    // Bottom left corner
                    .child(
                        div()
                            .absolute()
                            .bottom_4()
                            .left_4()
                            .w(px(24.0))
                            .h(px(24.0))
                            .border_l_2()
                            .border_b_2()
                            .border_color(corner_color),
                    )
                    // Bottom right corner
                    .child(
                        div()
                            .absolute()
                            .bottom_4()
                            .right_4()
                            .w(px(24.0))
                            .h(px(24.0))
                            .border_r_2()
                            .border_b_2()
                            .border_color(corner_color),
                    ),
            )
            // Header
            .child(
                v_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(header_color)
                            .child("SELECT MODULE"),
                    )
                    .child(div().w(px(40.0)).h(px(1.0)).bg(divider_color)),
            )
            // Module cards grid - 3x2
            // Build cards directly to avoid closure capture issues
            .child(
                v_flex()
                    .gap_3()
                    .child(
                        h_flex()
                            .gap_3()
                            .child(self.render_module_card(&cards[0], cx))
                            .child(self.render_module_card(&cards[1], cx))
                            .child(self.render_module_card(&cards[2], cx)),
                    )
                    .child(
                        h_flex()
                            .gap_3()
                            .child(self.render_module_card(&cards[3], cx))
                            .child(self.render_module_card(&cards[4], cx)),
                    ),
            )
            // Footer hint
            .child(
                h_flex().gap_2().items_center().child(
                    div()
                        .px_2()
                        .py_0p5()
                        .rounded_sm()
                        .bg(hint_bg)
                        .border_1()
                        .border_color(hint_border)
                        .child(
                            div()
                                .text_xs()
                                .text_color(hint_text)
                                .child("Click to configure"),
                        ),
                ),
            )
            // Clear hover when leaving
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                if this.hovered_type.is_some() {
                    this.hovered_type = None;
                    cx.notify();
                }
            }))
    }
}
