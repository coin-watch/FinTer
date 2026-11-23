//! Axis marks: price labels, tick marks, and time labels.

use gpui::{FontWeight, SharedString};

use crate::chart::core::{Point, PriceScale, Rect};
use crate::chart::scene::{Layer, Primitive, Scene};
use crate::chart::theme::ChartPalette;

pub const AXIS_FONT_SIZE: f32 = 11.0;
pub const TIME_FONT_SIZE: f32 = 10.0;
pub const TICK_LEN: f32 = 4.0;

/// Append price-axis ticks and labels.
pub fn build_price_axis(
    scene: &mut Scene,
    plot: Rect,
    scale: &PriceScale,
    palette: &ChartPalette,
    target_ticks: usize,
    formatter: impl Fn(f64) -> SharedString,
) {
    if plot.is_empty() {
        return;
    }
    let ticks = scale.ticks(target_ticks);
    let mut tick_segments = Vec::with_capacity(ticks.len());
    for tick in &ticks {
        let y = scale.to_y(tick.value);
        if !y.is_finite() || y < plot.y || y > plot.bottom() {
            continue;
        }
        let label_y = y - AXIS_FONT_SIZE * 0.5;
        tick_segments.push((
            Point::new(plot.right(), y),
            Point::new(plot.right() + TICK_LEN, y),
        ));
        scene.push(
            Layer::Chrome,
            Primitive::Glyph {
                text: formatter(tick.value),
                origin: Point::new(plot.right() + TICK_LEN + 4.0, label_y),
                font_size: AXIS_FONT_SIZE,
                weight: FontWeight::NORMAL,
                color: palette.axis_text,
                align: gpui::TextAlign::Left,
            },
        );
    }
    if !tick_segments.is_empty() {
        scene.push(
            Layer::Chrome,
            Primitive::Segments {
                segments: tick_segments,
                width: 1.0,
                color: palette.axis_text.opacity(0.30),
                dash: None,
            },
        );
    }
}

/// A pre-formatted time-axis tick.
#[derive(Clone, Debug, PartialEq)]
pub struct TimeTick {
    pub x: f32,
    pub label: SharedString,
    /// Measured width from the text cache, used for collision avoidance.
    pub width: f32,
    /// Day labels are emphasized.
    pub major: bool,
}

/// Append time-axis ticks and labels.
pub fn build_time_axis(
    scene: &mut Scene,
    layout: &crate::chart::core::ChartLayout,
    ticks: &[TimeTick],
    palette: &ChartPalette,
) {
    let plot = layout.plot;
    if plot.is_empty() || layout.time_rail.h <= 0.0 {
        return;
    }
    let mut seen: Vec<(f32, f32)> = Vec::new();
    for tick in ticks {
        if !tick.x.is_finite() || tick.x < plot.x || tick.x > plot.right() {
            continue;
        }
        // Collision check: reserve the measured width plus a 10 px gutter.
        let width = tick.width.max(0.0);
        let left = tick.x - width * 0.5;
        let right = tick.x + width * 0.5;
        if seen.iter().any(|(other_left, other_right)| {
            left < *other_right + 10.0 && right > *other_left - 10.0
        }) {
            continue;
        }
        seen.push((left, right));
        let y = plot.bottom() + 3.0;
        scene.push(
            Layer::Chrome,
            Primitive::Glyph {
                text: tick.label.clone(),
                origin: Point::new(tick.x, y),
                font_size: TIME_FONT_SIZE,
                weight: if tick.major {
                    FontWeight::MEDIUM
                } else {
                    FontWeight::NORMAL
                },
                color: if tick.major {
                    palette.axis_text_active
                } else {
                    palette.axis_text
                },
                align: gpui::TextAlign::Center,
            },
        );
        scene.push(
            Layer::Chrome,
            Primitive::Segment {
                from: Point::new(tick.x, plot.bottom()),
                to: Point::new(tick.x, plot.bottom() + 3.0),
                width: 1.0,
                color: palette.axis_text.opacity(0.30),
                dash: None,
            },
        );
    }
}

/// A horizontal rail separator.
pub fn build_rail_separator(
    scene: &mut Scene,
    layout: &crate::chart::core::ChartLayout,
    palette: &ChartPalette,
) {
    if layout.plot.is_empty() {
        return;
    }
    if layout.price_rail.w > 0.0 {
        scene.push(
            Layer::Chrome,
            Primitive::Segment {
                from: Point::new(layout.plot.right(), layout.bounds.y),
                to: Point::new(layout.plot.right(), layout.plot.bottom()),
                width: 1.0,
                color: palette.grid_major,
                dash: None,
            },
        );
    }
    if layout.time_rail.h > 0.0 {
        scene.push(
            Layer::Chrome,
            Primitive::Segment {
                from: Point::new(layout.plot.x, layout.plot.bottom()),
                to: Point::new(layout.plot.right(), layout.plot.bottom()),
                width: 1.0,
                color: palette.grid_major,
                dash: None,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chart::core::PriceScaleMode;
    use crate::chart::core::layout::{LayoutOptions, compute};
    use gpui::SharedString;

    fn layout() -> crate::chart::core::ChartLayout {
        compute(
            Rect::new(0.0, 0.0, 400.0, 300.0),
            60.0,
            &[],
            false,
            LayoutOptions::default(),
        )
    }

    #[test]
    fn price_axis_emits_labels_and_ticks() {
        let mut scene = Scene::new();
        let layout = layout();
        let scale = PriceScale::new(PriceScaleMode::Linear, 0.0, 100.0, layout.plot);
        build_price_axis(
            &mut scene,
            layout.plot,
            &scale,
            &ChartPalette::obsidian(),
            5,
            |value| SharedString::from(format!("{value:.0}")),
        );
        let glyphs = scene
            .layer(Layer::Chrome)
            .iter()
            .filter(|p| matches!(p, Primitive::Glyph { .. }))
            .count();
        assert!(glyphs >= 3);
    }

    #[test]
    fn time_axis_skips_colliding_labels() {
        let mut scene = Scene::new();
        let layout = layout();
        let ticks = vec![
            TimeTick {
                x: 20.0,
                label: "09:30".into(),
                width: 28.0,
                major: false,
            },
            TimeTick {
                x: 24.0,
                label: "09:31".into(),
                width: 28.0,
                major: false,
            },
            TimeTick {
                x: 200.0,
                label: "10:00".into(),
                width: 28.0,
                major: false,
            },
        ];
        build_time_axis(&mut scene, &layout, &ticks, &ChartPalette::obsidian());
        let glyphs = scene
            .layer(Layer::Chrome)
            .iter()
            .filter(|p| matches!(p, Primitive::Glyph { .. }))
            .count();
        assert_eq!(glyphs, 2);
    }

    #[test]
    fn empty_plot_is_safe() {
        let mut scene = Scene::new();
        let layout = crate::chart::core::ChartLayout::empty(Rect::ZERO);
        build_time_axis(&mut scene, &layout, &[], &ChartPalette::obsidian());
        build_rail_separator(&mut scene, &layout, &ChartPalette::obsidian());
        assert!(scene.is_empty());
    }
}
