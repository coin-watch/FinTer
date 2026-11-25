//! Interactive chart view.
//!
//! `ChartView` is an entity, not a stateless element: it owns the viewport,
//! cursor, and gesture state, so pointer movement invalidates only the chart
//! subtree instead of the whole panel. The scene is rebuilt in `prepaint` and
//! painted by the shared [`Painter`].

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    App, Context, EventEmitter, FocusHandle, Focusable, InteractiveElement, IntoElement,
    KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels,
    Point as GpuiPoint, Render, ScrollWheelEvent, SharedString, StatefulInteractiveElement, Styled,
    Window, div, px,
};

use crate::PixelsExt;
use crate::chart::charts::{axes, candles, grid, volume};
use crate::chart::core::{
    AxisHandle, ChartLayout, DragClassifier, DragUpdate, LayoutOptions, LodLevel, Ohlcv, PaneKind,
    Point, PriceScale, PriceScaleMode, Rect, Spring, VelocityTracker, Viewport, ViewportLimits,
    axis_handle, compute, drag_price_range, drag_time_span, price_rail_width,
};
use crate::chart::scene::{Layer, Painter, Primitive, Scene};
use crate::chart::theme::ChartPalette;
use crate::menu::{ContextMenuExt, PopupMenuItem};

#[derive(Clone, Debug)]
pub struct ChartSnapshot {
    pub bars: Arc<[Ohlcv]>,
    pub timestamps: Arc<[i64]>,
    pub epoch: u64,
    /// Median timestamp spacing, used to extend the axis into blank time.
    pub interval_ms: i64,
}

impl Default for ChartSnapshot {
    fn default() -> Self {
        Self {
            bars: Arc::from([]),
            timestamps: Arc::from([]),
            epoch: 0,
            interval_ms: 60_000,
        }
    }
}

impl ChartSnapshot {
    pub fn new(bars: Arc<[Ohlcv]>, timestamps: Arc<[i64]>, epoch: u64) -> Self {
        let len = bars.len().min(timestamps.len());
        let bars = if bars.len() != len {
            Arc::from(&bars[..len])
        } else {
            bars
        };
        let timestamps = if timestamps.len() != len {
            Arc::from(&timestamps[..len])
        } else {
            timestamps
        };
        let interval_ms = median_interval_ms(&timestamps);
        Self {
            bars,
            timestamps,
            epoch,
            interval_ms,
        }
    }

    pub fn len(&self) -> usize {
        self.bars.len().min(self.timestamps.len())
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[derive(Clone)]
pub struct ChartOptions {
    pub title: SharedString,
    pub show_grid: bool,
    pub price_decimals: usize,
    pub price_formatter: Option<Arc<dyn Fn(f64) -> SharedString>>,
    pub time_formatter: Option<Arc<dyn Fn(i64) -> SharedString>>,
    pub scale_mode: PriceScaleMode,
    pub pane_ratios: Vec<f32>,
    pub has_volume: bool,
    pub target_price_ticks: usize,
    pub target_time_ticks: usize,
}

impl Default for ChartOptions {
    fn default() -> Self {
        Self {
            title: SharedString::default(),
            show_grid: true,
            price_decimals: 2,
            price_formatter: None,
            time_formatter: None,
            scale_mode: PriceScaleMode::Linear,
            pane_ratios: vec![0.18],
            has_volume: true,
            target_price_ticks: 6,
            target_time_ticks: 6,
        }
    }
}

impl ChartOptions {
    fn format_price(&self, value: f64) -> SharedString {
        if let Some(formatter) = &self.price_formatter {
            formatter(value)
        } else if value.is_finite() {
            format!("{:.*}", self.price_decimals, value).into()
        } else {
            "--".into()
        }
    }

    fn format_time(&self, timestamp_ms: i64) -> SharedString {
        if let Some(formatter) = &self.time_formatter {
            formatter(timestamp_ms)
        } else {
            format!("{}", timestamp_ms / 1_000).into()
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InteractionMode {
    Full,
    None,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ChartEvent {
    CursorChanged {
        index: Option<usize>,
        price: f64,
        timestamp_ms: Option<i64>,
    },
    ViewportChanged {
        start: f64,
        span: f64,
        pinned: bool,
    },
    ScaleChanged {
        mode: PriceScaleMode,
        manual: Option<(f64, f64)>,
    },
}

/// Whether the price axis follows visible data or a user-locked range.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PriceAxisState {
    Auto,
    Manual { min: f64, max: f64 },
}

#[derive(Clone, Debug)]
enum AxisDrag {
    Price {
        start_y: f32,
        start_range: (f64, f64),
        anchor_price: f64,
        moved: bool,
    },
    Time {
        start_x: f32,
        start_span: f64,
        anchor_slot: f64,
        anchor_frac: f64,
        moved: bool,
    },
}

/// Cursor information in both pixel and time space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CursorInfo {
    pub pos: Point,
    /// Continuous slot under the cursor. May be negative or past `total`.
    pub slot: f64,
    /// `Some` only when the cursor is over a real candle.
    pub index: Option<usize>,
    pub price: f64,
    /// Extrapolated beyond the data range.
    pub timestamp_ms: Option<i64>,
}

struct RenderState {
    layout: ChartLayout,
    scene: Scene,
    price_range: (f64, f64),
    last_price_range: Option<(f64, f64)>,
    scale_mode: PriceScaleMode,
}

impl Default for RenderState {
    fn default() -> Self {
        Self {
            layout: ChartLayout::default(),
            scene: Scene::default(),
            price_range: (0.0, 1.0),
            last_price_range: None,
            scale_mode: PriceScaleMode::Linear,
        }
    }
}

pub struct ChartView {
    snapshot: ChartSnapshot,
    options: ChartOptions,
    palette: ChartPalette,
    viewport: Viewport,
    limits: ViewportLimits,
    layout_options: LayoutOptions,
    mode: InteractionMode,
    hovered: Option<usize>,
    cursor: Option<CursorInfo>,
    drag: DragClassifier,
    drag_origin_start: f64,
    panning: bool,
    velocity: VelocityTracker,
    drag_started: Instant,
    axis_drag: Option<AxisDrag>,
    hover_axis: Option<AxisHandle>,
    last_right_click: Option<Point>,
    price_axis_state: PriceAxisState,
    magnet: bool,
    pan_spring: Spring,
    pan_animating: bool,
    last_frame: Instant,
    runtime: Rc<RefCell<RenderState>>,
    painter: Rc<RefCell<Painter>>,
    focus_handle: FocusHandle,
}

impl EventEmitter<ChartEvent> for ChartView {}

impl Focusable for ChartView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl ChartView {
    pub fn new(
        snapshot: ChartSnapshot,
        options: ChartOptions,
        palette: ChartPalette,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut viewport = Viewport::default();
        viewport.scroll_to_latest(snapshot.len(), &ViewportLimits::default());
        let view = Self {
            snapshot,
            options,
            palette,
            viewport,
            limits: ViewportLimits::default(),
            layout_options: LayoutOptions::default(),
            mode: InteractionMode::Full,
            hovered: None,
            cursor: None,
            drag: DragClassifier::new(3.0),
            drag_origin_start: 0.0,
            panning: false,
            velocity: VelocityTracker::new(0.08),
            drag_started: Instant::now(),
            axis_drag: None,
            hover_axis: None,
            last_right_click: None,
            price_axis_state: PriceAxisState::Auto,
            magnet: false,
            pan_spring: Spring::new(0.0),
            pan_animating: false,
            last_frame: Instant::now(),
            runtime: Rc::new(RefCell::new(RenderState::default())),
            painter: Rc::new(RefCell::new(Painter::new())),
            focus_handle: cx.focus_handle(),
        };
        Self::start_countdown_ticker(cx);
        view
    }

    fn start_countdown_ticker(cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                if this
                    .update(cx, |view, cx| {
                        if matches!(view.mode, InteractionMode::None) {
                            return;
                        }
                        let total = view.snapshot.len();
                        if total == 0 {
                            return;
                        }
                        let last = (total - 1) as f64;
                        if last >= view.viewport.start - 0.5
                            && last <= view.viewport.start + view.viewport.span + 0.5
                        {
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    pub fn with_mode(mut self, mode: InteractionMode) -> Self {
        self.mode = mode;
        self
    }

    pub fn with_viewport(mut self, viewport: Viewport) -> Self {
        self.viewport = viewport;
        self.viewport.clamp(self.snapshot.len(), &self.limits);
        self
    }

    pub fn with_limits(mut self, limits: ViewportLimits) -> Self {
        self.limits = limits;
        self.viewport.clamp(self.snapshot.len(), &self.limits);
        self
    }

    pub fn set_snapshot(&mut self, snapshot: ChartSnapshot, cx: &mut Context<Self>) {
        if snapshot.epoch == self.snapshot.epoch {
            return;
        }
        let total = snapshot.len();
        let old_total = self.snapshot.len();
        let old_last = self.snapshot.timestamps.last().copied();
        let old_live = old_total as f64 - self.viewport.span + self.limits.right_margin;
        let was_in_future = self.viewport.start > old_live;
        let appended = match old_last {
            Some(old_last) => snapshot
                .timestamps
                .iter()
                .filter(|timestamp| **timestamp > old_last)
                .count(),
            None => 0,
        };
        // Prepending older history must not move the candles under the viewport.
        let prepended = match (
            self.snapshot.timestamps.first().copied(),
            snapshot.timestamps.first().copied(),
        ) {
            (Some(old_first), Some(new_first)) if new_first < old_first => snapshot
                .timestamps
                .iter()
                .take_while(|timestamp| **timestamp < old_first)
                .count(),
            _ => 0,
        };
        self.snapshot = snapshot;
        if prepended > 0 {
            self.viewport.prepend(prepended);
        }
        // If the user is looking at blank future time, appending candles should
        // not teleport the view backward in wall-clock time.
        if !self.viewport.pinned && was_in_future && appended > 0 {
            self.viewport.start += appended as f64;
        }
        if self.viewport.pinned {
            self.viewport.scroll_to_latest(total, &self.limits);
        } else {
            self.viewport.clamp(total, &self.limits);
        }
        cx.notify();
    }

    pub fn set_options(&mut self, options: ChartOptions, cx: &mut Context<Self>) {
        // Scale mode is owned by the view so a panel re-sync does not reset a
        // user-selected log/percent scale.
        let mode = self.options.scale_mode;
        self.options = options;
        self.options.scale_mode = mode;
        cx.notify();
    }

    pub fn price_axis_state(&self) -> PriceAxisState {
        self.price_axis_state
    }

    pub fn price_scale_mode(&self) -> PriceScaleMode {
        self.options.scale_mode
    }

    pub fn set_price_axis_state(&mut self, state: PriceAxisState, cx: &mut Context<Self>) {
        if self.price_axis_state != state {
            self.price_axis_state = state;
            self.emit_scale(cx);
            cx.notify();
        }
    }

    pub fn set_price_scale_mode(&mut self, mode: PriceScaleMode, cx: &mut Context<Self>) {
        if self.options.scale_mode != mode {
            self.options.scale_mode = mode;
            self.emit_scale(cx);
            cx.notify();
        }
    }

    pub fn reset_price_scale(&mut self, cx: &mut Context<Self>) {
        if self.price_axis_state != PriceAxisState::Auto {
            self.price_axis_state = PriceAxisState::Auto;
            self.emit_scale(cx);
            cx.notify();
        }
    }

    pub fn cycle_price_scale_mode(&mut self, cx: &mut Context<Self>) {
        let next = match self.options.scale_mode {
            PriceScaleMode::Linear => PriceScaleMode::Log,
            PriceScaleMode::Log => {
                let (min, max) = self.runtime.borrow().price_range;
                let anchor = if min.is_finite() && max.is_finite() && max > min {
                    (min + max) * 0.5
                } else {
                    1.0
                };
                PriceScaleMode::percent(anchor)
            }
            PriceScaleMode::Percent { .. } => PriceScaleMode::InvertedLinear,
            PriceScaleMode::InvertedLinear => PriceScaleMode::Linear,
        };
        self.set_price_scale_mode(next, cx);
    }

    pub fn magnet(&self) -> bool {
        self.magnet
    }

    pub fn is_animating(&self) -> bool {
        self.pan_animating
    }

    pub fn set_magnet(&mut self, magnet: bool, cx: &mut Context<Self>) {
        if self.magnet != magnet {
            self.magnet = magnet;
            cx.notify();
        }
    }

    pub fn fit_visible(&mut self, cx: &mut Context<Self>) {
        let total = self.snapshot.len();
        let visible = self.viewport.visible(total, &self.limits);
        if visible.is_empty() {
            self.viewport.reset_view(total, &self.limits);
        } else {
            self.viewport.fit_to(visible, total, &self.limits);
        }
        cx.emit(ChartEvent::ViewportChanged {
            start: self.viewport.start,
            span: self.viewport.span,
            pinned: self.viewport.pinned,
        });
        cx.notify();
    }

    fn emit_scale(&self, cx: &mut Context<Self>) {
        let manual = match self.price_axis_state {
            PriceAxisState::Auto => None,
            PriceAxisState::Manual { min, max } => Some((min, max)),
        };
        cx.emit(ChartEvent::ScaleChanged {
            mode: self.options.scale_mode,
            manual,
        });
    }

    pub fn set_palette(&mut self, palette: ChartPalette, cx: &mut Context<Self>) {
        self.palette = palette;
        cx.notify();
    }

    pub fn set_palette_if_changed(&mut self, palette: ChartPalette, cx: &mut Context<Self>) {
        if self.palette != palette {
            self.palette = palette;
            // Shaped text bakes in the label color.
            self.painter.borrow_mut().text.clear();
            cx.notify();
        }
    }

    pub fn snapshot_epoch(&self) -> u64 {
        self.snapshot.epoch
    }

    pub fn viewport(&self) -> Viewport {
        self.viewport
    }

    pub fn cursor(&self) -> Option<CursorInfo> {
        self.cursor
    }

    pub fn layout(&self) -> ChartLayout {
        self.runtime.borrow().layout.clone()
    }

    #[cfg(test)]
    fn runtime(&self) -> std::cell::Ref<'_, RenderState> {
        self.runtime.borrow()
    }

    pub fn set_viewport(&mut self, viewport: Viewport, cx: &mut Context<Self>) {
        self.viewport = viewport;
        self.viewport.clamp(self.snapshot.len(), &self.limits);
        cx.notify();
    }

    pub fn reset_view(&mut self, cx: &mut Context<Self>) {
        self.viewport.reset_view(self.snapshot.len(), &self.limits);
        cx.notify();
    }

    pub fn scroll_to_latest(&mut self, cx: &mut Context<Self>) {
        self.viewport
            .scroll_to_latest(self.snapshot.len(), &self.limits);
        cx.notify();
    }

    fn plot_rect(&self) -> Option<Rect> {
        let runtime = self.runtime.borrow();
        let plot = runtime.layout.plot;
        if plot.w > 0.0 && plot.h > 0.0 {
            Some(plot)
        } else {
            None
        }
    }

    fn cursor_info_for_slot(&self, slot: f64, y: f32, plot: Rect, magnet: bool) -> CursorInfo {
        let runtime = self.runtime.borrow();
        let (min, max) = runtime.price_range;
        let scale = PriceScale::new(runtime.scale_mode, min, max, plot);
        let total = self.snapshot.len();
        let rounded = slot.round();
        let index = if total > 0 && rounded >= 0.0 && rounded < total as f64 {
            Some(rounded as usize)
        } else {
            None
        };
        let timestamp_ms = timestamp_at_slot(&self.snapshot, slot);
        let x = self
            .viewport
            .index_to_x(slot, plot)
            .clamp(plot.x, plot.right() - 0.01);
        let mut price = scale.to_price(y);
        let mut y = y;
        if magnet {
            if let Some(index) = index {
                let bar = self.snapshot.bars[index];
                let candidates = [bar.open, bar.high, bar.low, bar.close];
                if let Some(best) = candidates
                    .iter()
                    .copied()
                    .filter(|value| value.is_finite())
                    .min_by(|a, b| (a - price).abs().total_cmp(&(b - price).abs()))
                {
                    price = best;
                    y = scale.to_y(best);
                }
            }
        }
        CursorInfo {
            pos: Point::new(x, y),
            slot,
            index,
            price,
            timestamp_ms,
        }
    }

    fn cursor_info_at(&self, point: Point, plot: Rect, magnet: bool) -> CursorInfo {
        let slot = self.viewport.x_to_index(point.x, plot);
        self.cursor_info_for_slot(slot, point.y, plot, magnet)
    }

    fn handle_move(
        &mut self,
        position: GpuiPoint<Pixels>,
        modifiers: gpui::Modifiers,
        cx: &mut Context<Self>,
    ) {
        if matches!(self.mode, InteractionMode::None) {
            return;
        }
        let Some(plot) = self.plot_rect() else {
            return;
        };
        let point = Point::new(position.x.as_f32(), position.y.as_f32());

        if let Some(drag) = self.axis_drag.take() {
            match drag {
                AxisDrag::Price {
                    start_y,
                    start_range,
                    anchor_price,
                    mut moved,
                } => {
                    let dy = point.y - start_y;
                    if !moved && dy.abs() < 2.0 {
                        self.axis_drag = Some(AxisDrag::Price {
                            start_y,
                            start_range,
                            anchor_price,
                            moved,
                        });
                        return;
                    }
                    moved = true;
                    let range =
                        drag_price_range(self.options.scale_mode, start_range, anchor_price, dy);
                    if range.0.is_finite() && range.1.is_finite() && range.1 > range.0 {
                        self.price_axis_state = PriceAxisState::Manual {
                            min: range.0,
                            max: range.1,
                        };
                    }
                    self.axis_drag = Some(AxisDrag::Price {
                        start_y,
                        start_range,
                        anchor_price,
                        moved,
                    });
                    cx.notify();
                }
                AxisDrag::Time {
                    start_x,
                    start_span,
                    anchor_slot,
                    anchor_frac,
                    mut moved,
                } => {
                    let dx = point.x - start_x;
                    if !moved && dx.abs() < 2.0 {
                        self.axis_drag = Some(AxisDrag::Time {
                            start_x,
                            start_span,
                            anchor_slot,
                            anchor_frac,
                            moved,
                        });
                        return;
                    }
                    moved = true;
                    let new_span =
                        drag_time_span(start_span, dx, self.limits.min_span, self.limits.max_span);
                    self.viewport.span = new_span;
                    self.viewport.start = anchor_slot - anchor_frac * new_span;
                    self.viewport.pinned = false;
                    self.viewport.clamp(self.snapshot.len(), &self.limits);
                    self.axis_drag = Some(AxisDrag::Time {
                        start_x,
                        start_span,
                        anchor_slot,
                        anchor_frac,
                        moved,
                    });
                    cx.notify();
                }
            }
            return;
        }

        if self.panning {
            if let DragUpdate::Dragging { dx, .. } = self.drag.update(point) {
                let slot = self.viewport.slot_width(plot);
                if slot > 0.0 {
                    let raw_start = self.drag_origin_start - (dx / slot) as f64;
                    let (min_start, max_start) =
                        self.viewport.bounds(self.snapshot.len(), &self.limits);
                    // Rubber-band past the blank-time limits instead of
                    // stopping dead at the edge.
                    self.viewport.start = if raw_start < min_start {
                        min_start + (raw_start - min_start) * 0.35
                    } else if raw_start > max_start {
                        max_start + (raw_start - max_start) * 0.35
                    } else {
                        raw_start
                    };
                    self.viewport.pinned = false;
                    let elapsed = self.drag_started.elapsed().as_secs_f64();
                    self.velocity.push(elapsed, point.x);
                    cx.emit(ChartEvent::ViewportChanged {
                        start: self.viewport.start,
                        span: self.viewport.span,
                        pinned: self.viewport.pinned,
                    });
                    cx.notify();
                }
            }
            return;
        }

        let layout = {
            let runtime = self.runtime.borrow();
            runtime.layout.clone()
        };
        if let Some(axis) = axis_handle(point, &layout, 2.0) {
            let changed = self.hover_axis != Some(axis) || self.cursor.is_some();
            self.hover_axis = Some(axis);
            if changed {
                self.cursor = None;
                self.hovered = None;
                cx.notify();
            }
            return;
        }
        self.hover_axis = None;

        if !layout.plot.contains(point) {
            if self.cursor.is_some() || self.hovered.is_some() {
                self.cursor = None;
                self.hovered = None;
                cx.notify();
            }
            return;
        }
        let magnet = self.magnet || modifiers.alt;
        let info = self.cursor_info_at(point, layout.plot, magnet);
        self.cursor = Some(info);
        self.hovered = info.index;
        cx.emit(ChartEvent::CursorChanged {
            index: info.index,
            price: info.price,
            timestamp_ms: info.timestamp_ms,
        });
        cx.notify();
    }

    fn handle_down(
        &mut self,
        position: GpuiPoint<Pixels>,
        click_count: usize,
        cx: &mut Context<Self>,
    ) {
        if matches!(self.mode, InteractionMode::None) {
            return;
        }
        let point = Point::new(position.x.as_f32(), position.y.as_f32());
        let (layout, price_range) = {
            let runtime = self.runtime.borrow();
            (runtime.layout.clone(), runtime.price_range)
        };
        if layout.plot.is_empty() {
            return;
        }
        if click_count >= 2 {
            if let Some(axis) = axis_handle(point, &layout, 2.0) {
                match axis {
                    AxisHandle::Price => self.reset_price_scale(cx),
                    AxisHandle::Time => {
                        self.viewport.reset_view(self.snapshot.len(), &self.limits);
                        cx.emit(ChartEvent::ViewportChanged {
                            start: self.viewport.start,
                            span: self.viewport.span,
                            pinned: self.viewport.pinned,
                        });
                        cx.notify();
                    }
                }
                return;
            }
            if layout.plot.contains(point) {
                self.fit_visible(cx);
                return;
            }
        }
        if let Some(axis) = axis_handle(point, &layout, 2.0) {
            self.hover_axis = Some(axis);
            match axis {
                AxisHandle::Price => {
                    let scale = PriceScale::new(
                        self.options.scale_mode,
                        price_range.0,
                        price_range.1,
                        layout.plot,
                    );
                    self.axis_drag = Some(AxisDrag::Price {
                        start_y: point.y,
                        start_range: price_range,
                        anchor_price: scale.to_price(point.y),
                        moved: false,
                    });
                }
                AxisHandle::Time => {
                    let anchor_frac =
                        ((point.x - layout.plot.x) / layout.plot.w).clamp(0.0, 1.0) as f64;
                    self.axis_drag = Some(AxisDrag::Time {
                        start_x: point.x,
                        start_span: self.viewport.span,
                        anchor_slot: self.viewport.x_to_position(point.x, layout.plot),
                        anchor_frac,
                        moved: false,
                    });
                }
            }
            cx.notify();
            return;
        }
        if !layout.plot.contains(point) {
            return;
        }
        self.drag.press(point);
        self.drag_origin_start = self.viewport.start;
        self.panning = true;
        self.velocity.clear();
        self.velocity.push(0.0, point.x);
        self.drag_started = Instant::now();
        cx.notify();
    }

    fn handle_up(&mut self, position: GpuiPoint<Pixels>, cx: &mut Context<Self>) {
        if let Some(drag) = self.axis_drag.take() {
            match drag {
                AxisDrag::Price { moved, .. } => {
                    if moved {
                        self.emit_scale(cx);
                    }
                }
                AxisDrag::Time { moved, .. } => {
                    if moved {
                        cx.emit(ChartEvent::ViewportChanged {
                            start: self.viewport.start,
                            span: self.viewport.span,
                            pinned: self.viewport.pinned,
                        });
                    }
                }
            }
            cx.notify();
            return;
        }
        if !self.panning {
            return;
        }
        self.panning = false;
        self.drag
            .release(Point::new(position.x.as_f32(), position.y.as_f32()));
        let total = self.snapshot.len();
        let velocity = self.velocity.velocity();
        if let Some(plot) = self.plot_rect() {
            let slot = self.viewport.slot_width(plot);
            if slot > 0.0 && velocity.abs() > 60.0 {
                let slots_per_sec = -(velocity / slot) as f64;
                let (min_start, max_start) = self.viewport.bounds(total, &self.limits);
                let target =
                    (self.viewport.start + slots_per_sec * 0.25).clamp(min_start, max_start);
                self.pan_spring.snap(self.viewport.start as f32);
                self.pan_spring.fling(target as f32, slots_per_sec as f32);
                self.pan_animating = true;
                self.last_frame = Instant::now();
            } else {
                self.viewport.clamp(total, &self.limits);
            }
        } else {
            self.viewport.clamp(total, &self.limits);
        }
        cx.emit(ChartEvent::ViewportChanged {
            start: self.viewport.start,
            span: self.viewport.span,
            pinned: self.viewport.pinned,
        });
        cx.notify();
    }

    fn handle_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        if matches!(self.mode, InteractionMode::None) {
            return;
        }
        let Some(plot) = self.plot_rect() else {
            return;
        };
        let total = self.snapshot.len();
        if total == 0 {
            return;
        }
        match event.keystroke.key.as_str() {
            "left" | "right" => {
                let current_slot = self.cursor.map(|cursor| cursor.slot).unwrap_or_else(|| {
                    let visible = self.viewport.visible(total, &self.limits);
                    visible.end.saturating_sub(1) as f64
                });
                let delta = if event.keystroke.key == "left" {
                    -1.0
                } else {
                    1.0
                };
                let (min_start, max_start) = self.viewport.bounds(total, &self.limits);
                let next_slot =
                    (current_slot + delta).clamp(min_start, max_start + self.viewport.span);
                self.viewport
                    .ensure_slot_visible(next_slot, 2.0, total, &self.limits);
                let y = self
                    .cursor
                    .map(|cursor| cursor.pos.y)
                    .unwrap_or(plot.y + plot.h * 0.5);
                let info = self.cursor_info_for_slot(next_slot, y, plot, self.magnet);
                self.hovered = info.index;
                self.cursor = Some(info);
                cx.emit(ChartEvent::CursorChanged {
                    index: info.index,
                    price: info.price,
                    timestamp_ms: info.timestamp_ms,
                });
                cx.notify();
            }
            "+" | "=" => {
                self.viewport
                    .zoom_at(0.9, plot.center().x, plot, total, &self.limits);
                cx.emit(ChartEvent::ViewportChanged {
                    start: self.viewport.start,
                    span: self.viewport.span,
                    pinned: self.viewport.pinned,
                });
                cx.notify();
            }
            "-" => {
                self.viewport
                    .zoom_at(1.1, plot.center().x, plot, total, &self.limits);
                cx.emit(ChartEvent::ViewportChanged {
                    start: self.viewport.start,
                    span: self.viewport.span,
                    pinned: self.viewport.pinned,
                });
                cx.notify();
            }
            "f" => {
                self.viewport.reset_view(total, &self.limits);
                cx.notify();
            }
            "l" => {
                self.viewport.scroll_to_latest(total, &self.limits);
                cx.notify();
            }
            "escape" => {
                self.cursor = None;
                self.hovered = None;
                cx.notify();
            }
            _ => {}
        }
    }

    fn handle_scroll(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        if matches!(self.mode, InteractionMode::None) {
            return;
        }
        let (layout, price_range) = {
            let runtime = self.runtime.borrow();
            (runtime.layout.clone(), runtime.price_range)
        };
        let plot = layout.plot;
        if plot.is_empty() {
            return;
        }
        let position = event.position;
        let point = Point::new(position.x.as_f32(), position.y.as_f32());
        let x = point.x;
        let delta = event.delta.pixel_delta(px(1.0));
        let dy = delta.y.as_f32();

        if let Some(axis) = axis_handle(point, &layout, 2.0) {
            match axis {
                AxisHandle::Price => {
                    if dy.abs() < 0.5 {
                        return;
                    }
                    let scale = PriceScale::new(
                        self.options.scale_mode,
                        price_range.0,
                        price_range.1,
                        plot,
                    );
                    let anchor = scale.to_price(point.y);
                    let step = if dy > 0.0 { -14.0 } else { 14.0 };
                    let range =
                        drag_price_range(self.options.scale_mode, price_range, anchor, step);
                    if range.0.is_finite() && range.1.is_finite() && range.1 > range.0 {
                        self.price_axis_state = PriceAxisState::Manual {
                            min: range.0,
                            max: range.1,
                        };
                        self.emit_scale(cx);
                        cx.notify();
                    }
                }
                AxisHandle::Time => {
                    if dy.abs() < 0.5 {
                        return;
                    }
                    let anchor = x.clamp(plot.x, plot.right() - 0.5);
                    let factor = if dy > 0.0 { 0.92 } else { 1.08 };
                    self.viewport
                        .zoom_at(factor, anchor, plot, self.snapshot.len(), &self.limits);
                    cx.emit(ChartEvent::ViewportChanged {
                        start: self.viewport.start,
                        span: self.viewport.span,
                        pinned: self.viewport.pinned,
                    });
                    cx.notify();
                }
            }
            return;
        }

        if dy > 0.0 {
            self.viewport.zoom_at(
                0.9,
                x.clamp(plot.x, plot.right() - 0.5),
                plot,
                self.snapshot.len(),
                &self.limits,
            );
        } else if dy < 0.0 {
            self.viewport.zoom_at(
                1.1,
                x.clamp(plot.x, plot.right() - 0.5),
                plot,
                self.snapshot.len(),
                &self.limits,
            );
        }
        if delta.x.as_f32().abs() > 0.5 {
            let slot = self.viewport.slot_width(plot);
            if slot > 0.0 {
                self.viewport.start -= (delta.x.as_f32() / slot) as f64;
                self.viewport.pinned = false;
                self.viewport.clamp(self.snapshot.len(), &self.limits);
            }
        }
        cx.emit(ChartEvent::ViewportChanged {
            start: self.viewport.start,
            span: self.viewport.span,
            pinned: self.viewport.pinned,
        });
        cx.notify();
    }
}

impl Render for ChartView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.pan_animating {
            let now = Instant::now();
            let dt = now.duration_since(self.last_frame).as_secs_f32();
            self.last_frame = now;
            let active = self.pan_spring.tick(dt);
            self.viewport.start = self.pan_spring.value() as f64;
            self.viewport.clamp(self.snapshot.len(), &self.limits);
            self.pan_animating = active;
            if active {
                window.request_animation_frame();
            }
        }
        let runtime = self.runtime.clone();
        let prepaint_painter = self.painter.clone();
        let paint_painter = self.painter.clone();
        let prepaint_runtime = runtime.clone();
        let paint_runtime = runtime;
        let snapshot = self.snapshot.clone();
        let options = self.options.clone();
        let palette = self.palette;
        let viewport = self.viewport;
        let limits = self.limits;
        let layout_options = self.layout_options;
        let hovered = self.hovered;
        let cursor = self.cursor;
        let mode = self.mode;
        let magnet = self.magnet;
        let price_axis_state = self.price_axis_state;
        let scale_chip = format!(
            "{} {}",
            match price_axis_state {
                PriceAxisState::Auto => "AUTO",
                PriceAxisState::Manual { .. } => "MANUAL",
            },
            self.options.scale_mode.label()
        );
        let cursor_style = match (&self.axis_drag, self.hover_axis) {
            (Some(AxisDrag::Price { .. }), _) | (None, Some(AxisHandle::Price)) => 1,
            (Some(AxisDrag::Time { .. }), _) | (None, Some(AxisHandle::Time)) => 2,
            _ => 0,
        };
        let font = window.text_style().font();
        let font_size = 11.0_f32;

        let canvas = gpui::canvas(
            move |bounds, window, _cx| {
                let rect = Rect::new(
                    bounds.origin.x.as_f32(),
                    bounds.origin.y.as_f32(),
                    bounds.size.width.as_f32(),
                    bounds.size.height.as_f32(),
                );
                let total = snapshot.len();
                let visible = viewport.visible(total, &limits);
                let mut state = prepaint_runtime.borrow_mut();
                let (min, max) = match price_axis_state {
                    PriceAxisState::Manual { min, max }
                        if min.is_finite() && max.is_finite() && max > min =>
                    {
                        (min, max)
                    }
                    _ => {
                        if visible.is_empty() {
                            state.last_price_range.unwrap_or_else(|| {
                                edge_price_range(&snapshot.bars, viewport.start >= total as f64)
                            })
                        } else {
                            let range = price_range(&snapshot.bars, visible.clone());
                            state.last_price_range = Some(range);
                            range
                        }
                    }
                };
                let sample = options.format_price(max);
                let rail = {
                    let mut painter = prepaint_painter.borrow_mut();
                    price_rail_width(
                        &[sample.as_str()],
                        |label| {
                            let label: SharedString = label.to_string().into();
                            painter.text.measure(
                                &label,
                                &font,
                                font_size,
                                palette.axis_text,
                                window,
                            )
                        },
                        16.0,
                        56.0,
                        96.0,
                    )
                };
                let layout = compute(
                    rect,
                    rail,
                    &options.pane_ratios,
                    options.has_volume,
                    layout_options,
                );
                if layout.plot.is_empty() {
                    state.layout = layout;
                    state.scene = Scene::new();
                    return;
                }
                let scale = PriceScale::new(options.scale_mode, min, max, layout.plot);
                let slot = viewport.slot_width(layout.plot);
                let lod = LodLevel::for_slot_width(slot);
                let style = candles::CandleStyle::from_palette(&palette);

                let mut scene = Scene::new();
                scene.push(
                    Layer::Background,
                    Primitive::rect(layout.bounds, palette.canvas),
                );
                if options.show_grid {
                    grid::build_horizontal_grid(
                        &mut scene,
                        layout.plot,
                        &scale,
                        &palette,
                        options.target_price_ticks,
                    );
                }
                candles::build_candles(
                    &mut scene,
                    &snapshot.bars,
                    visible.clone(),
                    &viewport,
                    layout.plot,
                    &scale,
                    &style,
                    hovered,
                    lod,
                );
                if options.has_volume {
                    if let Some(pane) = layout
                        .panes
                        .iter()
                        .find(|pane| matches!(pane.kind, PaneKind::Volume))
                    {
                        volume::build_volume(
                            &mut scene,
                            &snapshot.bars,
                            visible.clone(),
                            &viewport,
                            pane.rect,
                            &palette,
                        );
                    }
                }
                axes::build_price_axis(
                    &mut scene,
                    layout.plot,
                    &scale,
                    &palette,
                    options.target_price_ticks,
                    |value| options.format_price(value),
                );
                build_time_ticks(
                    &mut scene,
                    &layout,
                    &snapshot,
                    &viewport,
                    total,
                    &limits,
                    &options,
                    &palette,
                    &mut |label: &str| {
                        let label: SharedString = label.to_string().into();
                        prepaint_painter.borrow_mut().text.measure(
                            &label,
                            &font,
                            10.0,
                            palette.axis_text,
                            window,
                        )
                    },
                );
                axes::build_rail_separator(&mut scene, &layout, &palette);
                let latest_visible = total > 0 && visible.contains(&(total - 1));
                build_readout(
                    &mut scene,
                    &layout,
                    &snapshot,
                    cursor,
                    latest_visible,
                    magnet,
                    &options,
                    &palette,
                );

                if matches!(mode, InteractionMode::Full) {
                    if let Some(cursor) = cursor {
                        build_crosshair(&mut scene, &layout, cursor, &options, &palette);
                    }
                }

                state.layout = layout;
                state.scene = scene;
                state.price_range = (min, max);
                state.scale_mode = options.scale_mode;
            },
            move |_bounds, _state, window, cx| {
                let state = paint_runtime.borrow();
                paint_painter
                    .borrow_mut()
                    .paint(&state.scene, state.layout.bounds, window, cx);
            },
        )
        .size_full();

        let focus_handle = self.focus_handle.clone();
        let mut chart_area = div()
            .id("chart_v2_canvas")
            .track_focus(&focus_handle)
            .flex_1()
            .min_w_0()
            .h_full()
            .relative()
            .child(canvas);
        if matches!(mode, InteractionMode::Full) {
            chart_area = match cursor_style {
                1 => chart_area.cursor_row_resize(),
                2 => chart_area.cursor_col_resize(),
                _ => chart_area.cursor_crosshair(),
            };
            chart_area = chart_area
                .on_mouse_move({
                    cx.listener(|view, event: &MouseMoveEvent, _window, cx| {
                        view.handle_move(event.position, event.modifiers, cx);
                    })
                })
                .on_mouse_down(MouseButton::Left, {
                    cx.listener(|view, event: &MouseDownEvent, window, cx| {
                        view.handle_down(event.position, event.click_count, cx);
                        cx.focus_self(window);
                    })
                })
                .on_mouse_up(MouseButton::Left, {
                    cx.listener(|view, event: &MouseUpEvent, _window, cx| {
                        view.handle_up(event.position, cx);
                    })
                })
                .on_mouse_up_out(MouseButton::Left, {
                    cx.listener(|view, event: &MouseUpEvent, _window, cx| {
                        view.handle_up(event.position, cx);
                    })
                })
                .on_mouse_down(MouseButton::Right, {
                    cx.listener(|view, event: &MouseDownEvent, _window, cx| {
                        view.last_right_click = Some(Point::new(
                            event.position.x.as_f32(),
                            event.position.y.as_f32(),
                        ));
                        cx.notify();
                    })
                })
                .on_hover(cx.listener(|view, hovered: &bool, _window, cx| {
                    if !*hovered
                        && (view.cursor.is_some()
                            || view.hovered.is_some()
                            || view.hover_axis.is_some())
                    {
                        view.cursor = None;
                        view.hovered = None;
                        view.hover_axis = None;
                        cx.notify();
                    }
                }))
                .on_key_down(cx.listener(|view, event: &KeyDownEvent, _window, cx| {
                    view.handle_key(event, cx);
                }))
                .on_scroll_wheel({
                    cx.listener(|view, event: &ScrollWheelEvent, _window, cx| {
                        view.handle_scroll(event, cx);
                    })
                });
        }
        let content = chart_area;
        let entity = cx.entity();
        let chart_area = content.context_menu(move |mut menu, _window, cx| {
            let (axis, magnet) = entity.read_with(cx, |view, _| {
                let layout = view.runtime.borrow().layout.clone();
                let point = view.last_right_click.unwrap_or_default();
                (axis_handle(point, &layout, 2.0), view.magnet)
            });

            match axis {
                Some(AxisHandle::Price) => {
                    menu = menu.item(scale_item(
                        "Auto price range",
                        entity.clone(),
                        PriceScaleAction::Auto,
                    ));
                    menu = menu.separator();
                    for (label, action) in [
                        ("Linear scale", PriceScaleAction::Linear),
                        ("Log scale", PriceScaleAction::Log),
                        ("Percent scale", PriceScaleAction::Percent),
                        ("Invert scale", PriceScaleAction::Invert),
                    ] {
                        menu = menu.item(scale_item(label, entity.clone(), action));
                    }
                    menu
                }
                Some(AxisHandle::Time) => {
                    menu = menu.item(item_with("Fit visible data", entity.clone(), |view, cx| {
                        view.fit_visible(cx)
                    }));
                    menu = menu.item(item_with("Reset zoom", entity.clone(), |view, cx| {
                        view.reset_view(cx)
                    }));
                    menu.item(item_with("Jump to live", entity.clone(), |view, cx| {
                        view.scroll_to_latest(cx)
                    }))
                }
                None => {
                    menu = menu.item(item_with("Fit visible data", entity.clone(), |view, cx| {
                        view.fit_visible(cx)
                    }));
                    menu = menu.item(item_with("Reset chart", entity.clone(), |view, cx| {
                        view.reset_view(cx);
                        view.reset_price_scale(cx);
                    }));
                    menu = menu.item(PopupMenuItem::new("Magnet").checked(magnet).on_click({
                        let entity = entity.clone();
                        move |_, _, cx| {
                            let _ = entity.update(cx, |view, cx| {
                                view.set_magnet(!view.magnet, cx);
                            });
                        }
                    }));
                    menu = menu.separator();
                    menu = menu.item(scale_item(
                        "Auto price range",
                        entity.clone(),
                        PriceScaleAction::Auto,
                    ));
                    for (label, action) in [
                        ("Linear scale", PriceScaleAction::Linear),
                        ("Log scale", PriceScaleAction::Log),
                        ("Percent scale", PriceScaleAction::Percent),
                        ("Invert scale", PriceScaleAction::Invert),
                    ] {
                        menu = menu.item(scale_item(label, entity.clone(), action));
                    }
                    menu
                }
            }
        });
        let chip = div()
            .id("chart_scale_chip")
            .absolute()
            .top(px(4.0))
            .right(px(4.0))
            .px(px(6.0))
            .py(px(2.0))
            .rounded(px(3.0))
            .border_1()
            .border_color(palette.grid_major)
            .bg(palette.flag_bg.opacity(0.92))
            .text_size(px(9.0))
            .text_color(palette.axis_text_active)
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_view, _event: &MouseDownEvent, _window, cx| {
                    cx.stop_propagation();
                }),
            )
            .on_click(cx.listener(|view, _event: &gpui::ClickEvent, _window, cx| {
                view.cycle_price_scale_mode(cx);
            }))
            .child(scale_chip);
        let chart_area = chart_area.child(chip);

        let rail = if matches!(mode, InteractionMode::Full) {
            div()
                .id("chart_v2_rail")
                .w(px(34.0))
                .h_full()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(4.0))
                .pt(px(34.0))
                .bg(palette.rail)
                .border_r_1()
                .border_color(palette.grid_major)
                .child(rail_button(
                    "chart_rail_cursor",
                    "✥",
                    false,
                    &palette,
                    cx.listener(|view, _: &gpui::ClickEvent, _window, cx| {
                        view.cursor = None;
                        view.hovered = None;
                        cx.notify();
                    }),
                ))
                .child(rail_button(
                    "chart_rail_magnet",
                    "M",
                    self.magnet,
                    &palette,
                    cx.listener(|view, _: &gpui::ClickEvent, _window, cx| {
                        view.set_magnet(!view.magnet, cx);
                    }),
                ))
                .child(rail_button(
                    "chart_rail_fit",
                    "F",
                    false,
                    &palette,
                    cx.listener(|view, _: &gpui::ClickEvent, _window, cx| {
                        view.fit_visible(cx);
                    }),
                ))
                .child(rail_button(
                    "chart_rail_reset",
                    "↺",
                    false,
                    &palette,
                    cx.listener(|view, _: &gpui::ClickEvent, _window, cx| {
                        view.reset_view(cx);
                        view.reset_price_scale(cx);
                    }),
                ))
        } else {
            div().id("chart_v2_rail_empty").w(px(0.0)).h_full()
        };

        div().size_full().flex().child(rail).child(chart_area)
    }
}

fn rail_button(
    id: &'static str,
    label: &'static str,
    active: bool,
    palette: &ChartPalette,
    handler: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let mut button = div()
        .id(id)
        .size(px(24.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.0))
        .text_size(px(11.0))
        .text_color(palette.axis_text)
        .cursor_pointer()
        .hover(|style| style.bg(palette.flag_bg))
        .on_click(handler)
        .child(label);
    if active {
        button = button
            .bg(palette.flag_bg.opacity(0.90))
            .text_color(palette.axis_text_active);
    }
    button
}

fn price_range(bars: &[Ohlcv], visible: std::ops::Range<usize>) -> (f64, f64) {
    let start = visible.start.min(bars.len());
    let end = visible.end.min(bars.len());
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    for bar in &bars[start..end] {
        if bar.low.is_finite() {
            min = min.min(bar.low);
        }
        if bar.high.is_finite() {
            max = max.max(bar.high);
        }
    }
    if !min.is_finite() || !max.is_finite() {
        return (0.0, 1.0);
    }
    if max <= min {
        let pad = if min.abs() > 1.0 {
            min.abs() * 0.001
        } else {
            0.5
        };
        return (min - pad, min + pad);
    }
    let pad = (max - min) * 0.08;
    (min - pad, max + pad)
}

#[allow(clippy::too_many_arguments)]
fn build_time_ticks(
    scene: &mut Scene,
    layout: &ChartLayout,
    snapshot: &ChartSnapshot,
    viewport: &Viewport,
    total: usize,
    limits: &ViewportLimits,
    options: &ChartOptions,
    palette: &ChartPalette,
    measure: &mut dyn FnMut(&str) -> f32,
) {
    if total == 0 || snapshot.timestamps.is_empty() || layout.plot.is_empty() {
        return;
    }
    let slots = viewport.slots(total, limits);
    let slot_count = (slots.end - slots.start).max(1) as usize;
    let step = (slot_count / options.target_time_ticks.max(1)).max(1);
    let mut ticks = Vec::new();
    let mut last_day = i64::MIN;
    for slot in (slots.start..slots.end).step_by(step) {
        let Some(timestamp) = timestamp_at_slot(snapshot, slot as f64) else {
            continue;
        };
        let label = options.format_time(timestamp);
        let width = measure(label.as_ref());
        let day = timestamp.div_euclid(86_400_000);
        let major = day != last_day;
        last_day = day;
        ticks.push(axes::TimeTick {
            x: viewport.index_to_x(slot as f64, layout.plot),
            label,
            width,
            major,
        });
    }
    axes::build_time_axis(scene, layout, &ticks, palette);
}

#[allow(clippy::too_many_arguments)]
fn build_readout(
    scene: &mut Scene,
    layout: &ChartLayout,
    snapshot: &ChartSnapshot,
    cursor: Option<CursorInfo>,
    latest_visible: bool,
    magnet: bool,
    options: &ChartOptions,
    palette: &ChartPalette,
) {
    let total = snapshot.len();
    if total == 0 || layout.plot.is_empty() {
        return;
    }
    let title = options.title.as_ref();
    let prefix = if title.is_empty() {
        String::new()
    } else {
        format!("{title}   ")
    };

    let magnet_badge = if magnet { "  · MAGNET" } else { "" };
    let countdown = if latest_visible && snapshot.interval_ms > 0 {
        snapshot
            .timestamps
            .last()
            .copied()
            .and_then(|last| {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|duration| duration.as_millis() as i64)
                    .unwrap_or(0);
                let remaining = last + snapshot.interval_ms - now;
                (remaining > 0).then(|| format!("   CLOSE {}", format_countdown(remaining)))
            })
            .unwrap_or_default()
    } else {
        String::new()
    };

    let (text, color) = match cursor {
        Some(cursor) if cursor.index.is_none() => {
            let time = cursor
                .timestamp_ms
                .map(|timestamp| options.format_time(timestamp))
                .unwrap_or_else(|| "--".into());
            (
                format!(
                    "{prefix}{time}   price {}   O --  H --  L --  C --  V --  Δ --{countdown}{magnet_badge}",
                    options.format_price(cursor.price)
                ),
                palette.axis_text_active,
            )
        }
        _ => {
            let index = cursor
                .and_then(|cursor| cursor.index)
                .unwrap_or(total - 1)
                .min(total - 1);
            let bar = snapshot.bars[index];
            let time = snapshot
                .timestamps
                .get(index)
                .map(|timestamp| options.format_time(*timestamp))
                .unwrap_or_else(|| "--".into());
            let previous_close = if index > 0 {
                snapshot.bars[index - 1].close
            } else {
                bar.open
            };
            let change_pct = if previous_close.abs() > f64::EPSILON {
                (bar.close - previous_close) / previous_close * 100.0
            } else {
                0.0
            };
            let change_sign = if change_pct > 0.0 { "+" } else { "" };
            let color = if bar.close >= bar.open {
                palette.up
            } else {
                palette.down
            };
            (
                format!(
                    "{prefix}{time}   O {}  H {}  L {}  C {}  V {}  Δ {change_sign}{change_pct:.2}%{countdown}{magnet_badge}",
                    options.format_price(bar.open),
                    options.format_price(bar.high),
                    options.format_price(bar.low),
                    options.format_price(bar.close),
                    format_compact(bar.volume),
                ),
                color,
            )
        }
    };
    if text.is_empty() {
        return;
    }
    let text: SharedString = text.into();
    let width =
        (text.chars().count() as f32 * 6.4 + 20.0).clamp(1.0, (layout.plot.w - 16.0).max(1.0));
    scene.push(
        Layer::Chrome,
        Primitive::Rect {
            rect: Rect::new(layout.plot.x + 8.0, layout.plot.y + 6.0, width, 22.0),
            color: palette.flag_bg.opacity(0.82),
            radius: 3.0,
        },
    );
    scene.push(
        Layer::Chrome,
        Primitive::Glyph {
            text,
            origin: Point::new(layout.plot.x + 16.0, layout.plot.y + 11.0),
            font_size: 11.0,
            weight: gpui::FontWeight::NORMAL,
            color,
            align: gpui::TextAlign::Left,
        },
    );
}

#[derive(Clone, Copy)]
enum PriceScaleAction {
    Auto,
    Linear,
    Log,
    Percent,
    Invert,
}

fn scale_item(
    label: impl Into<SharedString>,
    entity: gpui::Entity<ChartView>,
    action: PriceScaleAction,
) -> PopupMenuItem {
    PopupMenuItem::new(label).on_click(move |_, _, cx| {
        let _ = entity.update(cx, |view, cx| match action {
            PriceScaleAction::Auto => view.reset_price_scale(cx),
            PriceScaleAction::Linear => view.set_price_scale_mode(PriceScaleMode::Linear, cx),
            PriceScaleAction::Log => view.set_price_scale_mode(PriceScaleMode::Log, cx),
            PriceScaleAction::Percent => {
                let (min, max) = view.runtime.borrow().price_range;
                let anchor = if min.is_finite() && max.is_finite() && max > min {
                    (min + max) * 0.5
                } else {
                    1.0
                };
                view.set_price_scale_mode(PriceScaleMode::percent(anchor), cx);
            }
            PriceScaleAction::Invert => {
                view.set_price_scale_mode(PriceScaleMode::InvertedLinear, cx)
            }
        });
    })
}

fn item_with(
    label: impl Into<SharedString>,
    entity: gpui::Entity<ChartView>,
    action: fn(&mut ChartView, &mut Context<ChartView>),
) -> PopupMenuItem {
    PopupMenuItem::new(label).on_click(move |_, _, cx| {
        let _ = entity.update(cx, action);
    })
}

fn format_countdown(remaining_ms: i64) -> String {
    let seconds = (remaining_ms.max(0) / 1_000) as u64;
    if seconds >= 3_600 {
        format!("{}h {:02}m", seconds / 3_600, (seconds % 3_600) / 60)
    } else {
        format!("{:02}:{:02}", seconds / 60, seconds % 60)
    }
}

fn format_compact(value: f64) -> String {
    if !value.is_finite() {
        return "--".to_string();
    }
    let value = value.abs();
    if value >= 1_000_000_000.0 {
        format!("{:.1}B", value / 1_000_000_000.0)
    } else if value >= 1_000_000.0 {
        format!("{:.1}M", value / 1_000_000.0)
    } else if value >= 1_000.0 {
        format!("{:.1}K", value / 1_000.0)
    } else {
        format!("{value:.0}")
    }
}

fn build_crosshair(
    scene: &mut Scene,
    layout: &ChartLayout,
    cursor: CursorInfo,
    options: &ChartOptions,
    palette: &ChartPalette,
) {
    let plot = layout.plot;
    if !plot.contains(cursor.pos) {
        return;
    }
    scene.push(
        Layer::Cursor,
        Primitive::Segment {
            from: Point::new(cursor.pos.x, plot.y),
            to: Point::new(cursor.pos.x, plot.bottom()),
            width: 1.0,
            color: palette.crosshair,
            dash: None,
        },
    );
    scene.push(
        Layer::Cursor,
        Primitive::Segment {
            from: Point::new(plot.x, cursor.pos.y),
            to: Point::new(plot.right(), cursor.pos.y),
            width: 1.0,
            color: palette.crosshair,
            dash: None,
        },
    );

    let price_label = options.format_price(cursor.price);
    let label_y = cursor.pos.y - 8.0;
    let box_width = (price_label.chars().count() as f32 * 6.2 + 10.0).clamp(48.0, 96.0);
    scene.push(
        Layer::Cursor,
        Primitive::Rect {
            rect: Rect::new(plot.right() + 2.0, label_y - 2.0, box_width, 18.0),
            color: palette.flag_bg,
            radius: 3.0,
        },
    );
    scene.push(
        Layer::Cursor,
        Primitive::Glyph {
            text: price_label,
            origin: Point::new(plot.right() + 7.0, label_y),
            font_size: 11.0,
            weight: gpui::FontWeight::MEDIUM,
            color: palette.flag_fg,
            align: gpui::TextAlign::Left,
        },
    );

    if cursor.index.is_some() {
        scene.push(
            Layer::Cursor,
            Primitive::Dot {
                center: cursor.pos,
                radius: 2.0,
                color: palette.measure_stroke,
            },
        );
    }

    if let Some(timestamp) = cursor.timestamp_ms {
        let label = options.format_time(timestamp);
        let width = (label.chars().count() as f32 * 6.2 + 12.0).clamp(48.0, 140.0);
        let x = (cursor.pos.x - width * 0.5).clamp(plot.x, (plot.right() - width).max(plot.x));
        let y = plot.bottom() + 1.0;
        scene.push(
            Layer::Cursor,
            Primitive::Rect {
                rect: Rect::new(x, y, width, 17.0),
                color: palette.flag_bg,
                radius: 3.0,
            },
        );
        scene.push(
            Layer::Cursor,
            Primitive::Glyph {
                text: label,
                origin: Point::new(x + 6.0, y + 3.0),
                font_size: 10.0,
                weight: gpui::FontWeight::MEDIUM,
                color: palette.flag_fg,
                align: gpui::TextAlign::Left,
            },
        );
    }
}

fn median_interval_ms(timestamps: &[i64]) -> i64 {
    if timestamps.len() < 2 {
        return 60_000;
    }
    let mut diffs: Vec<i64> = timestamps
        .windows(2)
        .filter_map(|pair| {
            let diff = pair[1] - pair[0];
            (diff > 0).then_some(diff)
        })
        .collect();
    if diffs.is_empty() {
        return 60_000;
    }
    diffs.sort_unstable();
    diffs[diffs.len() / 2].max(1)
}

fn timestamp_at_slot(snapshot: &ChartSnapshot, slot: f64) -> Option<i64> {
    let total = snapshot.len();
    if total == 0 || !slot.is_finite() {
        return None;
    }
    let interval = snapshot.interval_ms.max(1);
    let first = *snapshot.timestamps.first()?;
    let last = *snapshot.timestamps.last()?;
    let last_slot = (total - 1) as f64;
    let rounded = slot.round() as isize;
    if rounded < 0 {
        Some(first.saturating_add((slot * interval as f64) as i64))
    } else if rounded >= total as isize {
        Some(last.saturating_add(((slot - last_slot) * interval as f64) as i64))
    } else {
        snapshot.timestamps.get(rounded as usize).copied()
    }
}

fn edge_price_range(bars: &[Ohlcv], prefer_last: bool) -> (f64, f64) {
    if bars.is_empty() {
        return (0.0, 1.0);
    }
    let window = 100.min(bars.len());
    let slice = if prefer_last {
        &bars[bars.len() - window..]
    } else {
        &bars[..window]
    };
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    for bar in slice {
        if bar.low.is_finite() {
            min = min.min(bar.low);
        }
        if bar.high.is_finite() {
            max = max.max(bar.high);
        }
    }
    if !min.is_finite() || !max.is_finite() {
        return (0.0, 1.0);
    }
    if max <= min {
        let pad = if min.abs() > 1.0 {
            min.abs() * 0.005
        } else {
            0.5
        };
        return (min - pad, min + pad);
    }
    let pad = (max - min) * 0.05;
    (min - pad, max + pad)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bars() -> Vec<Ohlcv> {
        (0..20)
            .map(|i| {
                let base = 100.0 + i as f64;
                Ohlcv::new(base, base + 2.0, base - 2.0, base + 1.0, 1.0)
            })
            .collect()
    }

    #[test]
    fn snapshot_truncates_mismatched_lengths() {
        let bars: Arc<[Ohlcv]> = Arc::from(bars());
        let times: Arc<[i64]> = Arc::from((0..10).map(|i| i * 60_000).collect::<Vec<_>>());
        let snapshot = ChartSnapshot::new(bars, times, 1);
        assert_eq!(snapshot.len(), 10);
    }

    #[test]
    fn price_range_includes_padding() {
        let bars = bars();
        let (min, max) = price_range(&bars, 0..bars.len());
        assert!(min < 100.0);
        assert!(max > 119.0);
    }

    #[test]
    fn price_range_handles_empty_and_flat() {
        assert_eq!(price_range(&[], 0..0), (0.0, 1.0));
        let flat = vec![Ohlcv::new(10.0, 10.0, 10.0, 10.0, 1.0)];
        let (min, max) = price_range(&flat, 0..1);
        assert!(max > min);
    }

    #[test]
    fn timestamps_extrapolate_into_blank_time() {
        let bars: Arc<[Ohlcv]> = Arc::from(bars());
        let times: Arc<[i64]> = Arc::from(vec![0_i64, 60_000, 120_000]);
        let snapshot = ChartSnapshot::new(Arc::from(&bars[..3]), times, 1);
        assert_eq!(snapshot.interval_ms, 60_000);
        assert_eq!(timestamp_at_slot(&snapshot, -1.0), Some(-60_000));
        assert_eq!(timestamp_at_slot(&snapshot, 1.2), Some(60_000));
        assert_eq!(timestamp_at_slot(&snapshot, 3.0), Some(180_000));
    }

    #[test]
    fn countdown_formatting() {
        assert_eq!(format_countdown(65_000), "01:05");
        assert_eq!(format_countdown(0), "00:00");
        assert_eq!(format_countdown(3_665_000), "1h 01m");
    }

    #[gpui::test]
    fn scale_chip_cycles_scale_modes(cx: &mut gpui::TestAppContext) {
        let bars: Arc<[Ohlcv]> = Arc::from(bars());
        let times: Arc<[i64]> = Arc::from((0..20).map(|i| i * 60_000).collect::<Vec<_>>());
        let snapshot = ChartSnapshot::new(bars, times, 1);
        let (view, cx) = cx.add_window_view(|_window, cx| {
            ChartView::new(
                snapshot,
                ChartOptions::default(),
                ChartPalette::obsidian(),
                cx,
            )
        });
        view.update(cx, |view, cx| view.cycle_price_scale_mode(cx));
        assert!(matches!(
            cx.cx.update(|app| view.read(app).price_scale_mode()),
            PriceScaleMode::Log
        ));
        view.update(cx, |view, cx| view.cycle_price_scale_mode(cx));
        assert!(matches!(
            cx.cx.update(|app| view.read(app).price_scale_mode()),
            PriceScaleMode::Percent { .. }
        ));
    }

    #[gpui::test]
    fn alt_magnet_snaps_to_ohlc(cx: &mut gpui::TestAppContext) {
        let bars_vec = bars();
        let bars: Arc<[Ohlcv]> = Arc::from(bars_vec.clone());
        let times: Arc<[i64]> = Arc::from((0..20).map(|i| i * 60_000).collect::<Vec<_>>());
        let snapshot = ChartSnapshot::new(bars, times, 1);
        let (view, cx) = cx.add_window_view(|_window, cx| {
            ChartView::new(
                snapshot,
                ChartOptions::default(),
                ChartPalette::obsidian(),
                cx,
            )
        });
        let (layout, viewport) = cx.cx.update(|app| {
            let view = view.read(app);
            (view.layout(), view.viewport())
        });
        let x = viewport.index_to_x(19.0, layout.plot);
        let point = gpui::point(px(x), px(layout.plot.center().y));
        cx.simulate_mouse_move(
            point,
            None,
            gpui::Modifiers {
                alt: true,
                ..Default::default()
            },
        );
        let cursor = cx
            .cx
            .update(|app| view.read(app).cursor())
            .expect("cursor should exist");
        let index = cursor.index.expect("cursor should be over a candle");
        let bar = bars_vec[index];
        let candidates = [bar.open, bar.high, bar.low, bar.close];
        assert!(
            candidates
                .iter()
                .any(|candidate| (candidate - cursor.price).abs() < 1e-9),
            "cursor price {} was not one of {:?}",
            cursor.price,
            candidates
        );
    }

    #[gpui::test]
    fn crosshair_is_free_in_blank_future(cx: &mut gpui::TestAppContext) {
        let bars: Arc<[Ohlcv]> = Arc::from(bars());
        let times: Arc<[i64]> = Arc::from((0..20).map(|i| i * 60_000).collect::<Vec<_>>());
        let snapshot = ChartSnapshot::new(bars, times, 1);
        let (view, cx) = cx.add_window_view(|_window, cx| {
            ChartView::new(
                snapshot,
                ChartOptions::default(),
                ChartPalette::obsidian(),
                cx,
            )
        });
        view.update(cx, |view, cx| {
            let limits = ViewportLimits::default();
            let (_, max_start) = view.viewport().bounds(20, &limits);
            view.set_viewport(Viewport::new(max_start, 10.0), cx);
        });
        cx.run_until_parked();
        let point = gpui::point(px(400.0), px(300.0));
        cx.simulate_mouse_move(point, None, gpui::Modifiers::default());
        let cursor = cx
            .cx
            .update(|app| view.read(app).cursor())
            .expect("cursor should exist over the plot");
        assert!(
            cursor.index.is_none(),
            "blank future must not snap to a candle"
        );
        assert!(cursor.timestamp_ms.is_some());
        assert!(cursor.price.is_finite());
    }

    #[gpui::test]
    fn dragging_price_rail_switches_to_manual(cx: &mut gpui::TestAppContext) {
        let bars: Arc<[Ohlcv]> = Arc::from(bars());
        let times: Arc<[i64]> = Arc::from((0..20).map(|i| i * 60_000).collect::<Vec<_>>());
        let snapshot = ChartSnapshot::new(bars, times, 1);
        let (view, cx) = cx.add_window_view(|_window, cx| {
            ChartView::new(
                snapshot,
                ChartOptions::default(),
                ChartPalette::obsidian(),
                cx,
            )
        });
        let layout = cx.cx.update(|app| view.read(app).layout());
        assert!(matches!(
            cx.cx.update(|app| view.read(app).price_axis_state()),
            PriceAxisState::Auto
        ));
        let start = gpui::point(
            px(layout.price_rail.center().x),
            px(layout.price_rail.center().y),
        );
        let anchor_price = {
            let (range, mode) = cx.cx.update(|app| {
                let view = view.read(app);
                let runtime = view.runtime.borrow();
                (runtime.price_range, runtime.scale_mode)
            });
            PriceScale::new(mode, range.0, range.1, layout.plot).to_price(start.y.as_f32())
        };
        cx.simulate_mouse_down(start, MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_move(
            gpui::point(start.x, start.y - px(60.0)),
            Some(MouseButton::Left),
            gpui::Modifiers::default(),
        );
        cx.simulate_mouse_up(
            gpui::point(start.x, start.y - px(60.0)),
            MouseButton::Left,
            gpui::Modifiers::default(),
        );
        let manual = cx.cx.update(|app| view.read(app).price_axis_state());
        let PriceAxisState::Manual { min, max } = manual else {
            panic!("expected manual price range");
        };
        let anchor_y =
            PriceScale::new(PriceScaleMode::Linear, min, max, layout.plot).to_y(anchor_price);
        assert!(
            (anchor_y - start.y.as_f32()).abs() < 1.0,
            "{anchor_y} vs {}",
            start.y.as_f32()
        );
    }

    #[gpui::test]
    fn dragging_time_rail_changes_the_span(cx: &mut gpui::TestAppContext) {
        let bars: Arc<[Ohlcv]> = Arc::from(bars());
        let times: Arc<[i64]> = Arc::from((0..20).map(|i| i * 60_000).collect::<Vec<_>>());
        let snapshot = ChartSnapshot::new(bars, times, 1);
        let (view, cx) = cx.add_window_view(|_window, cx| {
            ChartView::new(
                snapshot,
                ChartOptions::default(),
                ChartPalette::obsidian(),
                cx,
            )
        });
        let layout = cx.cx.update(|app| view.read(app).layout());
        let before_viewport = cx.cx.update(|app| view.read(app).viewport());
        let before = before_viewport.span;
        let start = gpui::point(
            px(layout.time_rail.center().x),
            px(layout.time_rail.center().y),
        );
        let anchor_before = before_viewport.x_to_position(start.x.as_f32(), layout.plot);
        cx.simulate_mouse_down(start, MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_move(
            gpui::point(start.x + px(100.0), start.y),
            Some(MouseButton::Left),
            gpui::Modifiers::default(),
        );
        cx.simulate_mouse_up(
            gpui::point(start.x + px(100.0), start.y),
            MouseButton::Left,
            gpui::Modifiers::default(),
        );
        let after_viewport = cx.cx.update(|app| view.read(app).viewport());
        let after = after_viewport.span;
        assert!(after > before, "{before} -> {after}");
        let anchor_after = after_viewport.x_to_position(start.x.as_f32(), layout.plot);
        assert!(
            (anchor_before - anchor_after).abs() < 1e-6,
            "{anchor_before} vs {anchor_after}"
        );
    }

    #[gpui::test]
    fn double_clicking_price_rail_resets_to_auto(cx: &mut gpui::TestAppContext) {
        let bars: Arc<[Ohlcv]> = Arc::from(bars());
        let times: Arc<[i64]> = Arc::from((0..20).map(|i| i * 60_000).collect::<Vec<_>>());
        let snapshot = ChartSnapshot::new(bars, times, 1);
        let (view, cx) = cx.add_window_view(|_window, cx| {
            ChartView::new(
                snapshot,
                ChartOptions::default(),
                ChartPalette::obsidian(),
                cx,
            )
        });
        view.update(cx, |view, cx| {
            view.set_price_axis_state(
                PriceAxisState::Manual {
                    min: 10.0,
                    max: 20.0,
                },
                cx,
            );
        });
        let layout = cx.cx.update(|app| view.read(app).layout());
        cx.simulate_event(MouseDownEvent {
            position: gpui::point(
                px(layout.price_rail.center().x),
                px(layout.price_rail.center().y),
            ),
            modifiers: gpui::Modifiers::default(),
            button: MouseButton::Left,
            click_count: 2,
            first_mouse: false,
        });
        assert!(matches!(
            cx.cx.update(|app| view.read(app).price_axis_state()),
            PriceAxisState::Auto
        ));
    }

    #[gpui::test]
    fn a_fast_release_starts_inertia(cx: &mut gpui::TestAppContext) {
        let bars: Arc<[Ohlcv]> = Arc::from(bars());
        let times: Arc<[i64]> = Arc::from((0..20).map(|i| i * 60_000).collect::<Vec<_>>());
        let snapshot = ChartSnapshot::new(bars, times, 1);
        let (view, cx) = cx.add_window_view(|_window, cx| {
            ChartView::new(
                snapshot,
                ChartOptions::default(),
                ChartPalette::obsidian(),
                cx,
            )
        });
        let start = gpui::point(px(600.0), px(300.0));
        let end = gpui::point(px(200.0), px(300.0));
        cx.simulate_mouse_down(start, MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_move(end, Some(MouseButton::Left), gpui::Modifiers::default());
        cx.simulate_mouse_up(end, MouseButton::Left, gpui::Modifiers::default());
        assert!(cx.cx.update(|app| view.read(app).is_animating()));
    }

    #[gpui::test]
    fn dragging_can_reach_blank_future_without_snapping_back(cx: &mut gpui::TestAppContext) {
        let bars: Arc<[Ohlcv]> = Arc::from(bars());
        let times: Arc<[i64]> = Arc::from((0..20).map(|i| i * 60_000).collect::<Vec<_>>());
        let snapshot = ChartSnapshot::new(bars, times, 1);
        let (view, cx) = cx.add_window_view(|_window, cx| {
            ChartView::new(
                snapshot,
                ChartOptions::default(),
                ChartPalette::obsidian(),
                cx,
            )
        });
        let limits = ViewportLimits::default();
        view.update(cx, |view, cx| {
            let (_, max_start) = view.viewport().bounds(20, &limits);
            view.set_viewport(Viewport::new(max_start - 1.0, 12.0), cx);
        });
        cx.run_until_parked();
        let before = cx.cx.update(|app| view.read(app).viewport().start);
        cx.simulate_mouse_down(
            gpui::point(px(600.0), px(300.0)),
            MouseButton::Left,
            gpui::Modifiers::default(),
        );
        cx.simulate_mouse_move(
            gpui::point(px(200.0), px(300.0)),
            Some(MouseButton::Left),
            gpui::Modifiers::default(),
        );
        cx.simulate_mouse_up(
            gpui::point(px(200.0), px(300.0)),
            MouseButton::Left,
            gpui::Modifiers::default(),
        );
        let after = cx.cx.update(|app| view.read(app).viewport().start);
        assert!(after > before, "{before} -> {after}");
        assert!(after > 20.0, "drag should reach blank future: {after}");
    }

    #[gpui::test]
    fn panning_changes_the_viewport(cx: &mut gpui::TestAppContext) {
        let bars: Arc<[Ohlcv]> = Arc::from(bars());
        let times: Arc<[i64]> = Arc::from((0..20).map(|i| i * 60_000).collect::<Vec<_>>());
        let snapshot = ChartSnapshot::new(bars, times, 1);
        let (view, cx) = cx.add_window_view(|_window, cx| {
            ChartView::new(
                snapshot,
                ChartOptions::default(),
                ChartPalette::obsidian(),
                cx,
            )
        });
        let before = cx.cx.update(|app| view.read(app).viewport().start);
        let start = gpui::point(px(400.0), px(300.0));
        let end = gpui::point(px(500.0), px(300.0));
        cx.simulate_mouse_down(start, MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_move(end, Some(MouseButton::Left), gpui::Modifiers::default());
        cx.simulate_mouse_up(end, MouseButton::Left, gpui::Modifiers::default());
        let after = cx.cx.update(|app| view.read(app).viewport().start);
        assert!((before - after).abs() > 0.1, "{before} vs {after}");
    }

    #[gpui::test]
    fn prepending_history_preserves_the_visible_candles(cx: &mut gpui::TestAppContext) {
        let old_bars: Arc<[Ohlcv]> = Arc::from(
            (10..20)
                .map(|i| Ohlcv::new(i as f64, i as f64 + 1.0, i as f64 - 1.0, i as f64, 1.0))
                .collect::<Vec<_>>(),
        );
        let old_times: Arc<[i64]> = Arc::from((10..20).map(|i| i * 60_000).collect::<Vec<_>>());
        let snapshot = ChartSnapshot::new(old_bars, old_times, 1);
        let (view, cx) = cx.add_window_view(|_window, cx| {
            ChartView::new(
                snapshot,
                ChartOptions::default(),
                ChartPalette::obsidian(),
                cx,
            )
        });
        view.update(cx, |view, cx| {
            view.set_viewport(Viewport::new(5.0, 10.0), cx);
        });
        let before = cx.cx.update(|app| view.read(app).viewport().start);

        let combined_bars: Arc<[Ohlcv]> = Arc::from(
            (0..20)
                .map(|i| Ohlcv::new(i as f64, i as f64 + 1.0, i as f64 - 1.0, i as f64, 1.0))
                .collect::<Vec<_>>(),
        );
        let combined_times: Arc<[i64]> = Arc::from((0..20).map(|i| i * 60_000).collect::<Vec<_>>());
        let combined = ChartSnapshot::new(combined_bars, combined_times, 2);
        view.update(cx, |view, cx| view.set_snapshot(combined, cx));
        let after = cx.cx.update(|app| view.read(app).viewport().start);
        assert!(
            (after - (before + 10.0)).abs() < 0.01,
            "{before} -> {after}"
        );
    }

    #[gpui::test]
    fn keyboard_zoom_changes_the_span(cx: &mut gpui::TestAppContext) {
        let bars: Arc<[Ohlcv]> = Arc::from(bars());
        let times: Arc<[i64]> = Arc::from((0..20).map(|i| i * 60_000).collect::<Vec<_>>());
        let snapshot = ChartSnapshot::new(bars, times, 1);
        let (view, cx) = cx.add_window_view(|_window, cx| {
            ChartView::new(
                snapshot,
                ChartOptions::default(),
                ChartPalette::obsidian(),
                cx,
            )
        });
        let before = cx.cx.update(|app| view.read(app).viewport().span);
        let point = gpui::point(px(400.0), px(300.0));
        cx.simulate_mouse_down(point, MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_up(point, MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_keystrokes("-");
        let after = cx.cx.update(|app| view.read(app).viewport().span);
        assert!(after > before, "{before} vs {after}");
    }

    #[gpui::test]
    fn chart_view_renders_without_panicking(cx: &mut gpui::TestAppContext) {
        let bars: Arc<[Ohlcv]> = Arc::from(bars());
        let times: Arc<[i64]> = Arc::from((0..20).map(|i| i * 60_000).collect::<Vec<_>>());
        let snapshot = ChartSnapshot::new(bars, times, 1);
        let (view, cx) = cx.add_window_view(|_window, cx| {
            ChartView::new(
                snapshot,
                ChartOptions::default(),
                ChartPalette::obsidian(),
                cx,
            )
        });
        let epoch = cx.cx.update(|app| view.read(app).snapshot_epoch());
        assert_eq!(epoch, 1);
    }

    #[test]
    fn snapshot_epoch_is_stable() {
        let bars: Arc<[Ohlcv]> = Arc::from(bars());
        let times: Arc<[i64]> = Arc::from((0..20).map(|i| i * 60_000).collect::<Vec<_>>());
        let a = ChartSnapshot::new(bars.clone(), times.clone(), 7);
        let b = ChartSnapshot::new(bars, times, 7);
        assert_eq!(a.epoch, b.epoch);
        assert_eq!(a.len(), b.len());
    }
}
