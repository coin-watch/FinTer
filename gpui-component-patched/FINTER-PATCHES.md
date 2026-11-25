# FinTer GPUI Component patches

This directory vendors the published `gpui-component` 0.5.1 crate
(Apache-2.0). The root `Cargo.toml` selects it through `[patch.crates-io]`.

Keep changes minimal and re-check them when upgrading the dependency.

## `src/chart/{core,scene,charts,elements,theme}.rs` — pure chart engine (opt-in)

The scene-based chart engine currently lives in this vendored crate because the
`ChartView` element must be an `gpui_component` element: it consumes the crate's
theme, text cache, and element lifecycle, and the fork already owns the chart
widget surface. Moving it to `src/chart/engine` would require either exposing
those internals or reimplementing them in FinTer, so it is kept here for now.

- `core/` holds pure geometry, time, scale, viewport, layout, series, hit-test
  and gesture logic with no GPUI dependency.
- `scene/` holds renderer-agnostic primitives, layers, painter accumulation and
  the text cache.
- `charts/` holds the axes, grid, candle and volume painters built on `scene`.
- `elements/chart_view.rs` is the GPUI element that owns a `ChartSnapshot` and
  options and emits `ChartEvent`s.
- `theme.rs` maps the active `gpui_component` theme colours to a `ChartPalette`.

FinTer opts in with `FINTER_CHART_V2=1`. When unset, the legacy
`CandlestickChart` path runs unchanged, including its viewport clamping. The v2
path is experimental and does not yet render drawings, alerts, footprints or
L1/L2 overlays; those remain on the legacy path.

Promotion to a first-party `src/chart/engine` module is a follow-up. Until then,
changes to these files are fork patches and should be listed here.

## `src/chart/chart_state.rs`

Legacy viewport state used by `CandlestickChart`. The pure-core merge brought a
change that reserved three empty slots to the right of the newest candle and
removed the left blank margin. That alters the legacy (non-v2) viewport, so the
change was reverted in the merge commit; `clamp_offset` keeps its original
`total - 2` / `2 - visible_count` bounds. Re-landing it is a separate UX change.

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
