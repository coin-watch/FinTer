//! Z-order layers for the chart display list.
//!
//! Layers are painted in declaration order. Keeping the order explicit removes
//! the legacy pattern of deferring labels to the end of a single paint function.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(usize)]
pub enum Layer {
    Background = 0,
    Grid = 1,
    Series = 2,
    Overlays = 3,
    Chrome = 4,
    Cursor = 5,
    Tooltip = 6,
}

impl Layer {
    pub const COUNT: usize = 7;
    pub const ALL: [Layer; Self::COUNT] = [
        Layer::Background,
        Layer::Grid,
        Layer::Series,
        Layer::Overlays,
        Layer::Chrome,
        Layer::Cursor,
        Layer::Tooltip,
    ];

    pub fn index(self) -> usize {
        self as usize
    }
}
