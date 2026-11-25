//! Performance probe for the pure chart core.
//!
//! Run with:
//! ```text
//! cargo test --release chart_core_10k -- --ignored --nocapture
//! ```
//!
//! Set `CHART_PERF_ASSERT=1` to turn the p95 budget into a hard assertion.

use std::hint::black_box;
use std::time::Instant;

use gpui_component::chart::core::{
    build_envelope, compute, LayoutOptions, LodLevel, Ohlcv, PriceScale, PriceScaleMode, Rect,
    Viewport, ViewportLimits,
};

fn synthetic_bars(count: usize) -> Vec<Ohlcv> {
    let mut out = Vec::with_capacity(count);
    let mut price = 50_000.0_f64;
    for i in 0..count {
        let wave = ((i as f64) * 0.017).sin() * 120.0;
        let open = price;
        let close = open + wave * 0.08;
        let high = open.max(close) + 40.0 + (i % 7) as f64;
        let low = open.min(close) - 40.0 - (i % 5) as f64;
        out.push(Ohlcv::new(open, high, low, close, 10.0 + (i % 13) as f64));
        price = close;
    }
    out
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let index = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[index]
}

#[test]
#[ignore = "performance probe: run explicitly with --release --ignored"]
fn chart_core_10k_candles_6_indicators() {
    let bars = synthetic_bars(10_000);
    let plot = Rect::new(0.0, 0.0, 1_600.0, 800.0);
    let layout = compute(plot, 72.0, &[0.18, 0.18, 0.18], true, LayoutOptions::default());
    let limits = ViewportLimits::default();
    let mut viewport = Viewport::new(0.0, 1_500.0);
    let scale = PriceScale::new(PriceScaleMode::Linear, 0.0, 100_000.0, layout.plot);

    let mut samples = Vec::with_capacity(300);
    for i in 0..300 {
        let start = Instant::now();
        viewport.pan_pixels(((i % 41) as f32 - 20.0) * 3.0, layout.plot);
        viewport.clamp(bars.len(), &limits);
        let visible = viewport.visible(bars.len(), &limits);
        let lod = LodLevel::for_slot_width(viewport.slot_width(layout.plot));

        // Six indicator series: emulate the per-frame work of reading and
        // aggregating six precomputed arrays over the visible range.
        let mut accumulator = 0.0_f64;
        for series in 0..6 {
            for index in visible.clone() {
                accumulator += bars[index].close * (series as f64 + 1.0);
            }
        }
        let envelope = build_envelope(&bars, visible.clone(), &viewport, layout.plot);
        let ticks = scale.ticks(6);
        black_box(accumulator);
        black_box(visible);
        black_box(lod);
        black_box(envelope);
        black_box(ticks);
        samples.push(start.elapsed().as_secs_f64() * 1_000.0);
    }

    samples.sort_by(|a, b| a.total_cmp(b));
    let p50 = percentile(&samples, 0.50);
    let p95 = percentile(&samples, 0.95);
    let max = samples.last().copied().unwrap_or(0.0);
    println!(
        "chart core: 10k candles, 6 series, 1500 visible | p50={p50:.3} ms p95={p95:.3} ms max={max:.3} ms"
    );

    if std::env::var("CHART_PERF_ASSERT").ok().as_deref() == Some("1") {
        assert!(p95 < 2.0, "core frame work p95 {p95:.3} ms exceeds 2 ms");
    }
}
