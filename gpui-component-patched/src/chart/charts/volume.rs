//! Volume pane marks.

use std::ops::Range;

use crate::chart::core::{Ohlcv, Rect, Viewport};
use crate::chart::scene::{Layer, Primitive, Scene};
use crate::chart::theme::ChartPalette;

/// Append volume bars for the visible range into a volume pane.
pub fn build_volume(
    scene: &mut Scene,
    bars: &[Ohlcv],
    visible: Range<usize>,
    viewport: &Viewport,
    pane: Rect,
    palette: &ChartPalette,
) {
    if bars.is_empty() || pane.is_empty() {
        return;
    }
    let start = visible.start.min(bars.len());
    let end = visible.end.min(bars.len());
    if start >= end {
        return;
    }
    let max_volume = bars[start..end]
        .iter()
        .map(|bar| {
            if bar.volume.is_finite() {
                bar.volume.max(0.0)
            } else {
                0.0
            }
        })
        .fold(0.0_f64, f64::max);
    if max_volume <= 0.0 {
        return;
    }
    let slot = viewport.slot_width(pane);
    if slot <= 0.0 {
        return;
    }
    let body_w = (slot * 0.72).clamp(1.0, 24.0);
    let usable_h = (pane.h - 1.0).max(1.0);
    for index in start..end {
        let bar = bars[index];
        if !bar.volume.is_finite() || bar.volume <= 0.0 {
            continue;
        }
        let x = viewport.index_to_x(index as f64, pane);
        if x < pane.x - slot || x > pane.right() + slot {
            continue;
        }
        let h = ((bar.volume / max_volume) as f32 * usable_h).max(1.0);
        let color = if bar.close >= bar.open {
            palette.volume_up
        } else {
            palette.volume_down
        };
        scene.push(
            Layer::Series,
            Primitive::Rect {
                rect: Rect::new(x - body_w * 0.5, pane.bottom() - h, body_w, h),
                color,
                radius: 0.0,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chart::core::Viewport;

    fn bars() -> Vec<Ohlcv> {
        vec![
            Ohlcv::new(10.0, 11.0, 9.0, 10.5, 100.0),
            Ohlcv::new(10.5, 12.0, 10.0, 11.0, 200.0),
            Ohlcv::new(11.0, 11.5, 10.5, 10.8, 50.0),
        ]
    }

    #[test]
    fn bars_fit_inside_the_pane() {
        let mut scene = Scene::new();
        let pane = Rect::new(0.0, 300.0, 300.0, 100.0);
        build_volume(
            &mut scene,
            &bars(),
            0..3,
            &Viewport::new(0.0, 3.0),
            pane,
            &ChartPalette::obsidian(),
        );
        assert_eq!(scene.layer(Layer::Series).len(), 3);
        for primitive in scene.layer(Layer::Series) {
            if let Primitive::Rect { rect, .. } = primitive {
                assert!(rect.y >= pane.y);
                assert!(rect.bottom() <= pane.bottom() + 0.01);
            }
        }
    }

    #[test]
    fn zero_volume_is_safe() {
        let mut scene = Scene::new();
        let mut bars = bars();
        for bar in &mut bars {
            bar.volume = 0.0;
        }
        build_volume(
            &mut scene,
            &bars,
            0..3,
            &Viewport::new(0.0, 3.0),
            Rect::new(0.0, 0.0, 300.0, 100.0),
            &ChartPalette::obsidian(),
        );
        assert!(scene.is_empty());
    }
}
