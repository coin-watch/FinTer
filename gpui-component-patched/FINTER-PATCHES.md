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
path is experimental and does not yet render drawings, alerts, or position and
order overlays; those remain on the legacy path.

Promotion to a first-party `src/chart/engine` module is a follow-up. Until then,
changes to these files are fork patches and should be listed here.

## `src/chart/chart_state.rs`

Legacy viewport state used by `CandlestickChart`. The pure-core merge brought a
change that reserved three empty slots to the right of the newest candle and
removed the left blank margin. That alters the legacy (non-v2) viewport, so the
change was reverted in the merge commit; `clamp_offset` keeps its original
`total - 2` / `2 - visible_count` bounds. Re-landing it is a separate UX change.

Axis-drag additions:

- `chart_bounds` is now a shared `Rc<Cell<Option<Bounds<Pixels>>>>` captured
  during paint, so `ChartPanel` can hit-test the price and time rails against
  the rendered geometry (the chart receives a clone of `ChartState`).
- `resize_span(new_visible, total, cursor_frac)` sets an absolute visible span
  using the same cursor-anchor math as `zoom`, then clamps the offset and clears
  `pinned_to_latest`. The legacy time-axis drag uses it.
- `MIN_VISIBLE` / `MAX_VISIBLE` are re-exported from `src/chart/mod.rs` so
  `ChartPanel` can pass the same limits to `drag_time_span`.

## `src/chart/core/time.rs`

Pure additions: `timeframe_interval_ms()` maps a selected timeframe to an exact
uniform interval, and `timestamp_at_buffer_index()` resolves an in-range or
out-of-buffer candle slot to a timestamp. Past/future slots step the selected
timeframe (calendar months for `1M` via `chrono`), and unknown timeframes fall
back to a supplied median interval. Both the legacy X-axis labels and the
crosshair use these helpers for blank past/future slots; no synthetic candles
are drawn.

## `src/chart/candlestick_chart.rs`

- **Blank-time X labels.** The X-axis walks continuous candle-slot indices
  across the viewport, including slots before the first and after the last
  loaded candle. Labels use the existing formatter/colour, are clamped inside
  the plot, and replace the previous tick when a synthetic blank-future edge
  label would overlap it. No placeholder candles are drawn.
- **Crosshair into unformed time.** The crosshair snaps to the candle-slot cell
  under the cursor even when that slot has no loaded candle. In blank past or
  future space it keeps painting the hairlines, price badge, and X-axis
  timestamp pill using `timestamp_at_buffer_index`; `crosshair_index` stays
  `None` so the OHLC legend keeps falling back to the newest candle. The candle
  series itself still clamps to two candles at the buffer edges.
- **Subpane zero line.** `draw_lines` paints a faint zero baseline for any pane
  whose auto-scaled domain includes zero (for example histogram indicator
  panes). This is otherwise unchanged chart behaviour.
- **Optional manual price range.** A valid finite `manual_price_range` with
  `max > min` replaces the auto 5%-padded Y domain for ticks, the price scale,
  and `candle_price_range`; `None` or an invalid range keeps auto-fit. Paint
  also writes the shared `chart_bounds` cell so `ChartPanel` can hit-test the
  axis rails.

Manual checks on desktop where possible:

- Add a histogram indicator (for example MACD): its subpane draws a faint zero
  baseline and independently scaled bars.
- Pan, zoom, and toggle candle types; drawings, alerts, and position/order
  overlays still render.
