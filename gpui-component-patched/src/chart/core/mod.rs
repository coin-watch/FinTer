//! Pure chart engine primitives.
//!
//! Nothing in this module depends on GPUI. It is intentionally small and
//! testable so the interaction math can be verified without constructing a
//! window or an application context.

pub mod geometry;
pub mod gesture;
pub mod hit;
pub mod layout;
pub mod scale;
pub mod series;
pub mod spring;
pub mod time;
pub mod viewport;

pub use geometry::{Point, Rect, snap, snap_point};
pub use gesture::{
    DragClassifier, DragRelease, DragUpdate, VelocityTracker, drag_price_range, drag_time_span,
};
pub use hit::{AxisHandle, axis_handle, nearest_candle, pane_separator};
pub use layout::{
    AXIS_GAP, ChartLayout, LayoutOptions, PANE_GAP, PaneKind, PaneLayout, compute, price_rail_width,
};
pub use scale::{PriceScale, PriceScaleMode, Tick, nice_ticks};
pub use series::{EnvelopeBar, LodLevel, Ohlcv, build_envelope};
pub use spring::Spring;
pub use time::{
    TIME_INTERVALS_MS, align_down, align_up, choose_time_interval, timeframe_interval_ms,
    timestamp_at_buffer_index,
};
pub use viewport::{Viewport, ViewportLimits, ViewportOverflow};
