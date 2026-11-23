//! Built-in chart marks: candles, grid, axes.
//!
//! Each builder appends primitives to a [`Scene`] and never touches GPUI paint
//! APIs. This keeps geometry testable and lets the painter batch the work.

pub mod axes;
pub mod candles;
pub mod grid;
pub mod volume;
