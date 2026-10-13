use gpui::{
    App, Bounds, Element, GlobalElementId, InspectorElementId, IntoElement, LayoutId, Pixels,
    Window,
};

/// A wrapper element that calls a callback with the element's own bounds during prepaint.
pub struct BoundsObserverElement<E: Element> {
    inner: E,
    callback: Box<dyn Fn(Bounds<Pixels>, &mut Window, &mut App) + 'static>,
}

impl<E: Element> Element for BoundsObserverElement<E> {
    type RequestLayoutState = E::RequestLayoutState;
    type PrepaintState = E::PrepaintState;

    fn id(&self) -> Option<gpui::ElementId> {
        self.inner.id()
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        self.inner.source_location()
    }

    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        self.inner.request_layout(id, inspector_id, window, cx)
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        (self.callback)(bounds, window, cx);
        self.inner
            .prepaint(id, inspector_id, bounds, request_layout, window, cx)
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.inner.paint(
            id,
            inspector_id,
            bounds,
            request_layout,
            prepaint,
            window,
            cx,
        )
    }
}

impl<E: Element> IntoElement for BoundsObserverElement<E> {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

/// Extension trait for adding an `on_prepaint` bounds callback to any element.
///
/// Usage:
/// ```ignore
/// div().id("my-div")
///     .size_full()
///     .on_prepaint(move |bounds, _window, cx| {
///         my_view.update(cx, |view, _| view.bounds = bounds);
///     })
/// ```
pub trait ElementExt: IntoElement + Sized {
    fn on_prepaint<F>(self, f: F) -> BoundsObserverElement<Self::Element>
    where
        F: Fn(Bounds<Pixels>, &mut Window, &mut App) + 'static,
    {
        BoundsObserverElement {
            inner: self.into_element(),
            callback: Box::new(f),
        }
    }
}

impl<E: IntoElement> ElementExt for E {}
