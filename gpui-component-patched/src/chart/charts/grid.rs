//! Grid marks.

use crate::chart::core::{Point, PriceScale, Rect};
use crate::chart::scene::{Layer, Primitive, Scene};
use crate::chart::theme::ChartPalette;

/// Horizontal price grid. Vertical lines are intentionally omitted by default:
/// they add noise to a time series without helping read price.
pub fn build_horizontal_grid(
    scene: &mut Scene,
    plot: Rect,
    scale: &PriceScale,
    palette: &ChartPalette,
    target_ticks: usize,
) {
    if plot.is_empty() {
        return;
    }
    let ticks = scale.ticks(target_ticks);
    let mut segments = Vec::with_capacity(ticks.len());
    for tick in &ticks {
        let y = scale.to_y(tick.value);
        if !y.is_finite() || y < plot.y || y > plot.bottom() {
            continue;
        }
        segments.push((Point::new(plot.x, y), Point::new(plot.right(), y)));
    }
    if segments.is_empty() {
        return;
    }
    scene.push(
        Layer::Grid,
        Primitive::Segments {
            segments,
            width: 1.0,
            color: palette.grid_minor,
            dash: Some([3.0, 3.0]),
        },
    );
}

/// Vertical grid, available for users who explicitly enable it.
pub fn build_vertical_grid(scene: &mut Scene, plot: Rect, xs: &[f32], palette: &ChartPalette) {
    if plot.is_empty() {
        return;
    }
    let segments: Vec<(Point, Point)> = xs
        .iter()
        .copied()
        .filter(|x| x.is_finite() && *x >= plot.x && *x <= plot.right())
        .map(|x| (Point::new(x, plot.y), Point::new(x, plot.bottom())))
        .collect();
    if segments.is_empty() {
        return;
    }
    scene.push(
        Layer::Grid,
        Primitive::Segments {
            segments,
            width: 1.0,
            color: palette.grid_minor,
            dash: Some([2.0, 4.0]),
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chart::core::PriceScaleMode;

    #[test]
    fn horizontal_grid_culls_ticks_outside_plot() {
        let mut scene = Scene::new();
        let plot = Rect::new(0.0, 50.0, 300.0, 100.0);
        let scale = PriceScale::new(PriceScaleMode::Linear, 0.0, 100.0, plot);
        build_horizontal_grid(&mut scene, plot, &scale, &ChartPalette::obsidian(), 5);
        let grid = scene.layer(Layer::Grid);
        assert_eq!(grid.len(), 1);
        if let Primitive::Segments { segments, .. } = &grid[0] {
            assert!(segments.iter().all(|(_, to)| to.y >= 50.0 && to.y <= 150.0));
        } else {
            panic!("expected grid segments");
        }
    }

    #[test]
    fn vertical_grid_is_opt_in_and_filtered() {
        let mut scene = Scene::new();
        let plot = Rect::new(0.0, 0.0, 100.0, 100.0);
        build_vertical_grid(
            &mut scene,
            plot,
            &[-10.0, 0.0, 50.0, 200.0],
            &ChartPalette::obsidian(),
        );
        let grid = scene.layer(Layer::Grid);
        assert_eq!(grid.len(), 1);
        if let Primitive::Segments { segments, .. } = &grid[0] {
            assert_eq!(segments.len(), 2);
        }
    }
}
