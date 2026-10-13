use std::rc::Rc;

use gpui::{
    Context, Corner, DismissEvent, ElementId, Entity, Focusable, InteractiveElement, IntoElement,
    RenderOnce, SharedString, StyleRefinement, Styled, Window,
};

use crate::{button::Button, menu::PopupMenu, popover::Popover, Selectable};

/// A grouped-list popup trait for buttons and other interactive elements.
///
/// It uses the same popup list as `DropdownMenu`, while allowing callers to
/// add group labels with `PopupMenu::label` before each group of items.
pub trait GroupListPopup: Styled + Selectable + InteractiveElement + IntoElement + 'static {
    /// Create a grouped-list popup anchored to the TopLeft corner.
    fn group_list_popup(
        self,
        f: impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static,
    ) -> GroupListPopupPopover<Self> {
        self.group_list_popup_with_anchor(Corner::TopLeft, f)
    }

    /// Create a dropdown menu with the given items, anchored to the given corner
    fn group_list_popup_with_anchor(
        mut self,
        anchor: impl Into<Corner>,
        f: impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static,
    ) -> GroupListPopupPopover<Self> {
        let style = self.style().clone();
        let id = self.interactivity().element_id.clone();

        GroupListPopupPopover::new(id.unwrap_or(0.into()), anchor, self, f).trigger_style(style)
    }

    /// Create a dropdown menu that appears to the right side of the trigger
    fn dropdown_menu_side(
        mut self,
        anchor: impl Into<Corner>,
        f: impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static,
    ) -> GroupListPopupPopover<Self> {
        let style = self.style().clone();
        let id = self.interactivity().element_id.clone();

        GroupListPopupPopover::new(id.unwrap_or(0.into()), anchor, self, f)
            .trigger_style(style)
            .side_placement(true)
    }
}

impl GroupListPopup for Button {}

#[derive(IntoElement)]
pub struct GroupListPopupPopover<T: Selectable + IntoElement + 'static> {
    id: ElementId,
    style: StyleRefinement,
    anchor: Corner,
    side_placement: bool,
    trigger: T,
    builder: Rc<dyn Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu>,
}

impl<T> GroupListPopupPopover<T>
where
    T: Selectable + IntoElement + 'static,
{
    fn new(
        id: ElementId,
        anchor: impl Into<Corner>,
        trigger: T,
        builder: impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static,
    ) -> Self {
        Self {
            id: SharedString::from(format!("group-list-popup:{:?}", id)).into(),
            style: StyleRefinement::default(),
            anchor: anchor.into(),
            side_placement: false,
            trigger,
            builder: Rc::new(builder),
        }
    }

    /// Set the anchor corner for the dropdown menu popover.
    pub fn anchor(mut self, anchor: impl Into<Corner>) -> Self {
        self.anchor = anchor.into();
        self
    }

    /// Enable side placement (popover appears to the right of trigger)
    pub fn side_placement(mut self, side: bool) -> Self {
        self.side_placement = side;
        self
    }

    /// Set the style refinement for the dropdown menu trigger.
    fn trigger_style(mut self, style: StyleRefinement) -> Self {
        self.style = style;
        self
    }
}

#[derive(Default)]
struct GroupListPopupState {
    menu: Option<Entity<PopupMenu>>,
}

impl<T> RenderOnce for GroupListPopupPopover<T>
where
    T: Selectable + IntoElement + 'static,
{
    fn render(self, window: &mut Window, cx: &mut gpui::App) -> impl IntoElement {
        let builder = self.builder.clone();
        let menu_state =
            window.use_keyed_state(self.id.clone(), cx, |_, _| GroupListPopupState::default());

        Popover::new(SharedString::from(format!("popover:{}", self.id)))
            .appearance(false)
            .overlay_closable(false)
            .trigger(self.trigger)
            .trigger_style(self.style)
            .anchor(self.anchor)
            .side_placement(self.side_placement)
            .content(move |_, window, cx| {
                // Here is special logic to only create the PopupMenu once and reuse it.
                // Because this `content` will called in every time render, so we need to store the menu
                // in state to avoid recreating at every render.
                //
                // And we also need to rebuild the menu when it is dismissed, to rebuild menu items
                // dynamically for support `dropdown_menu` method, so we listen for DismissEvent below.
                let menu = match menu_state.read(cx).menu.clone() {
                    Some(menu) => menu,
                    None => {
                        let builder = builder.clone();
                        let menu = PopupMenu::build(window, cx, move |menu, window, cx| {
                            builder(menu, window, cx)
                                .scrollable(true)
                                .accordion_groups(true)
                                .searchable_accordion(true)
                        });
                        menu_state.update(cx, |state, _| {
                            state.menu = Some(menu.clone());
                        });
                        // Do not mutate focus while building deferred content. The trigger click is
                        // still being dispatched; focusing here re-enters GPUI's event tree and can
                        // tear down the parent Popover mid-event.
                        let menu_for_focus = menu.clone();
                        window.defer(cx, move |window, cx| {
                            menu_for_focus.focus_handle(cx).focus(window);
                        });

                        // Listen for dismiss events from the PopupMenu to close the popover. Defer
                        // both mutations until PopupMenu finishes its dismiss event.
                        let popover_state = cx.entity();
                        window
                            .subscribe(&menu, cx, {
                                let menu_state = menu_state.clone();
                                move |_, _: &DismissEvent, window, cx| {
                                    let menu_state = menu_state.clone();
                                    let popover_state = popover_state.clone();
                                    window.defer(cx, move |window, cx| {
                                        popover_state.update(cx, |state, cx| {
                                            state.dismiss(window, cx);
                                        });
                                        menu_state.update(cx, |state, _| {
                                            state.menu = None;
                                        });
                                    });
                                }
                            })
                            .detach();

                        menu.clone()
                    }
                };

                menu.clone()
            })
    }
}
