//! Title-bar controls for chart panes.
//!
//! Four separate controls sit next to the timeframe controls:
//! `[Type] [Indicators] [Drawings] [View]`. Keeping them separate at every pane
//! width is intentional: there is no single chart-feature overflow menu.

use gpui::{
    AnyElement, App, Corner, ElementId, Entity, FontWeight, IntoElement, ParentElement,
    SharedString, Styled, Window, div, prelude::FluentBuilder, px,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::menu::{DropdownMenu, PopupMenuItem};
use gpui_component::theme::ActiveTheme;
use gpui_component::{Disableable, Icon, Sizable, h_flex};

use super::chart_config::{CANDLE_TYPE_GROUPS, candle_type_label};
use super::renderer::{PaneGridRenderer, update_pane_config};
use crate::chart::color::hex_to_hsla;
use crate::chart::drawings::DrawingTool;
use crate::components::ui::chart_panel::ChartPanel;
use crate::components::ui::chart_tools::{ChartPaneBinding, ChartToolKind, open_chart_tool_window};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ControlWidth {
    /// Icon + short label + caret.
    Wide,
    /// Icon + caret; the label moves to the tooltip.
    Medium,
    /// 20 px icon + caret, still separate buttons.
    Compact,
}

impl ControlWidth {
    fn from_pane_width(width: f32) -> Self {
        // The labeled cluster needs ~610 px of title bar next to the six
        // timeframes and the pane actions, so labels start at 720 rather than
        // at the 700 px design threshold, where they would clip the View
        // control by a few pixels.
        if width >= 720.0 {
            Self::Wide
        } else if width >= 460.0 {
            Self::Medium
        } else {
            Self::Compact
        }
    }

    fn shows_label(self) -> bool {
        self == Self::Wide
    }

    /// Separation between the four controls: 4 px when there is room, 2 px in
    /// dense panes so the View control is not pushed out of the bar.
    fn gap(self) -> f32 {
        if self == Self::Wide { 4.0 } else { 2.0 }
    }
}

pub(super) fn render_chart_controls(
    pane_id: &str,
    grid: &Entity<PaneGridRenderer>,
    chart: Option<&Entity<ChartPanel>>,
    pane_width: f32,
    is_active: bool,
    title_fg: gpui::Hsla,
    cx: &App,
) -> AnyElement {
    let colors = &cx.theme().colors;
    let width = ControlWidth::from_pane_width(pane_width);
    let chart_type = chart
        .map(|chart| candle_type_of(chart, cx))
        .unwrap_or_else(|| "candle_solid".to_string());
    let indicator_count = chart
        .map(|chart| chart.read(cx).indicator_count())
        .unwrap_or(0);
    let active_tool = chart.and_then(|chart| chart.read(cx).active_drawing_tool());
    let snap = chart
        .map(|chart| chart.read(cx).snap_to_candle())
        .unwrap_or(true);
    let grid_visible = chart
        .map(|chart| chart.read(cx).horizontal_grid_visible())
        .unwrap_or(false);
    let drawing_color = chart
        .map(|chart| chart.read(cx).drawing_color_hex().to_string())
        .unwrap_or_else(|| "#C4E855".to_string());

    // On an active pane the title bar is filled with `primary`, so state
    // signals must use the title foreground there to stay visible. Colour is
    // still reserved for the armed-tool dot, never for plain metadata.
    let accent = if is_active {
        colors.primary_foreground
    } else {
        colors.primary
    };

    let pane = pane_id.to_string();

    let type_button = {
        let pane = pane.clone();
        let current = chart_type.clone();
        Button::new(ElementId::Name(SharedString::from(format!(
            "chart-type-{pane_id}"
        ))))
        .icon(Icon::default().path("icons/chart-candlestick.svg"))
        .when(width.shows_label(), |button| {
            button.label(short_type_label(&current))
        })
        .xsmall()
        .ghost()
        .text_color(title_fg)
        .tooltip(format!("Chart type: {}", candle_type_label(&chart_type)))
        .dropdown_caret(true)
        .dropdown_menu_with_anchor(Corner::BottomLeft, move |mut menu, _window, _cx| {
            for (group, entries) in CANDLE_TYPE_GROUPS {
                menu = menu.label(group);
                for (value, label) in entries {
                    let pane = pane.clone();
                    let value = *value;
                    menu = menu.item(
                        PopupMenuItem::new(*label)
                            .checked(current == value)
                            .on_click(move |_, window, cx| {
                                update_pane_config(
                                    pane.clone(),
                                    "type",
                                    serde_json::Value::String(value.to_string()),
                                    window,
                                    cx,
                                );
                            }),
                    );
                }
            }

            menu
        })
    };

    let indicators_button = {
        let binding = ChartPaneBinding {
            grid: grid.clone(),
            pane_id: pane_id.to_string(),
        };
        let button = Button::new(ElementId::Name(SharedString::from(format!(
            "chart-indicators-{pane_id}"
        ))))
        .icon(Icon::default().path("icons/chart-pie.svg"))
        .when(width.shows_label(), |button| button.label("Indicators"))
        .xsmall()
        .ghost()
        .text_color(if chart.is_none() {
            title_fg.opacity(0.55)
        } else {
            title_fg
        })
        .disabled(chart.is_none())
        .tooltip(if indicator_count > 0 {
            format!("Indicators ({indicator_count} on chart)")
        } else {
            "Indicators".to_string()
        })
        .on_click(move |_, window, cx| {
            open_chart_tool_window(ChartToolKind::Indicators, binding.clone(), window, cx);
        });

        // Below 500 px the title bar has no room for the count pill; the
        // tooltip already reports "(n on chart)".
        let show_badge = indicator_count > 0 && pane_width >= 500.0;
        h_flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(2.0))
            .child(button)
            .when(show_badge, |this| {
                this.child(
                    div()
                        .flex_shrink_0()
                        .min_w(px(14.0))
                        .px(px(4.0))
                        .h(px(14.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(7.0))
                        .bg(title_fg.opacity(0.16))
                        .text_size(px(9.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(title_fg)
                        .child(SharedString::from(indicator_count.to_string())),
                )
            })
    };

    let drawings_button = {
        let chart = chart.cloned();
        let grid = grid.clone();
        let trigger_label = active_tool.map(short_tool_label).unwrap_or("Drawings");
        let color = hex_to_hsla(&drawing_color).unwrap_or(colors.foreground);
        let border_color = colors.border;
        let current_tool = active_tool;
        let has_chart = chart.is_some();

        let binding = ChartPaneBinding {
            grid: grid.clone(),
            pane_id: pane_id.to_string(),
        };
        let button = Button::new(ElementId::Name(SharedString::from(format!(
            "chart-drawings-{pane_id}"
        ))))
        .icon(Icon::default().path("icons/trending-up.svg"))
        .when(width.shows_label(), |button| button.label(trigger_label))
        .xsmall()
        .ghost()
        .text_color(title_fg)
        .tooltip(active_tool.map(|tool| tool.label()).unwrap_or("Drawings"))
        .dropdown_caret(true)
        .dropdown_menu_with_anchor(Corner::BottomLeft, move |mut menu, _window, _cx| {
            let chart = chart.clone();
            let binding = binding.clone();

            menu = menu.item(
                PopupMenuItem::new("Pointer")
                    .checked(current_tool.is_none())
                    .on_click({
                        let chart = chart.clone();
                        let grid = grid.clone();
                        move |_, window, cx| {
                            apply_drawing_tool(&chart, &grid, None, window, cx);
                        }
                    }),
            );

            menu = menu.label("Lines");
            for tool in [
                DrawingTool::Trendline,
                DrawingTool::Ray,
                DrawingTool::Segment,
                DrawingTool::HorizontalLine,
                DrawingTool::VerticalLine,
            ] {
                menu = drawing_tool_item(menu, tool, current_tool, &chart, &grid);
            }

            menu = menu.label("Channels & Fib");
            for tool in [DrawingTool::PriceChannel, DrawingTool::Fibonacci] {
                menu = drawing_tool_item(menu, tool, current_tool, &chart, &grid);
            }

            menu = menu.label("Annotations");
            menu = drawing_tool_item(menu, DrawingTool::Text, current_tool, &chart, &grid);

            menu = menu.label("Positions");
            for tool in [DrawingTool::LongPosition, DrawingTool::ShortPosition] {
                menu = drawing_tool_item(menu, tool, current_tool, &chart, &grid);
            }

            menu = menu.separator().label("Style");
            menu = menu.item(
                PopupMenuItem::element(move |_window, _cx| {
                    h_flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(
                            div()
                                .size(px(12.0))
                                .rounded(px(2.0))
                                .border_1()
                                .border_color(border_color)
                                .bg(color),
                        )
                        .child("Drawing color…")
                })
                .disabled(!has_chart)
                .on_click({
                    let binding = binding.clone();
                    move |_, window, cx| {
                        open_chart_tool_window(ChartToolKind::Color, binding.clone(), window, cx);
                    }
                }),
            );
            menu = menu.item(
                PopupMenuItem::new("Snap to candles")
                    .checked(snap)
                    .disabled(!has_chart)
                    .on_click({
                        let chart = chart.clone();
                        let grid = grid.clone();
                        move |_, _window, cx| {
                            if let Some(chart) = &chart {
                                chart.update(cx, |panel, cx| {
                                    let next = !panel.snap_to_candle();
                                    panel.set_snap_to_candle(next);
                                    cx.notify();
                                });
                                grid.update(cx, |_, cx| cx.notify());
                            }
                        }
                    }),
            );
            menu.item(
                PopupMenuItem::new("Clear all drawings")
                    .disabled(!has_chart)
                    .on_click({
                        let chart = chart.clone();
                        let grid = grid.clone();
                        move |_, _window, cx| {
                            if let Some(chart) = &chart {
                                chart.update(cx, |panel, cx| {
                                    panel.clear_all_drawings();
                                    cx.notify();
                                });
                                grid.update(cx, |_, cx| cx.notify());
                            }
                        }
                    }),
            )
        });

        if width.shows_label() || active_tool.is_none() {
            button.into_any_element()
        } else {
            div()
                .relative()
                .flex_shrink_0()
                .child(button)
                .child(
                    div()
                        .absolute()
                        .top(px(1.0))
                        .right(px(2.0))
                        .size(px(5.0))
                        .rounded(px(2.5))
                        .bg(accent),
                )
                .into_any_element()
        }
    };

    let view_button = {
        let chart = chart.cloned();
        let grid = grid.clone();
        Button::new(ElementId::Name(SharedString::from(format!(
            "chart-view-{pane_id}"
        ))))
        .icon(Icon::default().path("icons/frame.svg"))
        .when(width.shows_label(), |button| button.label("View"))
        .xsmall()
        .ghost()
        .text_color(title_fg)
        .tooltip("Chart view")
        .dropdown_caret(true)
        .dropdown_menu_with_anchor(Corner::BottomLeft, move |mut menu, _window, _cx| {
            let chart = chart.clone();
            let grid = grid.clone();
            menu = menu.item(
                PopupMenuItem::new("Horizontal grid (G)")
                    .checked(grid_visible)
                    .disabled(chart.is_none())
                    .on_click({
                        let chart = chart.clone();
                        let grid = grid.clone();
                        move |_, _window, cx| {
                            if let Some(chart) = &chart {
                                chart.update(cx, |panel, cx| {
                                    panel.toggle_horizontal_grid();
                                    cx.notify();
                                });
                                grid.update(cx, |_, cx| cx.notify());
                            }
                        }
                    }),
            );
            menu = menu.separator();
            menu = menu.item(
                PopupMenuItem::new("Reset view (F)")
                    .disabled(chart.is_none())
                    .on_click({
                        let chart = chart.clone();
                        let grid = grid.clone();
                        move |_, _window, cx| {
                            if let Some(chart) = &chart {
                                chart.update(cx, |panel, cx| {
                                    panel.reset_view_all(cx);
                                    cx.notify();
                                });
                                grid.update(cx, |_, cx| cx.notify());
                            }
                        }
                    }),
            );
            menu.item(
                PopupMenuItem::new("Go to latest (L)")
                    .disabled(chart.is_none())
                    .on_click({
                        let chart = chart.clone();
                        let grid = grid.clone();
                        move |_, _window, cx| {
                            if let Some(chart) = &chart {
                                chart.update(cx, |panel, cx| {
                                    panel.go_to_latest_all(cx);
                                    cx.notify();
                                });
                                grid.update(cx, |_, cx| cx.notify());
                            }
                        }
                    }),
            )
        })
    };

    // Four separate controls, not a segmented strip: the gap reads as
    // separation while the 6 px title-bar gap still groups them away from the
    // timeframes.
    h_flex()
        .flex_shrink_0()
        .items_center()
        .gap(px(width.gap()))
        .child(type_button)
        .child(indicators_button)
        .child(drawings_button)
        .child(view_button)
        .into_any_element()
}

fn drawing_tool_item(
    menu: gpui_component::menu::PopupMenu,
    tool: DrawingTool,
    current: Option<DrawingTool>,
    chart: &Option<Entity<ChartPanel>>,
    grid: &Entity<PaneGridRenderer>,
) -> gpui_component::menu::PopupMenu {
    let chart = chart.clone();
    let grid = grid.clone();
    menu.item(
        PopupMenuItem::new(tool.label())
            .checked(current == Some(tool))
            .disabled(chart.is_none())
            .on_click(move |_, window, cx| {
                apply_drawing_tool(&chart, &grid, Some(tool), window, cx);
            }),
    )
}

fn apply_drawing_tool(
    chart: &Option<Entity<ChartPanel>>,
    grid: &Entity<PaneGridRenderer>,
    tool: Option<DrawingTool>,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(chart) = chart else {
        return;
    };
    let focus = chart.read(cx).chart_focus_handle();
    chart.update(cx, |panel, cx| {
        panel.set_active_drawing_tool(tool);
        cx.notify();
    });
    window.focus(&focus);
    grid.update(cx, |_, cx| cx.notify());
    window.refresh();
}

fn candle_type_of(chart: &Entity<ChartPanel>, cx: &App) -> String {
    chart.read(cx).candle_type_config().to_string()
}

/// Short chart-type label for the title-bar trigger. The full label stays in
/// the tooltip and the menu; the trigger has to survive a 700 px pane next to
/// the timeframe buttons and the pane actions.
fn short_type_label(config: &str) -> &'static str {
    match config {
        "candle_solid" => "Solid",
        "candle_stroke" => "Stroke",
        "candle_up_stroke" => "Up",
        "candle_down_stroke" => "Down",
        "heikin_ashi" => "Heikin",
        "ohlc" => "OHLC",
        "area" => "Area",
        _ => "Solid",
    }
}

fn short_tool_label(tool: DrawingTool) -> &'static str {
    match tool {
        DrawingTool::Trendline => "Trend",
        DrawingTool::Ray => "Ray",
        DrawingTool::Segment => "Seg",
        DrawingTool::HorizontalLine => "Horiz",
        DrawingTool::VerticalLine => "Vert",
        DrawingTool::Fibonacci => "Fib",
        DrawingTool::PriceChannel => "Chan",
        DrawingTool::Text => "Note",
        DrawingTool::LongPosition => "Long",
        DrawingTool::ShortPosition => "Short",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::ui::pane_grid::chart_config::CANDLE_TYPES;

    #[test]
    fn control_width_tiers_keep_the_cluster_inside_the_title_bar() {
        assert_eq!(ControlWidth::from_pane_width(900.0), ControlWidth::Wide);
        assert_eq!(ControlWidth::from_pane_width(720.0), ControlWidth::Wide);
        assert_eq!(ControlWidth::from_pane_width(719.0), ControlWidth::Medium);
        assert_eq!(ControlWidth::from_pane_width(460.0), ControlWidth::Medium);
        assert_eq!(ControlWidth::from_pane_width(459.0), ControlWidth::Compact);
        assert!(ControlWidth::Wide.shows_label());
        assert!(!ControlWidth::Medium.shows_label());
        assert_eq!(ControlWidth::Wide.gap(), 4.0);
        assert_eq!(ControlWidth::Medium.gap(), 2.0);
        assert_eq!(ControlWidth::Compact.gap(), 2.0);
    }

    #[test]
    fn every_chart_type_has_a_short_trigger_label() {
        for (config, _) in CANDLE_TYPES {
            let short = short_type_label(config);
            assert!(!short.is_empty(), "{config} has no short label");
            assert!(
                short.chars().count() <= 6,
                "{short} is too wide for the title-bar trigger"
            );
        }
    }
}
