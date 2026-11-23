//! Candlestick marks with level-of-detail aggregation.

use std::ops::Range;

use gpui::Hsla;

use crate::chart::core::series::{LodLevel, Ohlcv, build_envelope};
use crate::chart::core::{Point, PriceScale, Rect, Viewport};
use crate::chart::scene::{Layer, Primitive, Scene};
use crate::chart::theme::ChartPalette;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CandleStyle {
    pub body_ratio: f32,
    pub min_body_px: f32,
    pub max_body_px: f32,
    pub wick_width: f32,
    pub up: Hsla,
    pub down: Hsla,
    pub up_hollow: bool,
    pub hover_halo: Hsla,
    pub wick_alpha: f32,
    pub label_size: f32,
}

impl CandleStyle {
    pub fn from_palette(palette: &ChartPalette) -> Self {
        Self {
            body_ratio: 0.72,
            min_body_px: 1.0,
            max_body_px: 24.0,
            wick_width: 1.0,
            up: palette.up,
            down: palette.down,
            up_hollow: true,
            hover_halo: palette.measure_stroke.opacity(0.20),
            wick_alpha: 0.70,
            label_size: 11.0,
        }
    }
}

/// Append candle marks for the visible range.
#[allow(clippy::too_many_arguments)]
pub fn build_candles(
    scene: &mut Scene,
    bars: &[Ohlcv],
    visible: Range<usize>,
    viewport: &Viewport,
    plot: Rect,
    scale: &PriceScale,
    style: &CandleStyle,
    hovered: Option<usize>,
    lod: LodLevel,
) {
    if bars.is_empty() || plot.is_empty() {
        return;
    }
    let start = visible.start.min(bars.len());
    let end = visible.end.min(bars.len());
    if start >= end {
        return;
    }
    if matches!(lod, LodLevel::Envelope | LodLevel::Density) {
        build_envelope_marks(scene, bars, start..end, viewport, plot, scale, style);
        return;
    }

    let slot = viewport.slot_width(plot);
    if slot <= 0.0 {
        return;
    }
    let thin = matches!(lod, LodLevel::ThinCandles);
    let body_w = (slot * style.body_ratio)
        .clamp(style.min_body_px, style.max_body_px)
        .min((slot - 1.0).max(1.0));
    let half = body_w * 0.5;
    let wick_color_up = style.up.opacity(style.wick_alpha);
    let wick_color_down = style.down.opacity(style.wick_alpha);
    let mut wick_up: Vec<(Point, Point)> = Vec::new();
    let mut wick_down: Vec<(Point, Point)> = Vec::new();
    let mut halos: Vec<Primitive> = Vec::new();
    let mut bodies: Vec<Primitive> = Vec::new();

    for index in start..end {
        let bar = bars[index];
        if !bar.open.is_finite()
            || !bar.high.is_finite()
            || !bar.low.is_finite()
            || !bar.close.is_finite()
        {
            continue;
        }
        let x = viewport.index_to_x(index as f64, plot);
        if x < plot.x - slot || x > plot.right() + slot {
            continue;
        }
        let open_y = scale.to_y(bar.open);
        let close_y = scale.to_y(bar.close);
        let high_y = scale.to_y(bar.high);
        let low_y = scale.to_y(bar.low);
        if !(open_y.is_finite() && close_y.is_finite() && high_y.is_finite() && low_y.is_finite()) {
            continue;
        }
        let bullish = bar.close >= bar.open;
        let color = if bullish { style.up } else { style.down };

        let wick_from = Point::new(x, high_y.min(low_y));
        let wick_to = Point::new(x, high_y.max(low_y));
        if bullish {
            wick_up.push((wick_from, wick_to));
        } else {
            wick_down.push((wick_from, wick_to));
        }

        let body_top = open_y.min(close_y);
        let body_bottom = open_y.max(close_y);
        let body_h = (body_bottom - body_top).max(1.0);
        let rect = Rect::new(x - half, body_top, body_w, body_h);

        if hovered == Some(index) {
            halos.push(Primitive::Rect {
                rect: Rect::new(rect.x - 2.0, rect.y - 2.0, rect.w + 4.0, rect.h + 4.0),
                color: style.hover_halo,
                radius: 2.0,
            });
        }

        if style.up_hollow && bullish && !thin {
            let points = vec![
                Point::new(rect.x, rect.y),
                Point::new(rect.right(), rect.y),
                Point::new(rect.right(), rect.bottom()),
                Point::new(rect.x, rect.bottom()),
                Point::new(rect.x, rect.y),
            ];
            bodies.push(Primitive::Polyline {
                points,
                width: 1.5,
                color,
                closed: true,
                fill: None,
            });
        } else {
            bodies.push(Primitive::Rect {
                rect,
                color,
                radius: 0.0,
            });
        }
    }

    scene.extend(Layer::Series, halos);
    if !wick_up.is_empty() {
        scene.push(
            Layer::Series,
            Primitive::Segments {
                segments: wick_up,
                width: style.wick_width,
                color: wick_color_up,
                dash: None,
            },
        );
    }
    if !wick_down.is_empty() {
        scene.push(
            Layer::Series,
            Primitive::Segments {
                segments: wick_down,
                width: style.wick_width,
                color: wick_color_down,
                dash: None,
            },
        );
    }
    scene.extend(Layer::Series, bodies);
}

fn build_envelope_marks(
    scene: &mut Scene,
    bars: &[Ohlcv],
    visible: Range<usize>,
    viewport: &Viewport,
    plot: Rect,
    scale: &PriceScale,
    style: &CandleStyle,
) {
    let envelope = build_envelope(bars, visible, viewport, plot);
    if envelope.is_empty() {
        return;
    }
    let mut up: Vec<(Point, Point)> = Vec::new();
    let mut down: Vec<(Point, Point)> = Vec::new();
    for bar in envelope {
        let x = bar.x;
        let y0 = scale.to_y(bar.high).min(scale.to_y(bar.low));
        let y1 = scale.to_y(bar.high).max(scale.to_y(bar.low));
        if !(x.is_finite() && y0.is_finite() && y1.is_finite()) {
            continue;
        }
        let segment = (Point::new(x, y0), Point::new(x, y1));
        if bar.close >= bar.open {
            up.push(segment);
        } else {
            down.push(segment);
        }
    }
    if !up.is_empty() {
        scene.push(
            Layer::Series,
            Primitive::Segments {
                segments: up,
                width: 1.0,
                color: style.up.opacity(0.85),
                dash: None,
            },
        );
    }
    if !down.is_empty() {
        scene.push(
            Layer::Series,
            Primitive::Segments {
                segments: down,
                width: 1.0,
                color: style.down.opacity(0.85),
                dash: None,
            },
        );
    }
}

/// Small helper for callers that want to render a close marker on the
/// envelope without duplicating color logic.
pub fn envelope_close_dot(
    x: f32,
    price: f64,
    scale: &PriceScale,
    bullish: bool,
    palette: &ChartPalette,
) -> Primitive {
    Primitive::Dot {
        center: Point::new(x, scale.to_y(price)),
        radius: 1.0,
        color: if bullish { palette.up } else { palette.down },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chart::core::PriceScaleMode;

    fn bars() -> Vec<Ohlcv> {
        vec![
            Ohlcv::new(10.0, 12.0, 9.0, 11.0, 100.0),
            Ohlcv::new(11.0, 13.0, 10.0, 10.5, 120.0),
            Ohlcv::new(10.5, 12.5, 9.5, 12.0, 90.0),
        ]
    }

    fn setup() -> (Scene, Viewport, Rect, PriceScale, CandleStyle) {
        (
            Scene::new(),
            Viewport::new(0.0, 3.0),
            Rect::new(0.0, 0.0, 300.0, 200.0),
            PriceScale::new(
                PriceScaleMode::Linear,
                9.0,
                13.0,
                Rect::new(0.0, 0.0, 300.0, 200.0),
            ),
            CandleStyle::from_palette(&ChartPalette::obsidian()),
        )
    }

    #[test]
    fn builds_bodies_and_wicks() {
        let (mut scene, viewport, plot, scale, style) = setup();
        let bars = bars();
        build_candles(
            &mut scene,
            &bars,
            0..bars.len(),
            &viewport,
            plot,
            &scale,
            &style,
            None,
            LodLevel::Candles,
        );
        assert!(scene.primitive_count() >= 4);
        assert!(
            scene
                .layer(Layer::Series)
                .iter()
                .any(|p| matches!(p, Primitive::Segments { .. }))
        );
    }

    #[test]
    fn bullish_candles_are_hollow_by_default() {
        let (mut scene, viewport, plot, scale, style) = setup();
        let bars = bars();
        build_candles(
            &mut scene,
            &bars,
            0..bars.len(),
            &viewport,
            plot,
            &scale,
            &style,
            None,
            LodLevel::Candles,
        );
        let hollow = scene
            .layer(Layer::Series)
            .iter()
            .filter(|p| matches!(p, Primitive::Polyline { fill: None, .. }))
            .count();
        assert!(hollow >= 1);
    }

    #[test]
    fn hover_adds_a_halo() {
        let (mut scene, viewport, plot, scale, style) = setup();
        let bars = bars();
        build_candles(
            &mut scene,
            &bars,
            0..bars.len(),
            &viewport,
            plot,
            &scale,
            &style,
            Some(1),
            LodLevel::Candles,
        );
        let halos = scene
            .layer(Layer::Series)
            .iter()
            .filter(|p| matches!(p, Primitive::Rect { radius, .. } if *radius > 0.0))
            .count();
        assert_eq!(halos, 1);
    }

    #[test]
    fn envelope_lod_uses_batched_segments() {
        let (mut scene, viewport, plot, scale, style) = setup();
        let bars = bars();
        build_candles(
            &mut scene,
            &bars,
            0..bars.len(),
            &viewport,
            plot,
            &scale,
            &style,
            None,
            LodLevel::Envelope,
        );
        let segments = scene
            .layer(Layer::Series)
            .iter()
            .filter(|p| matches!(p, Primitive::Segments { .. }))
            .count();
        assert!((1..=2).contains(&segments));
    }

    #[test]
    fn empty_range_is_safe() {
        let (mut scene, viewport, plot, scale, style) = setup();
        let bars = bars();
        build_candles(
            &mut scene,
            &bars,
            0..0,
            &viewport,
            plot,
            &scale,
            &style,
            None,
            LodLevel::Candles,
        );
        assert!(scene.is_empty());
    }
}
