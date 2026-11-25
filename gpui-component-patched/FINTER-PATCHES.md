# FinTer GPUI Component patches

This directory vendors the published `gpui-component` 0.5.1 crate
(Apache-2.0). The root `Cargo.toml` selects it through `[patch.crates-io]`.

Keep changes minimal and re-check them when upgrading the dependency.

## `src/chart/candlestick_chart.rs`

Order-flow rendering fixes only; non-order-flow chart behaviour is unchanged.

- **Subpane zero line.** `draw_lines` paints a faint zero baseline for any pane
  whose auto-scaled domain includes zero (all histogram panes, and CVD panes
  that turn negative). The existing 1.5px CVD line width is preserved.
- **Footprint sub-pixel rows.** Rows shorter than 0.5px are skipped unless they
  carry POC/VA/hover/selection markers. Zoom changes text painting and this
  overdraw guard, not the row aggregation (which is done in FinTer's
  `src/chart/footprint.rs`).
- **Footprint text fit.** The conservative character-width estimate is
  `font_size * 0.62` and the two-column/one-column tests reserve horizontal
  padding, so bid/ask numbers cannot overflow a narrow cell. Cells too short
  for text still keep their background and markers.
- **Ask/bid stripe mapping.** `imbalance > 0` is now documented and painted as
  ask-side / aggressive buying: stripe on the right in the bullish colour;
  `imbalance < 0` is bid-side / aggressive selling on the left in the bearish
  colour.
- **Heatmap clipping.** Fully off-screen heatmap cells are skipped before
  clamping. Only partially visible cells are clipped to the plot, so panning no
  longer smears a liquidity band along the edge.

Manual checks on desktop where possible:

- Enable Delta/CVD: two subpanes, independent scales, faint zero lines, a
  legend row and crosshair value in each pane.
- Enable Footprint: pan/zoom repeatedly; a given print stays in the same price
  row, text never overflows, and POC/VA/imbalance markers remain visible.
- Enable Liquidity: pan left and right; no persistent band at either plot edge.
