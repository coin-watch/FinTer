//! Fixed-size floating drawing-colour picker.
//!
//! The window owns the interaction state for the HSV square/hue strip; colour
//! writes are forwarded to the bound chart so new drawings use the chosen
//! value. The palette and hex helpers are the same math used by the chart.

use gpui::{
    AnyElement, App, Context, ElementId, Entity, FocusHandle, Focusable, InteractiveElement,
    IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, ParentElement, Render,
    SharedString, StatefulInteractiveElement, Styled, WeakEntity, Window, canvas, div, point,
    prelude::FluentBuilder, px,
};
use gpui_component::PixelsExt;
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::scroll::ScrollableElement;
use gpui_component::theme::ActiveTheme;
use gpui_component::tooltip::Tooltip;
use gpui_component::{Icon, Sizable, h_flex, v_flex};

use super::ChartPaneBinding;
use crate::chart::color::{hex_to_hsla, hsla_to_hex, hsla_to_hsv, hsv_to_hsla};
use crate::components::ui::chart_panel::ChartPanel;
use crate::components::ui::pane_grid::renderer::PaneGridRenderer;

const SQUARE_WIDTH: f32 = 208.0;
const SQUARE_HEIGHT: f32 = 104.0;
const HUE_HEIGHT: f32 = 16.0;
const SWATCH_SIZE: f32 = 16.0;
const DEFAULT_COLOR: &str = "#C4E855";

/// Palette swatches in intent order: quick picks, greens, blues & violet, warm
/// accents, pink & red, neutrals. The five duplicates from the old grouped
/// palette are collapsed and the group captions removed so the whole picker
/// fits the fixed 240 × 340 window without scrolling.
const PALETTE: [&str; 25] = [
    "#C4E855", "#00F0FF", "#FFD600", "#FF0055", "#FFFFFF", // quick picks
    "#00E676", "#2EBD85", "#AEEA00", "#76FF03", // greens
    "#00B0FF", "#2979FF", "#651FFF", "#3D5AFE", // blues & violet
    "#FFEA00", "#FF9100", "#FF6D00", "#FF3D00", // warm accents
    "#F6465D", "#FF1744", "#F50057", "#D500F9", // pink & red
    "#ECEFF1", "#90A4AE", "#546E7A", "#263238", // neutrals
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum ColorDrag {
    Square,
    Hue,
}

pub struct ColorWindow {
    grid: WeakEntity<PaneGridRenderer>,
    pane_id: String,
    focus_handle: FocusHandle,
    hex: String,
    hue: f32,
    saturation: f32,
    value: f32,
    drag: Option<ColorDrag>,
    square_bounds: gpui::Bounds<gpui::Pixels>,
    hue_bounds: gpui::Bounds<gpui::Pixels>,
}

impl ColorWindow {
    pub fn new(binding: ChartPaneBinding, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let ChartPaneBinding { grid, pane_id } = binding;
        let initial_hex = grid
            .read(cx)
            .chart_for_pane(&pane_id)
            .map(|chart| chart.read(cx).drawing_color_hex().to_string())
            .unwrap_or_else(|| DEFAULT_COLOR.to_string());
        let parsed = hex_to_hsla(&initial_hex)
            .unwrap_or_else(|| hex_to_hsla(DEFAULT_COLOR).expect("default drawing colour"));
        let (hue, saturation, value) = hsla_to_hsv(parsed);
        let weak_grid = grid.downgrade();
        cx.observe(&grid, |_, _, cx| cx.notify()).detach();

        let focus_handle = cx.focus_handle();
        let focus_for_window = focus_handle.clone();
        window.defer(cx, move |window, _| {
            window.focus(&focus_for_window);
        });

        Self {
            grid: weak_grid,
            pane_id,
            focus_handle,
            hex: hsla_to_hex(parsed),
            hue,
            saturation,
            value,
            drag: None,
            square_bounds: gpui::Bounds::default(),
            hue_bounds: gpui::Bounds::default(),
        }
    }

    fn find_chart(&self, cx: &App) -> Option<Entity<ChartPanel>> {
        let grid = self.grid.upgrade()?;
        let grid = grid.read(cx);
        let module = grid.pane_module_type(&self.pane_id)?;
        if !module.eq_ignore_ascii_case("chart") {
            return None;
        }
        grid.chart_for_pane(&self.pane_id)
    }

    fn notify_grid(&self, cx: &mut App) {
        if let Some(grid) = self.grid.upgrade() {
            grid.update(cx, |_, cx| cx.notify());
        }
    }

    fn apply_hex(&mut self, hex: String, cx: &mut App) {
        self.hex = hex;
        if let Some(chart) = self.find_chart(cx) {
            let hex = self.hex.clone();
            chart.update(cx, |panel, cx| {
                panel.set_drawing_color_hex(&hex);
                cx.notify();
            });
            self.notify_grid(cx);
        }
    }

    fn set_hex_preserving_hue(&mut self, hex: &str, cx: &mut App) {
        let Some(color) = hex_to_hsla(hex) else {
            return;
        };
        let (hue, saturation, value) = hsla_to_hsv(color);
        if saturation > f32::EPSILON {
            self.hue = hue;
        }
        self.saturation = saturation;
        self.value = value;
        self.apply_hex(hsla_to_hex(color), cx);
    }

    fn set_hsv(&mut self, hue: f32, saturation: f32, value: f32, cx: &mut App) {
        self.hue = hue.rem_euclid(1.0);
        self.saturation = saturation.clamp(0.0, 1.0);
        self.value = value.clamp(0.0, 1.0);
        self.apply_hex(
            hsla_to_hex(hsv_to_hsla(self.hue, self.saturation, self.value)),
            cx,
        );
    }

    fn update_from_position(
        &mut self,
        position: gpui::Point<gpui::Pixels>,
        target: ColorDrag,
        cx: &mut App,
    ) {
        let (hue, saturation, value) = (self.hue, self.saturation, self.value);
        match target {
            ColorDrag::Square => {
                if self.square_bounds.size.width.as_f32() <= 0.0
                    || self.square_bounds.size.height.as_f32() <= 0.0
                {
                    return;
                }
                let x = ((position.x - self.square_bounds.origin.x).as_f32()
                    / self.square_bounds.size.width.as_f32())
                .clamp(0.0, 1.0);
                let y = ((position.y - self.square_bounds.origin.y).as_f32()
                    / self.square_bounds.size.height.as_f32())
                .clamp(0.0, 1.0);
                self.set_hsv(hue, x, 1.0 - y, cx);
            }
            ColorDrag::Hue => {
                if self.hue_bounds.size.width.as_f32() <= 0.0 {
                    return;
                }
                let x = ((position.x - self.hue_bounds.origin.x).as_f32()
                    / self.hue_bounds.size.width.as_f32())
                .clamp(0.0, 1.0);
                self.set_hsv(x, saturation, value, cx);
            }
        }
    }

    fn render_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors;
        let drag_region = h_flex()
            .flex_1()
            .min_w_0()
            .gap(px(8.0))
            .items_center()
            .child(Icon::default().path("icons/palette.svg").size(px(15.0)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_sm()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child("Drawing color"),
            );

        #[cfg(target_os = "linux")]
        let drag_region = drag_region.on_mouse_down(MouseButton::Left, |_, window, _| {
            window.start_window_move();
        });
        #[cfg(not(target_os = "linux"))]
        let drag_region = drag_region.window_control_area(gpui::WindowControlArea::Drag);

        h_flex()
            .id(ElementId::Name(SharedString::from(format!(
                "color-window-header-{}",
                self.pane_id
            ))))
            .flex_shrink_0()
            .h(px(36.0))
            .px(px(10.0))
            .gap(px(8.0))
            .items_center()
            .border_b_1()
            .border_color(colors.title_bar_border)
            .bg(colors.title_bar)
            .child(drag_region)
            .child(
                Button::new(ElementId::Name(SharedString::from(format!(
                    "color-window-close-{}",
                    self.pane_id
                ))))
                .icon(Icon::default().path("icons/close.svg"))
                .ghost()
                .xsmall()
                .tooltip("Close")
                .on_click(|_, window, cx| {
                    cx.stop_propagation();
                    window.remove_window();
                }),
            )
            .into_any_element()
    }

    fn render_closed(&self, cx: &App) -> AnyElement {
        let colors = cx.theme().colors;
        v_flex()
            .flex_1()
            .min_h_0()
            .items_center()
            .justify_center()
            .gap(px(6.0))
            .p(px(20.0))
            .child(
                Icon::default()
                    .path("icons/chart-candlestick.svg")
                    .size(px(22.0))
                    .text_color(colors.muted_foreground),
            )
            .child(
                div()
                    .text_sm()
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .child("Chart pane closed"),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(colors.muted_foreground)
                    .child("Open a chart pane to choose a drawing color."),
            )
            .into_any_element()
    }

    fn render_palette(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors;
        let selected = self.hex.clone();
        let mut grid = h_flex().flex_wrap().gap(px(4.0)).items_center();

        for (index, hex) in PALETTE.iter().enumerate() {
            let view = cx.entity().downgrade();
            let is_active = selected.eq_ignore_ascii_case(hex);
            let swatch = hex_to_hsla(hex).unwrap_or(colors.foreground);
            // Pair the selected outline with a check glyph so the choice never
            // depends on the swatch colour itself.
            let check = if swatch.l > 0.55 {
                colors.foreground
            } else {
                gpui::Hsla::white()
            };
            let hex = hex.to_string();
            grid = grid.child(
                div()
                    .id(ElementId::Name(SharedString::from(format!(
                        "color-swatch-{index}-{}",
                        self.pane_id
                    ))))
                    .size(px(SWATCH_SIZE))
                    .rounded(px(3.0))
                    .border_1()
                    .border_color(if is_active {
                        colors.foreground
                    } else {
                        colors.border.opacity(0.65)
                    })
                    .when(is_active, |this| this.border_2())
                    .bg(swatch)
                    .when(is_active, |this| {
                        this.child(
                            Icon::default()
                                .path("icons/check.svg")
                                .size(px(9.0))
                                .text_color(check),
                        )
                    })
                    .tooltip({
                        let hex = hex.clone();
                        move |window, cx| Tooltip::new(hex.clone()).build(window, cx)
                    })
                    .on_mouse_down(MouseButton::Left, {
                        let view = view.clone();
                        let hex = hex.clone();
                        move |_, _window, cx| {
                            let _ = view.update(cx, |this, cx| {
                                this.set_hex_preserving_hue(&hex, cx);
                                cx.notify();
                            });
                        }
                    }),
            );
        }

        v_flex()
            .gap(px(6.0))
            .child(
                div()
                    .text_size(px(9.0))
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(colors.muted_foreground)
                    .child("PALETTE"),
            )
            .child(grid)
            .into_any_element()
    }

    fn render_picker(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors;
        let mono_font = cx.theme().mono_font_family.clone();
        let view = cx.entity().downgrade();
        let hue_color = hsv_to_hsla(self.hue, 1.0, 1.0);
        // Keep the full marker ring inside the square at the extremes instead
        // of clipping it against the edges.
        let marker_x = (self.saturation * SQUARE_WIDTH - 6.0).clamp(0.0, SQUARE_WIDTH - 12.0);
        let marker_y = ((1.0 - self.value) * SQUARE_HEIGHT - 6.0).clamp(0.0, SQUARE_HEIGHT - 12.0);

        let square_view = view.clone();
        let square_prepaint_view = view.clone();
        let square_hue = self.hue;
        let square = div()
            .id(ElementId::Name(SharedString::from(format!(
                "color-square-{}",
                self.pane_id
            ))))
            .relative()
            .w(px(SQUARE_WIDTH))
            .h(px(SQUARE_HEIGHT))
            .overflow_hidden()
            .border_1()
            .border_color(colors.border)
            .cursor_crosshair()
            .on_mouse_down(MouseButton::Left, {
                let view = square_view.clone();
                move |event: &MouseDownEvent, _window, cx| {
                    cx.stop_propagation();
                    let _ = view.update(cx, |this, cx| {
                        this.drag = Some(ColorDrag::Square);
                        this.update_from_position(event.position, ColorDrag::Square, cx);
                        cx.notify();
                    });
                }
            })
            .on_mouse_move({
                let view = square_view.clone();
                move |event: &MouseMoveEvent, _window, cx| {
                    let _ = view.update(cx, |this, cx| {
                        if this.drag == Some(ColorDrag::Square) {
                            cx.stop_propagation();
                            this.update_from_position(event.position, ColorDrag::Square, cx);
                            cx.notify();
                        }
                    });
                }
            })
            .on_mouse_up(MouseButton::Left, {
                let view = square_view;
                move |_, _window, cx| {
                    let _ = view.update(cx, |this, cx| {
                        if this.drag == Some(ColorDrag::Square) {
                            cx.stop_propagation();
                            this.drag = None;
                            cx.notify();
                        }
                    });
                }
            })
            .child(
                canvas(
                    move |bounds, _, cx| {
                        let _ = square_prepaint_view.update(cx, |this, _| {
                            this.square_bounds = bounds;
                        });
                    },
                    move |bounds, _, window, _cx| {
                        const COLUMNS: usize = 34;
                        const ROWS: usize = 26;
                        for row in 0..ROWS {
                            for column in 0..COLUMNS {
                                let saturation = column as f32 / (COLUMNS - 1) as f32;
                                let value = 1.0 - row as f32 / (ROWS - 1) as f32;
                                let color = hsv_to_hsla(square_hue, saturation, value);
                                let x0 = bounds.origin.x
                                    + bounds.size.width * (column as f32 / COLUMNS as f32);
                                let y0 = bounds.origin.y
                                    + bounds.size.height * (row as f32 / ROWS as f32);
                                let x1 = bounds.origin.x
                                    + bounds.size.width * ((column + 1) as f32 / COLUMNS as f32);
                                let y1 = bounds.origin.y
                                    + bounds.size.height * ((row + 1) as f32 / ROWS as f32);
                                window.paint_quad(gpui::fill(
                                    gpui::Bounds::from_corners(point(x0, y0), point(x1, y1)),
                                    color,
                                ));
                            }
                        }
                    },
                )
                .absolute()
                .size_full(),
            )
            .child(
                div()
                    .absolute()
                    .left(px(marker_x))
                    .top(px(marker_y))
                    .w(px(12.0))
                    .h(px(12.0))
                    .rounded(px(6.0))
                    .border_2()
                    .border_color(gpui::Hsla::white())
                    .bg(hue_color.opacity(0.2))
                    .shadow_sm(),
            );

        let hue_view = view.clone();
        let hue_prepaint_view = view;
        let hue = self.hue;
        let hue_strip = div()
            .id(ElementId::Name(SharedString::from(format!(
                "color-hue-{}",
                self.pane_id
            ))))
            .relative()
            .w(px(SQUARE_WIDTH))
            .h(px(HUE_HEIGHT))
            .overflow_hidden()
            .border_1()
            .border_color(colors.border)
            .cursor_pointer()
            .on_mouse_down(MouseButton::Left, {
                let view = hue_view.clone();
                move |event: &MouseDownEvent, _window, cx| {
                    cx.stop_propagation();
                    let _ = view.update(cx, |this, cx| {
                        this.drag = Some(ColorDrag::Hue);
                        this.update_from_position(event.position, ColorDrag::Hue, cx);
                        cx.notify();
                    });
                }
            })
            .on_mouse_move({
                let view = hue_view.clone();
                move |event: &MouseMoveEvent, _window, cx| {
                    let _ = view.update(cx, |this, cx| {
                        if this.drag == Some(ColorDrag::Hue) {
                            cx.stop_propagation();
                            this.update_from_position(event.position, ColorDrag::Hue, cx);
                            cx.notify();
                        }
                    });
                }
            })
            .on_mouse_up(MouseButton::Left, {
                let view = hue_view;
                move |_, _window, cx| {
                    let _ = view.update(cx, |this, cx| {
                        if this.drag == Some(ColorDrag::Hue) {
                            cx.stop_propagation();
                            this.drag = None;
                            cx.notify();
                        }
                    });
                }
            })
            .child(
                canvas(
                    move |bounds, _, cx| {
                        let _ = hue_prepaint_view.update(cx, |this, _| {
                            this.hue_bounds = bounds;
                        });
                    },
                    move |bounds, _, window, _cx| {
                        const STEPS: usize = 36;
                        for step in 0..STEPS {
                            let hue = step as f32 / STEPS as f32;
                            let x0 = bounds.origin.x + bounds.size.width * hue;
                            let x1 = bounds.origin.x
                                + bounds.size.width * ((step + 1) as f32 / STEPS as f32);
                            window.paint_quad(gpui::fill(
                                gpui::Bounds::from_corners(
                                    point(x0, bounds.origin.y),
                                    point(x1, bounds.origin.y + bounds.size.height),
                                ),
                                hsv_to_hsla(hue, 1.0, 1.0),
                            ));
                        }
                    },
                )
                .absolute()
                .size_full(),
            )
            .child(
                div()
                    .absolute()
                    .left(px((hue * SQUARE_WIDTH - 2.0).clamp(0.0, SQUARE_WIDTH - 4.0)))
                    .top(px(1.0))
                    .w(px(4.0))
                    .h(px(HUE_HEIGHT - 2.0))
                    .rounded(px(2.0))
                    .border_1()
                    .border_color(gpui::Hsla::white())
                    .bg(gpui::Hsla::transparent_black()),
            );

        // Compact readout: the swatch and hex carry the "current colour" state,
        // the subtitle strip carries the "applies to new drawings" contract.
        let readout = h_flex()
            .h(px(SWATCH_SIZE))
            .gap(px(8.0))
            .items_center()
            .child(
                div()
                    .size(px(SWATCH_SIZE))
                    .flex_shrink_0()
                    .rounded(px(3.0))
                    .border_1()
                    .border_color(colors.foreground.opacity(0.35))
                    .bg(hex_to_hsla(&self.hex).unwrap_or(colors.foreground)),
            )
            .child(
                div()
                    .font_family(mono_font)
                    .text_size(px(11.0))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(self.hex.clone()),
            );

        v_flex()
            .gap(px(8.0))
            .child(square)
            .child(hue_strip)
            .child(readout)
            .child(self.render_palette(cx))
            .into_any_element()
    }
}

impl Focusable for ColorWindow {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ColorWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let available = self.find_chart(cx).is_some();
        let body = if available {
            v_flex()
                .flex_1()
                .min_h_0()
                .overflow_y_scrollbar()
                .px(px(12.0))
                .py(px(10.0))
                .child(self.render_picker(cx))
                .into_any_element()
        } else {
            self.render_closed(cx)
        };

        v_flex()
            .size_full()
            .bg(colors.background)
            .text_color(colors.foreground)
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(|_this, event: &KeyDownEvent, window, _cx| {
                if event.keystroke.key == "escape" {
                    window.remove_window();
                }
            }))
            .child(self.render_header(cx))
            // The "applies to new drawings" contract only makes sense while a
            // chart is bound; the closed state keeps just the title chrome.
            .when(available, |this| {
                this.child(
                    div()
                        .flex_shrink_0()
                        .h(px(26.0))
                        .px(px(12.0))
                        .flex()
                        .items_center()
                        .text_size(px(10.0))
                        .text_color(colors.muted_foreground)
                        .border_b_1()
                        .border_color(colors.border)
                        .child("Applies to new drawings"),
                )
            })
            .child(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_palette_swatch_is_a_valid_color() {
        for hex in PALETTE {
            assert!(hex_to_hsla(hex).is_some(), "{hex}");
        }
    }

    #[test]
    fn palette_has_no_duplicate_swatches() {
        for (index, hex) in PALETTE.iter().enumerate() {
            assert!(
                !PALETTE[..index]
                    .iter()
                    .any(|seen| seen.eq_ignore_ascii_case(hex)),
                "{hex} appears twice"
            );
        }
    }

    #[test]
    fn hsv_hex_round_trip_for_picker_values() {
        let hex = hsla_to_hex(hsv_to_hsla(0.33, 0.7, 0.8));
        let parsed = hex_to_hsla(&hex).expect("valid hex");
        let (hue, saturation, value) = hsla_to_hsv(parsed);
        assert!((hue - 0.33).abs() < 0.01);
        assert!((saturation - 0.7).abs() < 0.01);
        assert!((value - 0.8).abs() < 0.01);
    }
}
