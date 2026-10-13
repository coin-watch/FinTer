# PRD: CWN Native Desktop Chart Module Improvements

## 1. Summary

The current CWN chart module is already capable: it renders live OHLCV candlesticks, crosshair tooltip data, viewport pan/zoom, last-price markers, volume, Heikin Ashi, horizontal grid toggles, and a broad indicator library ported from CoinWatch's TypeScript `klinechart` engine.

The next improvement phase should turn this from a functional chart into a professional trading workspace: predictable interaction, persistent layouts, drawing tools, better indicator management, performance guardrails, and a clearer separation between chart engine, chart state, data feed, and GPUI presentation.

This PRD defines the product goals, scope, milestones, acceptance criteria, and technical plan for improving the current chart module in the CWN native desktop app.

## 2. Current State

### 2.1 Primary files

- `src/components/ui/chart_panel.rs`
  - GPUI panel and UI composition.
  - Owns `ChartPanel` state, mouse interactions, indicator picker/settings UI, pane resizing, legend overlays, and chart renderer configuration.
  - Currently ~1,400 LOC.
- `src/klinechart.rs`
  - Native Rust technical-analysis and chart math module.
  - Contains indicator calculations, overlay shape models, Fibonacci/risk-reward helpers, coordinate transforms, timeframe parsing, and formatting helpers.
  - Currently ~950 LOC.
- `src/data/candle_feed.rs`
  - Live OHLCV data buffer consumed by the chart.
  - Merges history and streaming candle updates by timestamp.
  - Keeps candles sorted and capped at `MAX_CANDLES = 10_000`.
- `gpui-component-patched/src/chart/candlestick_chart.rs`
  - Low-level candlestick renderer.
  - Handles candle, indicator, pane, grid, crosshair, and axis painting.
  - Currently ~1,800 LOC.
- `gpui-component-patched/src/chart/chart_state.rs`
  - Shared viewport and interaction state passed into the renderer.

### 2.2 Current capabilities

- Live WebSocket-driven candle updates through `CandleFeed`.
- Initial history and older-history prepend support.
- Pan and scroll-wheel zoom.
- Crosshair and active OHLCV legend.
- Last-price line and visible-period high/low markers.
- Volume sub-pane.
- Indicator library with 28 technical indicators available in Rust.
- Main-pane and oscillator/sub-pane indicator routing.
- Independent sub-pane resizing.
- Indicator legend hover and close actions.
- Solid and Heikin Ashi candle rendering.
- Exchange-aware price formatting through `PrecisionInfo`.
- Timezone-aware axis and crosshair timestamps.

### 2.3 Current gaps / risks

- `ChartPanel` mixes product state, GPUI interaction handling, indicator computation, settings UI, and renderer wiring in one large file.
- `CandlestickChart` is a patched dependency module but is being used as part of product-specific chart behavior.
- `OverlayShape` models exist in `src/klinechart.rs`, but drawing overlays are not wired into `ChartPanel` or `CandlestickChart`.
- Indicator settings are mostly add/remove and visibility oriented; parameter editing and persistence are not yet first-class.
- Viewport, selected indicators, pane sizes, candle type, grid visibility, timezone, and drawings reset between app sessions.
- Performance is vulnerable to repeated full indicator recomputation during render, especially as indicator count and candle count increase.
- Debug output still exists in render flow for Heikin Ashi transformation.
- Testing appears stronger for isolated math than for interaction behavior, persistence, and renderer state invariants.

## 3. Goals

### 3.1 Product goals

1. Make CWN charts feel like a native professional trading terminal.
2. Preserve smooth interaction under live updates, multiple indicators, and long candle histories.
3. Let users customize and persist their chart workspace.
4. Add practical drawing and annotation tools for market analysis.
5. Reduce future chart feature cost by separating chart logic from GPUI rendering code.

### 3.2 User goals

Users should be able to:

- Open a market chart and quickly understand price, volume, trend, and momentum.
- Pan, zoom, inspect, and return to latest price without losing context.
- Add indicators, configure parameters, and keep those settings across sessions.
- Draw trendlines, price levels, Fibonacci retracements, and risk/reward plans.
- Save a chart layout per symbol/timeframe or as reusable presets.
- Trust that live updates do not reset their view or freeze the UI.

## 4. Non-goals

- Replacing GPUI or the existing patched chart renderer in the first phase.
- Building a full Pine Script-compatible strategy engine.
- Implementing broker order placement from chart drawings.
- Cloud sync of layouts in the first implementation phase.
- Full parity with every TradingView drawing or study tool.

## 5. Target Users

### 5.1 Active trader

Needs responsive chart interaction, multiple indicators, and reliable latest-price behavior.

### 5.2 Market researcher

Needs persistent layouts, annotations, trendlines, and multi-timeframe analysis.

### 5.3 Power user / developer

Needs predictable keyboard shortcuts, stable performance, and clear extension points for new indicators or overlays.

## 6. Scope

### 6.1 Phase 1: Stabilize and simplify chart architecture

#### Requirements

- Split chart responsibilities into focused modules:
  - `chart_panel.rs`: panel shell and GPUI layout only.
  - `chart_interaction.rs`: mouse, wheel, keyboard, viewport commands.
  - `chart_indicators.rs`: indicator selection, parameter models, computed series building.
  - `chart_persistence.rs`: serializable chart layout state.
  - `chart_drawings.rs`: drawing model and editing state.
- Remove debug print statements from render paths.
- Add internal chart state model that can be serialized without GPUI types.
- Keep current visible behavior unchanged during refactor.

#### Acceptance criteria

- Existing chart renders the same candle, volume, crosshair, and indicator views after the refactor.
- No render-path `println!` debugging remains.
- The chart compiles without increasing dependency count.
- Core interaction state is testable without constructing a GPUI window.

### 6.2 Phase 2: Persistent chart layouts

#### Requirements

Persist the following per chart context:

- Symbol and exchange.
- Timeframe.
- Candle type.
- Show/hide grid.
- Enabled indicators.
- Indicator parameters.
- Indicator pane assignment and pane heights.
- Main series visibility.
- Timezone preference.
- Drawing overlays.

Support two persistence modes:

1. Per-symbol/timeframe saved state.
2. Named layout presets reusable across markets.

#### Acceptance criteria

- Closing and reopening the app restores the last chart layout.
- Switching symbol/timeframe restores that pair's last saved layout when available.
- User can reset a chart to defaults.
- Corrupt layout data does not crash the app; it falls back to defaults and logs a warning.

### 6.3 Phase 3: Indicator parameter editing & Standalone Indicator Library Window

#### Requirements

- **Standalone Free-Floating Indicator Library Window**:
  - The Indicator Library can be opened as an independent floating window (`gpui::deferred`), detached from pane clipping.
  - Features: draggable title bar, close button, real-time search/filter input, category tabs (Trend, Momentum, Volatility, Volume), and an "Active on Chart" quick-management section.
  - Adding/removing indicators happens interactively without closing the window.
  - Window position and size persist across sessions.
  - Multi-chart aware: displays target chart title and syncs selections directly.
- Add parameter schemas for every supported indicator.
- Allow editing parameters through the indicator settings panel or directly within the standalone window.
- Validate parameters before applying them.
- Recompute only affected indicator series after a parameter change.
- Show clear labels for indicator instances, e.g. `EMA 20`, `EMA 50`, `RSI 14`.
- Support multiple instances of the same indicator with different parameters.

#### Acceptance criteria

- User can open the Indicator Library as a floating window that moves freely over the workspace.
- User can search for indicators by name/acronym (e.g. "RSI", "Bollinger").
- User can add at least two EMA instances with different periods.
- User can edit MACD short/long/signal periods and see the result update immediately.
- Invalid values are rejected with inline feedback.
- Indicator settings and library window geometry persist across restart.

### 6.4 Phase 4: Drawing and annotation tools

#### Requirements

Implement initial drawing tools from existing `OverlayShape` models:

- Horizontal price line.
- Vertical time marker.
- Trend line.
- Ray line.
- Segment.
- Fibonacci retracement.
- Price channel.
- Text annotation.
- Long/short position risk-reward tool.

Interactions:

- Tool selection toolbar.
- Click/drag placement.
- Select existing drawing.
- Move drawing points.
- Delete selected drawing.
- Escape cancels active drawing.
- Snap option for candle high/low/close.

#### Acceptance criteria

- Drawings remain anchored to timestamp and price during pan/zoom.
- Drawings persist with chart layout.
- User can create, select, move, and delete a trendline.
- Fibonacci levels use existing `fibonacci_levels()` helper.
- Long/short position tool displays risk/reward ratio using existing `risk_reward_ratio()` helper.

### 6.5 Phase 5: Performance and data efficiency

#### Requirements

- Avoid full indicator recomputation on every render when candles and indicator parameters have not changed.
- Cache computed indicator series by:
  - Candle buffer version.
  - Indicator kind.
  - Indicator parameters.
- Keep render path allocation predictable.
- Keep chart UI responsive with 10,000 candles and at least 6 active indicators.
- Track candle feed updates with a monotonic version counter instead of only timestamp polling.
- Reduce unnecessary full `Vec<OhlcvCandle>` clones where possible.

#### Acceptance criteria

- With 10,000 candles and 6 indicators, pan/zoom remains visually smooth on target hardware.
- Adding/removing an indicator does not block the UI for more than 100 ms on target hardware.
- Live candle updates do not trigger recomputation of unchanged indicator sets more than necessary.
- Add benchmark or measurement harness for indicator computation and chart-state operations.

### 6.6 Phase 6: UX polish and chart controls

#### Requirements

- Add a compact chart toolbar containing:
  - Candle type.
  - Indicators.
  - Drawings.
  - Grid toggle.
  - Auto-scroll/latest toggle.
  - Reset view.
  - Layout preset selector.
- Add keyboard shortcuts:
  - `Esc`: cancel drawing or close floating panel.
  - `Delete`: delete selected drawing.
  - `F`: fit visible data.
  - `L`: jump/pin to latest.
  - `G`: toggle grid.
  - `/`: open command/search palette for chart actions.
- Improve empty/loading/error states with actionable messages.
- Add small status indicators for live, reconnecting, stale, and error states.

#### Acceptance criteria

- User can discover indicators and drawings without opening nested panels.
- User can jump back to the latest candle after panning away.
- Keyboard shortcuts work only when chart panel is focused.
- Network or data errors show a visible state without clearing the last good candles.

## 7. Functional Requirements

### 7.1 Chart state

- The chart must preserve viewport when historical candles are prepended.
- The chart must preserve cursor-anchored zoom behavior.
- The chart must not auto-scroll to latest after the user manually pans away.
- The chart must provide an explicit command to re-pin to latest.
- The chart must keep pane sizes stable across live updates.

### 7.2 Indicator management

- Indicators must be represented by stable instance IDs, not only enum kind.
- Each indicator instance must own parameters and visual style.
- Indicators must support main-pane and sub-pane placement.
- Indicator legend must expose configure, hide, and remove actions.
- Multiple instances of the same indicator must be supported.

### 7.3 Drawings

- Drawings must be stored in data coordinates, not screen coordinates.
- Drawings must be serializable.
- Drawing hit-testing must use screen coordinates computed from the current viewport.
- Drawing tools must support cancel, undo last point, and delete.
- Drawings must support basic style fields: color, width, line style, label visibility.

### 7.4 Persistence

- Layout save operations should be debounced.
- Layout schema must be versioned.
- New versions must migrate or safely ignore unknown fields.
- Failed persistence must not block chart rendering.

### 7.5 Data and live updates

- The chart must display the most recent in-progress candle update.
- Candle merges must stay sorted by timestamp.
- Duplicate timestamps must update existing candles instead of adding duplicates.
- Invalid candles must be filtered before rendering.
- Data errors must be surfaced without discarding the last valid chart data.

## 8. Non-functional Requirements

### 8.1 Performance

- Render-path work should be bounded and avoid unnecessary full-series recomputation.
- Interaction updates should feel immediate at 60 FPS where hardware allows.
- The chart should handle 10,000 candles and 6 indicators without obvious UI stalls.
- Expensive computations should be cached or moved outside direct GPUI render composition.

### 8.2 Reliability

- No panics from empty candles, missing precision, missing sub-pane ratios, or corrupt layout data.
- Renderer and panel state should tolerate partial data during startup and reconnects.
- Chart should keep last good candles visible during data interruptions.

### 8.3 Maintainability

- Product chart logic should live in `src/` modules, not be buried inside patched `gpui-component` files unless it is truly generic renderer functionality.
- Indicator math should remain unit-testable without GPUI.
- Interaction math should remain unit-testable without GPUI.
- Renderer-specific code should be isolated behind a small adapter layer.

## 9. Proposed Technical Design

### 9.1 Module split

Proposed module layout:

```text
src/components/ui/chart_panel.rs
src/chart/mod.rs
src/chart/model.rs
src/chart/interaction.rs
src/chart/indicators.rs
src/chart/drawings.rs
src/chart/layout.rs
src/chart/persistence.rs
src/chart/performance.rs
src/klinechart.rs
```

Responsibilities:

- `chart/model.rs`
  - Serializable chart layout, indicator instance, drawing instance, and user preferences.
- `chart/interaction.rs`
  - Pure viewport and input-command logic.
- `chart/indicators.rs`
  - Indicator parameter schemas, validation, instance labels, and cached series generation.
- `chart/drawings.rs`
  - Drawing creation/editing state, hit-testing, and serialization models.
- `chart/layout.rs`
  - Pane sizing and layout constraints.
- `chart/persistence.rs`
  - Load/save layout files, migrations, defaults.
- `chart/performance.rs`
  - Cache keys, candle feed versioning, and measurement helpers.
- `chart_panel.rs`
  - GPUI component composition and command wiring.

### 9.2 Layout schema example

```json
{
  "schema_version": 1,
  "exchange": "binance",
  "symbol": "BTC/USDT",
  "timeframe": "1m",
  "candle_type": "solid",
  "show_grid": false,
  "timezone": "UTC",
  "indicators": [
    {
      "id": "ema-20",
      "kind": "EMA",
      "pane": "main",
      "params": { "period": 20 },
      "style": { "color": "#C4E855", "width": 1.5 }
    }
  ],
  "drawings": [],
  "pane_ratios": { "oscillator-1": 0.18 }
}
```

### 9.3 Indicator cache

Cache key:

```text
(candle_buffer_version, indicator_kind, indicator_params_hash)
```

The `CandleFeed` should expose:

- `snapshot_with_version() -> (Vec<OhlcvCandle>, u64)`
- `last_update_version() -> u64`

This avoids recomputing indicators when the render loop runs but candle data has not changed.

### 9.4 Drawing storage

Drawings should be stored in chart coordinates:

```rust
pub struct DrawingPoint {
    pub timestamp: i64,
    pub price: f64,
}
```

Screen conversion should happen only during render and hit-testing through a viewport transform object.

### 9.5 Renderer boundary

`CandlestickChart` should remain responsible for drawing primitives and chart axes. Product-specific chart behavior should stay in `src/chart/*`:

- Indicator instance models.
- Drawing tool state.
- Persistence.
- Toolbar actions.
- Layout presets.

Only generic rendering support should be added to `gpui-component-patched`.

## 10. UX Requirements

### 10.1 Toolbar

A compact toolbar should appear in the chart header or top-left chart area:

- `Candles`: Solid / Heikin Ashi.
- `Indicators`: opens indicator library.
- `Draw`: opens drawing tool picker.
- `Grid`: toggles horizontal grid.
- `Latest`: re-pin viewport to the latest candle.
- `Layout`: select/save/reset presets.

### 10.2 Indicator panel & Standalone Window

The Indicator Library can be launched either from the title bar toolbar or via the shortcut (`I`). It provides:

- **Free-Floating Window Mode**: Rendered via `gpui::deferred` to avoid clipping, with draggable title bar and close button.
- **Search Header**: Full-text filter matching indicator names, symbols, and descriptions.
- **Category Tabs**: Filter by `All`, `Trend`, `Momentum`, `Volatility`, `Volume`.
- **Active Section**: Top panel listing all currently active indicators with quick eye (toggle visibility), settings (gear), and remove (x) controls.
- **Library Grid**: Categorized list with descriptions and an "Add" / "Added" toggle button.

Each indicator row / settings popup includes:

- Name and parameter summary (e.g. `EMA (20, close)`).
- Visibility toggle.
- Parameter inputs (periods, deviations, source).
- Visual style picker (line color, stroke width).
- Reset to default button.
- Remove action.
- Pane placement (Main vs Subpane).

### 10.3 Drawing tools

The drawing tool picker should include:

- Cursor/select.
- Horizontal line.
- Vertical line.
- Trendline.
- Ray.
- Segment.
- Fibonacci.
- Price channel.
- Text annotation.
- Long position.
- Short position.

### 10.4 Empty and error states

- Loading: "Loading market candles..."
- No data: "No candles available for this market/timeframe."
- Stale data: "Live feed paused. Showing last received candles."
- Error: Show source and retry/reconnect action if available.

## 11. Metrics

### 11.1 Product metrics

- Chart layout restored successfully after restart.
- Average time to add an indicator.
- Average time to draw a trendline.
- Number of chart settings preserved across sessions.

### 11.2 Quality metrics

- Pan/zoom responsiveness with 10,000 candles and 6 indicators.
- Indicator recomputation count per live candle update.
- Crash-free chart sessions.
- Number of chart-related panics or unwrap crashes.

### 11.3 Engineering metrics

- LOC reduction in `chart_panel.rs` after modularization.
- Unit test coverage for interaction, indicators, and drawing math.
- Number of chart behaviors testable without GPUI.

## 12. Milestones

### Milestone 1: Refactor without behavior change

- Split chart modules.
- Remove render-path debug output.
- Add pure tests for viewport and pane sizing.
- Preserve current UI.

### Milestone 2: Persist chart state

- Define versioned layout schema.
- Save/load local layout files.
- Restore indicators, pane sizes, grid, candle type, and timezone.
- Add reset-to-default action.

### Milestone 3: Indicator settings v1

- Indicator instance IDs.
- Parameter schemas and validation.
- Settings UI for top indicators: MA, EMA, BOLL, MACD, RSI, KDJ, VOL.
- Multiple instances of same indicator.

### Milestone 4: Drawing tools v1

- Drawing toolbar.
- Horizontal line, vertical line, trendline, Fibonacci, annotation.
- Select/move/delete.
- Persistence.

### Milestone 5: Performance pass

- Candle feed version counter.
- Indicator cache.
- Render-path allocation review.
- Benchmarks or measurement harness.

### Milestone 6: Professional polish

- Keyboard shortcuts.
- Better latest/autoscroll affordance.
- Stale/reconnecting states.
- Layout preset management.

## 13. Open Questions

1. Should chart layouts be global, per workspace, or per exchange/symbol/timeframe?
2. Should drawing overlays be shared across timeframes for the same symbol?
3. Should indicator presets sync with any future account/backend service, or stay local-only?
4. What is the target minimum hardware for 60 FPS performance?
5. Should the patched `gpui-component` chart module remain vendored long term, or should CWN own its chart renderer directly?
6. Which indicators are most important to expose parameter editing for first beyond EMA, MA, BOLL, MACD, RSI, KDJ, and VOL?

## 14. Risks and Mitigations

| Risk | Impact | Mitigation |
| --- | --- | --- |
| Refactor breaks existing chart behavior | High | Ship milestone 1 as behavior-preserving with screenshots/manual QA and unit tests. |
| Drawing tools become too complex | Medium | Start with simple anchored shapes and defer advanced snapping/style editing. |
| Indicator caching introduces stale displays | Medium | Use explicit candle buffer version and parameter hash keys. |
| Persistence schema changes break old layouts | Medium | Version schema and ignore unknown fields. |
| Renderer code keeps accumulating product logic | High | Define strict boundary: generic drawing primitives in renderer, product state in `src/chart/*`. |
| Performance measurements are subjective | Medium | Add repeatable local benchmark/scenario with 10,000 candles and fixed indicator set. |

## 15. Implementation Notes

- Keep the first implementation dependency-free unless a clear persistence or serialization dependency is already present in the project.
- Prefer pure Rust tests for interaction and indicator logic before GPUI integration tests.
- Do not change chart data semantics while refactoring UI structure.
- Avoid broad rewrites of `gpui-component-patched` unless necessary for generic renderer capabilities.
- Persist data in a local app config directory using a versioned schema.
- Keep chart defaults simple: solid candles, no indicators by default, UTC timezone, horizontal grid off unless user enables it.

## 16. Definition of Done

The chart improvement plan is complete when:

- The chart module is split into maintainable modules.
- Chart layouts persist and restore across restart.
- Users can configure and persist common indicator parameters.
- Users can create, edit, delete, and persist basic drawings.
- Chart interaction remains smooth with 10,000 candles and multiple indicators.
- The app exposes clear chart controls without cluttering the workspace.
- Core chart state and math are covered by unit tests independent of GPUI.
