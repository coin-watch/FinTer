//! Custom title bar for FinTer
//!
//! Provides an OS-aware custom title bar that:
//! - On macOS: Uses native traffic lights with transparent titlebar
//! - On Windows: Renders custom window controls (minimize, maximize, close)
//! - On Linux: Renders custom window controls with client-side decorations

use gpui::{
    AnyElement, App, ClickEvent, Decorations, Hsla, InteractiveElement, IntoElement, MouseButton,
    ParentElement, Pixels, RenderOnce, StatefulInteractiveElement as _, StyleRefinement, Styled,
    Window, WindowControlArea, div, prelude::FluentBuilder as _, px,
};
use gpui_component::{
    ActiveTheme, Icon, InteractiveElementExt as _, Sizable as _, StyledExt, h_flex,
};
use smallvec::SmallVec;
use std::rc::Rc;

type CloseWindowFn = Option<Rc<Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>>>;

/// Height of the custom title bar
pub const TITLE_BAR_HEIGHT: Pixels = px(38.0);

/// Left padding for macOS (space for traffic lights)
#[cfg(target_os = "macos")]
const TITLE_BAR_LEFT_PADDING: Pixels = px(80.0);

/// Left padding for Windows/Linux
#[cfg(not(target_os = "macos"))]
const TITLE_BAR_LEFT_PADDING: Pixels = px(12.0);

/// Right padding (0px so window controls align flush to the right)
const TITLE_BAR_RIGHT_PADDING: Pixels = px(0.0);

/// Custom title bar for FinTer application
#[derive(IntoElement)]
pub struct FinTerTitleBar {
    style: StyleRefinement,
    children: SmallVec<[AnyElement; 2]>,
    leading_children: SmallVec<[AnyElement; 2]>,
    on_close_window: CloseWindowFn,
    title: String,
    tab_strip: Option<AnyElement>,
}

impl FinTerTitleBar {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            style: StyleRefinement::default(),
            children: SmallVec::new(),
            leading_children: SmallVec::new(),
            on_close_window: None,
            title: title.into(),
            tab_strip: None,
        }
    }

    /// Add an editor-style strip between the app title and window controls.
    pub fn tabs<E: IntoElement>(mut self, tabs: E) -> Self {
        self.tab_strip = Some(tabs.into_any_element());
        self
    }

    #[allow(dead_code)]
    pub fn child_before_title<E: IntoElement>(self, element: E) -> Self {
        let mut this = self;
        this.leading_children.push(element.into_any_element());
        this
    }

    #[allow(dead_code)]
    pub fn on_close_window(
        mut self,
        f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        if cfg!(target_os = "linux") {
            self.on_close_window = Some(Rc::new(Box::new(f)));
        }
        self
    }
}

impl Styled for FinTerTitleBar {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl ParentElement for FinTerTitleBar {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for FinTerTitleBar {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let is_client_decorated = matches!(window.window_decorations(), Decorations::Client { .. });
        let is_linux = cfg!(target_os = "linux");
        let is_macos = cfg!(target_os = "macos");

        let title_bar_bg = cx.theme().title_bar;
        let title_bar_border = cx.theme().title_bar_border;
        let foreground = cx.theme().foreground;

        let has_tabs = self.tab_strip.is_some();

        div().flex_shrink_0().child(
            div()
                .id("finter-title-bar")
                .flex()
                .flex_row()
                .justify_between()
                .h(TITLE_BAR_HEIGHT)
                .pl(if has_tabs && !is_macos { px(0.0) } else { TITLE_BAR_LEFT_PADDING })
                .pr(TITLE_BAR_RIGHT_PADDING)
                .bg(title_bar_bg)
                .refine_style(&self.style)
                .when(is_linux, |this| {
                    this.on_double_click(|_, window, _| window.zoom_window())
                })
                .when(is_macos, |this| {
                    this.on_double_click(|_, window, _| window.titlebar_double_click())
                })
                .child(
                    h_flex()
                        .id("title-bar-content")
                        .when(!has_tabs && window.is_fullscreen(), |this| this.pl_3())
                        .h_full()
                        .flex_1()
                        .items_center()
                        .border_b_1()
                        .border_color(title_bar_border)
                        .when(!has_tabs, |this| this.gap_3())
                        .when(has_tabs, |this| this.gap(px(0.0)))
                        .children(self.leading_children)
                        .when(!self.title.is_empty(), |this| {
                            this.child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .h_full()
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                            .text_color(foreground)
                                            .child(self.title.clone()),
                                    )
                                    .when(self.tab_strip.is_some(), |this| {
                                        this.child(
                                            div()
                                                .w(px(1.0))
                                                .h(px(16.0))
                                                .bg(title_bar_border),
                                        )
                                    }),
                            )
                        })
                        .when_some(self.tab_strip, |this, tabs| this.child(tabs))
                        .children(self.children)
                        .child(
                            div()
                                .id("title-bar-drag-spacer")
                                .flex_1()
                                .h_full()
                                .window_control_area(WindowControlArea::Drag)
                                .when(is_linux && is_client_decorated, |this| {
                                    this.on_mouse_down(MouseButton::Left, |_, window, _| {
                                        window.start_window_move();
                                    })
                                }),
                        ),
                )
                .child(WindowControls {
                    on_close_window: self.on_close_window,
                }),
        )
    }
}

#[derive(IntoElement, Clone)]
enum ControlIcon {
    Minimize,
    Restore,
    Maximize,
    Close { on_close_window: CloseWindowFn },
}

impl ControlIcon {
    fn minimize() -> Self {
        Self::Minimize
    }

    fn restore() -> Self {
        Self::Restore
    }

    fn maximize() -> Self {
        Self::Maximize
    }

    fn close(on_close_window: CloseWindowFn) -> Self {
        Self::Close { on_close_window }
    }

    fn id(&self) -> &'static str {
        match self {
            Self::Minimize => "minimize",
            Self::Restore => "restore",
            Self::Maximize => "maximize",
            Self::Close { .. } => "close",
        }
    }

    fn icon_path(&self) -> &'static str {
        match self {
            Self::Minimize => "icons/window-minimize.svg",
            Self::Restore => "icons/window-restore.svg",
            Self::Maximize => "icons/window-maximize.svg",
            Self::Close { .. } => "icons/window-close.svg",
        }
    }

    fn window_control_area(&self) -> WindowControlArea {
        match self {
            Self::Minimize => WindowControlArea::Min,
            Self::Restore | Self::Maximize => WindowControlArea::Max,
            Self::Close { .. } => WindowControlArea::Close,
        }
    }

    fn is_close(&self) -> bool {
        matches!(self, Self::Close { .. })
    }

    #[inline]
    fn hover_fg(&self, cx: &App) -> Hsla {
        if self.is_close() {
            cx.theme().danger_foreground
        } else {
            cx.theme().secondary_foreground
        }
    }

    #[inline]
    fn hover_bg(&self, cx: &App) -> Hsla {
        if self.is_close() {
            cx.theme().danger
        } else {
            cx.theme().secondary_hover
        }
    }

    #[inline]
    fn active_bg(&self, cx: &mut App) -> Hsla {
        if self.is_close() {
            cx.theme().danger_active
        } else {
            cx.theme().secondary_active
        }
    }
}

impl RenderOnce for ControlIcon {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let is_linux = cfg!(target_os = "linux");
        let is_windows = cfg!(target_os = "windows");
        let hover_fg = self.hover_fg(cx);
        let hover_bg = self.hover_bg(cx);
        let active_bg = self.active_bg(cx);
        let icon = self.clone();
        let on_close_window = match &self {
            ControlIcon::Close { on_close_window } => on_close_window.clone(),
            _ => None,
        };

        div()
            .id(self.id())
            .flex()
            .w(px(46.0))
            .h(TITLE_BAR_HEIGHT)
            .flex_shrink_0()
            .justify_center()
            .content_center()
            .items_center()
            .text_color(cx.theme().foreground)
            .hover(|style| style.bg(hover_bg).text_color(hover_fg))
            .active(|style| style.bg(active_bg).text_color(hover_fg))
            .when(is_windows, |this| {
                this.window_control_area(self.window_control_area())
            })
            .when(is_linux, |this| {
                this.on_mouse_down(MouseButton::Left, move |_, window, cx| {
                    window.prevent_default();
                    cx.stop_propagation();
                })
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    match icon {
                        ControlIcon::Minimize => window.minimize_window(),
                        ControlIcon::Restore | ControlIcon::Maximize => window.zoom_window(),
                        ControlIcon::Close { .. } => {
                            if let Some(f) = on_close_window.clone() {
                                f(&ClickEvent::default(), window, cx);
                            } else {
                                window.remove_window();
                            }
                        }
                    }
                })
            })
            .child(Icon::default().path(self.icon_path()).small())
    }
}

#[derive(IntoElement)]
struct WindowControls {
    on_close_window: CloseWindowFn,
}

impl RenderOnce for WindowControls {
    fn render(self, window: &mut Window, _cx: &mut App) -> impl IntoElement {
        if cfg!(target_os = "macos") {
            return div().id("window-controls");
        }

        let title_bar_border = _cx.theme().title_bar_border;

        h_flex()
            .id("window-controls")
            .items_center()
            .flex_shrink_0()
            .h_full()
            .border_b_1()
            .border_color(title_bar_border)
            .child(ControlIcon::minimize())
            .child(if window.is_maximized() {
                ControlIcon::restore()
            } else {
                ControlIcon::maximize()
            })
            .child(ControlIcon::close(self.on_close_window))
    }
}
