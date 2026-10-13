// From:
// https://github.com/zed-industries/zed/blob/a8afc63a91f6b75528540dcffe73dc8ce0c92ad8/crates/gpui/examples/window_shadow.rs
use gpui::{
    canvas, div, point, prelude::FluentBuilder as _, px, AnyElement, App, Bounds, CursorStyle,
    Decorations, Edges, HitboxBehavior, Hsla, InteractiveElement as _, IntoElement, MouseButton,
    ParentElement, Pixels, Point, RenderOnce, ResizeEdge, Size, Styled as _, Window,
};

use crate::ActiveTheme;

const SHADOW_SIZE: Pixels = px(0.0);
const RESIZE_BORDER: Pixels = px(4.0);
const BORDER_SIZE: Pixels = px(1.0);
pub(crate) const BORDER_RADIUS: Pixels = px(0.0);

/// Create a new window border.
pub fn window_border() -> WindowBorder {
    WindowBorder::new()
}

/// Window border use to render a custom window border and shadow for Linux.
#[derive(IntoElement, Default)]
pub struct WindowBorder {
    children: Vec<AnyElement>,
}

impl WindowBorder {
    pub fn new() -> Self {
        Self {
            ..Default::default()
        }
    }
}

/// Get the window paddings.
pub fn window_paddings(window: &Window) -> Edges<Pixels> {
    match window.window_decorations() {
        Decorations::Server => Edges::all(px(0.0)),
        Decorations::Client { .. } => Edges::all(px(0.0)),
    }
}

impl ParentElement for WindowBorder {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for WindowBorder {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let decorations = window.window_decorations();
        window.set_client_inset(SHADOW_SIZE);

        div()
            .id("window-backdrop")
            .bg(gpui::transparent_black())
            .map(|div| match decorations {
                Decorations::Server => div,
                Decorations::Client { tiling, .. } => div
                    .bg(gpui::transparent_black())
                    .child(
                        canvas(
                            |_bounds, window, _| {
                                window.insert_hitbox(
                                    Bounds::new(
                                        point(px(0.0), px(0.0)),
                                        window.window_bounds().get_bounds().size,
                                    ),
                                    HitboxBehavior::Normal,
                                )
                            },
                            move |_bounds, hitbox, window, _| {
                                let mouse = window.mouse_position();
                                let size = window.window_bounds().get_bounds().size;
                                let Some(edge) = resize_edge(mouse, size) else {
                                    return;
                                };
                                window.set_cursor_style(
                                    match edge {
                                        ResizeEdge::Top | ResizeEdge::Bottom => {
                                            CursorStyle::ResizeUpDown
                                        }
                                        ResizeEdge::Left | ResizeEdge::Right => {
                                            CursorStyle::ResizeLeftRight
                                        }
                                        ResizeEdge::TopLeft | ResizeEdge::BottomRight => {
                                            CursorStyle::ResizeUpLeftDownRight
                                        }
                                        ResizeEdge::TopRight | ResizeEdge::BottomLeft => {
                                            CursorStyle::ResizeUpRightDownLeft
                                        }
                                    },
                                    &hitbox,
                                );
                            },
                        )
                        .size_full()
                        .absolute(),
                    )
                    .when(!(tiling.top || tiling.right), |div| {
                        div.rounded_tr(BORDER_RADIUS)
                    })
                    .when(!(tiling.top || tiling.left), |div| {
                        div.rounded_tl(BORDER_RADIUS)
                    })
                    .on_mouse_down(MouseButton::Left, move |_, window, _| {
                        let size = window.window_bounds().get_bounds().size;
                        let pos = window.mouse_position();

                        if let Some(edge) = resize_edge(pos, size) {
                            window.start_window_resize(edge);
                        }
                    }),
            })
            .size_full()
            .child(
                div()
                    .map(|div| match decorations {
                        Decorations::Server => div,
                        Decorations::Client { tiling } => div
                            .when(!(tiling.top || tiling.right), |div| {
                                div.rounded_tr(BORDER_RADIUS)
                            })
                            .when(!(tiling.top || tiling.left), |div| {
                                div.rounded_tl(BORDER_RADIUS)
                            })
                            .border_color(cx.theme().window_border)
                            .when(!tiling.top, |div| div.border_t(BORDER_SIZE))
                            .when(!tiling.bottom, |div| div.border_b(BORDER_SIZE))
                            .when(!tiling.left, |div| div.border_l(BORDER_SIZE))
                            .when(!tiling.right, |div| div.border_r(BORDER_SIZE)),
                    })
                    .on_mouse_move(|_e, _, cx| {
                        cx.stop_propagation();
                    })
                    .bg(gpui::transparent_black())
                    .size_full()
                    .children(self.children),
            )
    }
}

fn resize_edge(pos: Point<Pixels>, size: Size<Pixels>) -> Option<ResizeEdge> {
    let edge_size = RESIZE_BORDER;
    let edge = if pos.y < edge_size && pos.x < edge_size {
        ResizeEdge::TopLeft
    } else if pos.y < edge_size && pos.x > size.width - edge_size {
        ResizeEdge::TopRight
    } else if pos.y < edge_size {
        ResizeEdge::Top
    } else if pos.y > size.height - edge_size && pos.x < edge_size {
        ResizeEdge::BottomLeft
    } else if pos.y > size.height - edge_size && pos.x > size.width - edge_size {
        ResizeEdge::BottomRight
    } else if pos.y > size.height - edge_size {
        ResizeEdge::Bottom
    } else if pos.x < edge_size {
        ResizeEdge::Left
    } else if pos.x > size.width - edge_size {
        ResizeEdge::Right
    } else {
        return None;
    };
    Some(edge)
}
