#![allow(dead_code)]
mod axis;
mod configuration;
mod direction;
mod drag;
mod edge;
mod node;
mod region;
mod resize;
mod state;
mod style;
mod title_bar;

pub use axis::Axis;
pub use configuration::Configuration;
pub use direction::Direction;
pub use drag::{DragEvent, Target};
pub use edge::Edge;
pub use node::{Node, Pane, Split};
pub use region::Region;
pub use resize::ResizeEvent;
pub use state::State;
pub use style::{ActivePaneStyle, DividerStyle, DropTargetStyle, PaneGridStyle};
pub use title_bar::TitleBar;

use gpui::{
    div, px, AnyElement, Bounds, CursorStyle, Div, InteractiveElement, IntoElement, MouseButton,
    ParentElement, Pixels, Styled,
};
use std::marker::PhantomData;
use std::sync::Arc;

/// Content for a pane in the grid.
pub struct Content {
    body: AnyElement,
    title_bar: Option<AnyElement>,
}

impl Content {
    /// Creates new pane content.
    pub fn new(body: impl IntoElement) -> Self {
        Self {
            body: body.into_any_element(),
            title_bar: None,
        }
    }

    /// Sets the title bar for the pane.
    pub fn title_bar(mut self, title_bar: impl IntoElement) -> Self {
        self.title_bar = Some(title_bar.into_any_element());
        self
    }
}

/// Callback types for PaneGrid events.
type OnClickCallback = Arc<dyn Fn(Pane) + Send + Sync>;
type OnResizeCallback = Arc<dyn Fn(ResizeEvent) + Send + Sync>;
type OnDragCallback = Arc<dyn Fn(DragEvent) + Send + Sync>;

/// Builder for a PaneGrid component.
pub struct PaneGrid<T> {
    style: PaneGridStyle,
    on_click: Option<OnClickCallback>,
    on_resize: Option<OnResizeCallback>,
    on_drag: Option<OnDragCallback>,
    resize_threshold: Pixels,
    _phantom: PhantomData<T>,
}

impl<T> PaneGrid<T> {
    /// Creates a new PaneGrid builder.
    pub fn new() -> Self {
        Self {
            style: PaneGridStyle::default(),
            on_click: None,
            on_resize: None,
            on_drag: None,
            resize_threshold: px(10.0),
            _phantom: PhantomData,
        }
    }

    /// Sets the style for the pane grid.
    pub fn style(mut self, style: PaneGridStyle) -> Self {
        self.style = style;
        self
    }

    /// Sets the callback for pane click events.
    pub fn on_click<F>(mut self, f: F) -> Self
    where
        F: Fn(Pane) + Send + Sync + 'static,
    {
        self.on_click = Some(Arc::new(f));
        self
    }

    /// Sets the callback for resize events.
    pub fn on_resize<F>(mut self, threshold: Pixels, f: F) -> Self
    where
        F: Fn(ResizeEvent) + Send + Sync + 'static,
    {
        self.resize_threshold = threshold;
        self.on_resize = Some(Arc::new(f));
        self
    }

    /// Sets the callback for drag events.
    pub fn on_drag<F>(mut self, f: F) -> Self
    where
        F: Fn(DragEvent) + Send + Sync + 'static,
    {
        self.on_drag = Some(Arc::new(f));
        self
    }

    /// Builds the pane grid layout from the state tree.
    pub fn build<F>(self, state: &State<T>, build_content: F) -> Div
    where
        F: Fn(Pane, &T, bool) -> Content,
    {
        // Get the root node and render it
        if let Some(root) = state.root() {
            self.render_node_simple(root, state, &build_content)
        } else {
            // No root node, empty state
            div()
                .flex()
                .items_center()
                .justify_center()
                .child("Empty PaneGrid")
        }
    }

    /// Renders a node without explicit bounds (uses flex layout).
    fn render_node_simple<F>(&self, node: &Node, state: &State<T>, build_content: &F) -> Div
    where
        F: Fn(Pane, &T, bool) -> Content,
    {
        match node {
            Node::Pane(pane_id) => {
                let pane = *pane_id;
                let is_active = state.active().map(|active| active == pane).unwrap_or(false);

                if let Some(content_data) = state.get(pane) {
                    let content = build_content(pane, content_data, is_active);
                    let active_style = &self.style.active_pane;

                    let mut pane_div = div().flex_1().overflow_hidden();

                    if is_active {
                        pane_div = pane_div.border_2().border_color(active_style.border_color);
                    }

                    // Add click handler if callback is set
                    if let Some(on_click) = self.on_click.clone() {
                        pane_div = pane_div.on_mouse_down(
                            MouseButton::Left,
                            move |_event, _window, _cx| {
                                on_click(pane);
                            },
                        );
                    }

                    // Add title bar if present
                    if let Some(title_bar) = content.title_bar {
                        pane_div.flex().flex_col().child(title_bar).child(
                            div()
                                .flex_1()
                                .size_full()
                                .overflow_hidden()
                                .child(content.body),
                        )
                    } else {
                        pane_div.child(content.body)
                    }
                } else {
                    div().flex_1().child("Missing pane content")
                }
            }
            Node::Split {
                axis,
                ratio,
                id: split_id,
                a,
                b,
            } => {
                let split_axis = *axis;
                let split_ratio = *ratio;
                let split = *split_id;

                let divider_style = &self.style.divider;
                let thickness = divider_style.thickness;

                let child_a = self.render_node_simple(a, state, build_content);
                let child_b = self.render_node_simple(b, state, build_content);

                // Create divider with resize interaction
                let mut divider = match split_axis {
                    Axis::Vertical => div()
                        .flex_shrink_0()
                        .w(thickness)
                        .bg(divider_style.color)
                        .cursor(CursorStyle::ResizeLeftRight),
                    Axis::Horizontal => div()
                        .flex_shrink_0()
                        .h(thickness)
                        .bg(divider_style.color)
                        .cursor(CursorStyle::ResizeUpDown),
                };

                // Add resize handler if callback is set
                if let Some(on_resize) = self.on_resize.clone() {
                    divider =
                        divider.on_mouse_down(MouseButton::Left, move |_event, _window, _cx| {
                            // TODO: Implement drag tracking for resize
                            // For now, just trigger the callback with current ratio
                            on_resize(ResizeEvent::new(split, split_ratio));
                        });
                }

                // Container with both children and divider
                match split_axis {
                    Axis::Vertical => div()
                        .flex()
                        .flex_row()
                        .size_full()
                        .child(child_a.flex_basis(gpui::relative(split_ratio)))
                        .child(divider)
                        .child(child_b.flex_basis(gpui::relative(1.0 - split_ratio))),
                    Axis::Horizontal => div()
                        .flex()
                        .flex_col()
                        .size_full()
                        .child(child_a.flex_basis(gpui::relative(split_ratio)))
                        .child(divider)
                        .child(child_b.flex_basis(gpui::relative(1.0 - split_ratio))),
                }
            }
        }
    }

    /// Renders the layout tree recursively.
    fn render_node<F>(
        &self,
        node: &Node,
        state: &State<T>,
        bounds: Bounds<Pixels>,
        build_content: &F,
    ) -> Div
    where
        F: Fn(Pane, &T, bool) -> Content,
    {
        match node {
            Node::Pane(pane_id) => {
                let pane = *pane_id;
                let is_active = state.active().map(|active| active == pane).unwrap_or(false);

                if let Some(content_data) = state.get(pane) {
                    let content = build_content(pane, content_data, is_active);
                    let active_style = &self.style.active_pane;

                    let mut pane_div = div().size_full();

                    if is_active {
                        pane_div = pane_div.border_2().border_color(active_style.border_color);
                    }

                    // Add title bar if present
                    if let Some(title_bar) = content.title_bar {
                        pane_div = pane_div.flex().flex_col().child(title_bar).child(
                            div()
                                .flex_1()
                                .size_full()
                                .overflow_hidden()
                                .child(content.body),
                        );
                    } else {
                        pane_div = pane_div.child(content.body);
                    }

                    pane_div
                } else {
                    div().child("Missing pane content")
                }
            }
            Node::Split {
                axis,
                ratio,
                id: _split_id,
                a,
                b,
            } => {
                let split_axis = *axis;
                let split_ratio = *ratio;

                let divider_style = &self.style.divider;
                let thickness = divider_style.thickness;

                // Calculate bounds for the two child regions
                let (bounds_a, bounds_b) = match split_axis {
                    Axis::Vertical => {
                        let split_x = bounds.origin.x + bounds.size.width * split_ratio;
                        let bounds_a = Bounds {
                            origin: bounds.origin,
                            size: gpui::Size {
                                width: (split_x - bounds.origin.x) - thickness / 2.0,
                                height: bounds.size.height,
                            },
                        };
                        let bounds_b = Bounds {
                            origin: gpui::Point {
                                x: split_x + thickness / 2.0,
                                y: bounds.origin.y,
                            },
                            size: gpui::Size {
                                width: (bounds.origin.x + bounds.size.width)
                                    - split_x
                                    - thickness / 2.0,
                                height: bounds.size.height,
                            },
                        };
                        (bounds_a, bounds_b)
                    }
                    Axis::Horizontal => {
                        let split_y = bounds.origin.y + bounds.size.height * split_ratio;
                        let bounds_a = Bounds {
                            origin: bounds.origin,
                            size: gpui::Size {
                                width: bounds.size.width,
                                height: (split_y - bounds.origin.y) - thickness / 2.0,
                            },
                        };
                        let bounds_b = Bounds {
                            origin: gpui::Point {
                                x: bounds.origin.x,
                                y: split_y + thickness / 2.0,
                            },
                            size: gpui::Size {
                                width: bounds.size.width,
                                height: (bounds.origin.y + bounds.size.height)
                                    - split_y
                                    - thickness / 2.0,
                            },
                        };
                        (bounds_a, bounds_b)
                    }
                };

                let child_a = self.render_node(a, state, bounds_a, build_content);
                let child_b = self.render_node(b, state, bounds_b, build_content);

                // Create divider
                let divider = div()
                    .w(match split_axis {
                        Axis::Vertical => thickness,
                        Axis::Horizontal => Pixels::from(0.0),
                    })
                    .h(match split_axis {
                        Axis::Vertical => Pixels::from(0.0),
                        Axis::Horizontal => thickness,
                    })
                    .bg(divider_style.color);

                // Container with both children and divider
                match split_axis {
                    Axis::Vertical => div()
                        .flex()
                        .flex_row()
                        .size_full()
                        .child(child_a)
                        .child(divider)
                        .child(child_b),
                    Axis::Horizontal => div()
                        .flex()
                        .flex_col()
                        .size_full()
                        .child(child_a)
                        .child(divider)
                        .child(child_b),
                }
            }
        }
    }
}

impl<T> Default for PaneGrid<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// Creates a new PaneGrid builder.
pub fn pane_grid<T>() -> PaneGrid<T> {
    PaneGrid::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pane_creation() {
        let pane = Pane(0);
        assert_eq!(pane.0, 0);
    }

    #[test]
    fn test_split_creation() {
        let split = Split(0);
        assert_eq!(split.0, 0);
    }

    #[test]
    fn test_node_panes() {
        let node = Node::pane(Pane(0));
        let panes = node.panes();
        assert_eq!(panes.len(), 1);
        assert_eq!(panes[0], Pane(0));
    }

    #[test]
    fn test_node_splits() {
        let node = Node::split(
            Split(0),
            Axis::Vertical,
            0.5,
            Node::pane(Pane(0)),
            Node::pane(Pane(1)),
        );
        let splits = node.splits();
        assert_eq!(splits.len(), 1);
        assert_eq!(splits[0], Split(0));
    }

    #[test]
    fn test_pane_grid_builder() {
        let state = State::new("test");
        let _grid = pane_grid::<&str>().build(&state, |_pane, content, _active| {
            Content::new(format!("Pane: {}", content))
        });
    }
}
