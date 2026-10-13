use crate::actions::{Cancel, Confirm, SelectDown, SelectUp};
use crate::actions::{SelectLeft, SelectRight};
use crate::input::{Input, InputEvent, InputState};
use crate::menu::menu_item::MenuItemElement;
use crate::scroll::ScrollableElement;
use crate::{ActiveTheme, Icon, IconName, Sizable as _, h_flex, v_flex};
use crate::{Side, Size, StyledExt, kbd::Kbd};
use gpui::{
    Action, AnyElement, App, AppContext, Bounds, Context, Corner, DismissEvent, Edges, ElementId,
    Entity, EventEmitter, FocusHandle, Focusable, InteractiveElement, IntoElement, KeyBinding,
    ParentElement, Pixels, Render, ScrollHandle, SharedString, StatefulInteractiveElement, Styled,
    WeakEntity, Window, anchored, canvas, div, prelude::FluentBuilder, px, rems,
};
use gpui::{ClickEvent, Half, MouseDownEvent, OwnedMenuItem, Subscription};
use std::{
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
};

const CONTEXT: &str = "PopupMenu";

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("enter", Confirm { secondary: false }, Some(CONTEXT)),
        KeyBinding::new("escape", Cancel, Some(CONTEXT)),
        KeyBinding::new("up", SelectUp, Some(CONTEXT)),
        KeyBinding::new("down", SelectDown, Some(CONTEXT)),
        KeyBinding::new("left", SelectLeft, Some(CONTEXT)),
        KeyBinding::new("right", SelectRight, Some(CONTEXT)),
    ]);
}

/// An menu item in a popup menu.
pub enum PopupMenuItem {
    /// A menu separator item.
    Separator,
    /// A non-interactive label item.
    Label(SharedString),
    /// A standard menu item.
    Item {
        icon: Option<Icon>,
        label: SharedString,
        disabled: bool,
        checked: bool,
        is_link: bool,
        action: Option<Box<dyn Action>>,
        // For link item
        handler: Option<Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>>,
    },
    /// A menu item with custom element render.
    ElementItem {
        icon: Option<Icon>,
        disabled: bool,
        checked: bool,
        action: Option<Box<dyn Action>>,
        render: Box<dyn Fn(&mut Window, &mut App) -> AnyElement + 'static>,
        handler: Option<Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>>,
    },
    /// A submenu item that opens another popup menu.
    ///
    /// NOTE: This is only supported when the parent menu is not `scrollable`.
    Submenu {
        icon: Option<Icon>,
        label: SharedString,
        disabled: bool,
        menu: Entity<PopupMenu>,
    },
}

impl FluentBuilder for PopupMenuItem {}
impl PopupMenuItem {
    /// Create a new menu item with the given label.
    #[inline]
    pub fn new(label: impl Into<SharedString>) -> Self {
        PopupMenuItem::Item {
            icon: None,
            label: label.into(),
            disabled: false,
            checked: false,
            action: None,
            is_link: false,
            handler: None,
        }
    }

    /// Create a new menu item with custom element render.
    #[inline]
    pub fn element<F, E>(builder: F) -> Self
    where
        F: Fn(&mut Window, &mut App) -> E + 'static,
        E: IntoElement,
    {
        PopupMenuItem::ElementItem {
            icon: None,
            disabled: false,
            checked: false,
            action: None,
            render: Box::new(move |window, cx| builder(window, cx).into_any_element()),
            handler: None,
        }
    }

    /// Create a new submenu item that opens another popup menu.
    #[inline]
    pub fn submenu(label: impl Into<SharedString>, menu: Entity<PopupMenu>) -> Self {
        PopupMenuItem::Submenu {
            icon: None,
            label: label.into(),
            disabled: false,
            menu,
        }
    }

    /// Create a separator menu item.
    #[inline]
    pub fn separator() -> Self {
        PopupMenuItem::Separator
    }

    /// Creates a label menu item.
    #[inline]
    pub fn label(label: impl Into<SharedString>) -> Self {
        PopupMenuItem::Label(label.into())
    }

    /// Set the icon for the menu item.
    ///
    /// Only works for [`PopupMenuItem::Item`], [`PopupMenuItem::ElementItem`] and [`PopupMenuItem::Submenu`].
    pub fn icon(mut self, icon: impl Into<Icon>) -> Self {
        match &mut self {
            PopupMenuItem::Item { icon: i, .. } => {
                *i = Some(icon.into());
            }
            PopupMenuItem::ElementItem { icon: i, .. } => {
                *i = Some(icon.into());
            }
            PopupMenuItem::Submenu { icon: i, .. } => {
                *i = Some(icon.into());
            }
            _ => {}
        }
        self
    }

    /// Set the action for the menu item.
    ///
    /// Only works for [`PopupMenuItem::Item`] and [`PopupMenuItem::ElementItem`].
    pub fn action(mut self, action: Box<dyn Action>) -> Self {
        match &mut self {
            PopupMenuItem::Item { action: a, .. } => {
                *a = Some(action);
            }
            PopupMenuItem::ElementItem { action: a, .. } => {
                *a = Some(action);
            }
            _ => {}
        }
        self
    }

    /// Set the disabled state for the menu item.
    ///
    /// Only works for [`PopupMenuItem::Item`], [`PopupMenuItem::ElementItem`] and [`PopupMenuItem::Submenu`].
    pub fn disabled(mut self, disabled: bool) -> Self {
        match &mut self {
            PopupMenuItem::Item { disabled: d, .. } => {
                *d = disabled;
            }
            PopupMenuItem::ElementItem { disabled: d, .. } => {
                *d = disabled;
            }
            PopupMenuItem::Submenu { disabled: d, .. } => {
                *d = disabled;
            }
            _ => {}
        }
        self
    }

    /// Set checked state for the menu item.
    ///
    /// NOTE: If `check_side` is [`Side::Left`], the icon will replace with a check icon.
    pub fn checked(mut self, checked: bool) -> Self {
        match &mut self {
            PopupMenuItem::Item { checked: c, .. } => {
                *c = checked;
            }
            PopupMenuItem::ElementItem { checked: c, .. } => {
                *c = checked;
            }
            _ => {}
        }
        self
    }

    /// Add a click handler for the menu item.
    ///
    /// Only works for [`PopupMenuItem::Item`] and [`PopupMenuItem::ElementItem`].
    pub fn on_click<F>(mut self, handler: F) -> Self
    where
        F: Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    {
        match &mut self {
            PopupMenuItem::Item { handler: h, .. } => {
                *h = Some(Rc::new(handler));
            }
            PopupMenuItem::ElementItem { handler: h, .. } => {
                *h = Some(Rc::new(handler));
            }
            _ => {}
        }
        self
    }

    /// Create a link menu item.
    #[inline]
    pub fn link(label: impl Into<SharedString>, href: impl Into<String>) -> Self {
        let href = href.into();
        PopupMenuItem::Item {
            icon: None,
            label: label.into(),
            disabled: false,
            checked: false,
            action: None,
            is_link: true,
            handler: Some(Rc::new(move |_, _, cx| cx.open_url(&href))),
        }
    }

    #[inline]
    fn is_clickable(&self) -> bool {
        !matches!(self, PopupMenuItem::Separator)
            && matches!(
                self,
                PopupMenuItem::Item {
                    disabled: false,
                    ..
                } | PopupMenuItem::ElementItem {
                    disabled: false,
                    ..
                } | PopupMenuItem::Submenu {
                    disabled: false,
                    ..
                }
            )
    }

    #[inline]
    fn is_separator(&self) -> bool {
        matches!(self, PopupMenuItem::Separator)
    }

    fn has_left_icon(&self, check_side: Side) -> bool {
        match self {
            PopupMenuItem::Item { icon, checked, .. } => {
                icon.is_some() || (check_side.is_left() && *checked)
            }
            PopupMenuItem::ElementItem { icon, checked, .. } => {
                icon.is_some() || (check_side.is_left() && *checked)
            }
            PopupMenuItem::Submenu { icon, .. } => icon.is_some(),
            _ => false,
        }
    }

    #[inline]
    fn is_checked(&self) -> bool {
        match self {
            PopupMenuItem::Item { checked, .. } => *checked,
            PopupMenuItem::ElementItem { checked, .. } => *checked,
            _ => false,
        }
    }

    #[inline]
    pub fn label_text(&self) -> Option<&SharedString> {
        match self {
            PopupMenuItem::Label(label) => Some(label),
            PopupMenuItem::Item { label, .. } => Some(label),
            PopupMenuItem::Submenu { label, .. } => Some(label),
            _ => None,
        }
    }
}

pub struct PopupMenu {
    pub(crate) focus_handle: FocusHandle,
    pub(crate) menu_items: Vec<PopupMenuItem>,
    /// The focus handle of Entity to handle actions.
    pub(crate) action_context: Option<FocusHandle>,
    selected_index: Option<usize>,
    min_width: Option<Pixels>,
    max_width: Option<Pixels>,
    max_height: Option<Pixels>,
    bounds: Bounds<Pixels>,
    size: Size,
    check_side: Side,

    /// The parent menu of this menu, if this is a submenu
    parent_menu: Option<WeakEntity<Self>>,
    scrollable: bool,
    external_link_icon: bool,
    scroll_handle: ScrollHandle,
    accordion_groups: bool,
    open_accordion_labels: BTreeSet<usize>,
    searchable_accordion: bool,
    accordion_search_inputs: BTreeMap<usize, Entity<InputState>>,
    // This will update on render
    submenu_anchor: (Corner, Pixels),

    _subscriptions: Vec<Subscription>,
}

impl PopupMenu {
    pub(crate) fn new(cx: &mut App) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            action_context: None,
            parent_menu: None,
            menu_items: Vec::new(),
            selected_index: None,
            min_width: None,
            max_width: None,
            max_height: None,
            check_side: Side::Left,
            bounds: Bounds::default(),
            scrollable: false,
            scroll_handle: ScrollHandle::default(),
            external_link_icon: true,
            accordion_groups: false,
            open_accordion_labels: BTreeSet::new(),
            searchable_accordion: false,
            accordion_search_inputs: BTreeMap::new(),
            size: Size::default(),
            submenu_anchor: (Corner::TopLeft, Pixels::ZERO),
            _subscriptions: vec![],
        }
    }

    pub fn build(
        window: &mut Window,
        cx: &mut App,
        f: impl FnOnce(Self, &mut Window, &mut Context<PopupMenu>) -> Self,
    ) -> Entity<Self> {
        cx.new(|cx| {
            let mut menu = f(Self::new(cx), window, cx);
            menu.init_accordion_search(window, cx);
            menu
        })
    }

    /// Set the focus handle of Entity to handle actions.
    ///
    /// When the menu is dismissed or before an action is triggered, the focus will be returned to this handle.
    ///
    /// Then the action will be dispatched to this handle.
    pub fn action_context(mut self, handle: FocusHandle) -> Self {
        self.action_context = Some(handle);
        self
    }

    /// Set min width of the popup menu, default is 120px
    pub fn min_w(mut self, width: impl Into<Pixels>) -> Self {
        self.min_width = Some(width.into());
        self
    }

    /// Set max width of the popup menu, default is 500px
    pub fn max_w(mut self, width: impl Into<Pixels>) -> Self {
        self.max_width = Some(width.into());
        self
    }

    /// Set max height of the popup menu, default is half of the window height
    pub fn max_h(mut self, height: impl Into<Pixels>) -> Self {
        self.max_height = Some(height.into());
        self
    }

    /// Set the menu to be scrollable to show vertical scrollbar.
    ///
    /// NOTE: If this is true, the sub-menus will cannot be support.
    pub fn scrollable(mut self, scrollable: bool) -> Self {
        self.scrollable = scrollable;
        self
    }

    /// Render labels as accordion headers and hide items in collapsed groups.
    pub fn accordion_groups(mut self, accordion_groups: bool) -> Self {
        self.accordion_groups = accordion_groups;
        self
    }

    /// Enable search input inside each open accordion group.
    pub fn searchable_accordion(mut self, searchable: bool) -> Self {
        self.searchable_accordion = searchable;
        self
    }

    pub(crate) fn init_accordion_search(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.accordion_groups || !self.searchable_accordion {
            return;
        }

        for (ix, item) in self.menu_items.iter().enumerate() {
            if let PopupMenuItem::Label(label) = item {
                let has_items = self.menu_items[ix + 1..]
                    .iter()
                    .take_while(|item| !matches!(item, PopupMenuItem::Label(_)))
                    .any(|item| item.is_clickable());

                if has_items && !self.accordion_search_inputs.contains_key(&ix) {
                    let placeholder = format!("Search {label}...");
                    let input = cx.new(|cx| {
                        InputState::new(window, cx)
                            .placeholder(placeholder)
                    });

                    let sub = cx.subscribe(&input, |_, _, event: &InputEvent, cx| {
                        if matches!(event, InputEvent::Change) {
                            cx.notify();
                        }
                    });
                    self._subscriptions.push(sub);
                    self.accordion_search_inputs.insert(ix, input);
                }
            }
        }
    }

    /// Set the side to show check icon, default is `Side::Left`.
    pub fn check_side(mut self, side: Side) -> Self {
        self.check_side = side;
        self
    }

    /// Set the menu to show external link icon, default is true.
    pub fn external_link_icon(mut self, visible: bool) -> Self {
        self.external_link_icon = visible;
        self
    }

    /// Add Menu Item
    pub fn menu(self, label: impl Into<SharedString>, action: Box<dyn Action>) -> Self {
        self.menu_with_disabled(label, action, false)
    }

    /// Add Menu Item with enable state
    pub fn menu_with_enable(
        mut self,
        label: impl Into<SharedString>,
        action: Box<dyn Action>,
        enable: bool,
    ) -> Self {
        self.add_menu_item(label, None, action, !enable, false);
        self
    }

    /// Add Menu Item with disabled state
    pub fn menu_with_disabled(
        mut self,
        label: impl Into<SharedString>,
        action: Box<dyn Action>,
        disabled: bool,
    ) -> Self {
        self.add_menu_item(label, None, action, disabled, false);
        self
    }

    /// Add label
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.menu_items.push(PopupMenuItem::label(label.into()));
        self
    }

    /// Add Menu to open link
    pub fn link(self, label: impl Into<SharedString>, href: impl Into<String>) -> Self {
        self.link_with_disabled(label, href, false)
    }

    /// Add Menu to open link with disabled state
    pub fn link_with_disabled(
        mut self,
        label: impl Into<SharedString>,
        href: impl Into<String>,
        disabled: bool,
    ) -> Self {
        let href = href.into();
        self.menu_items
            .push(PopupMenuItem::link(label, href).disabled(disabled));
        self
    }

    /// Add Menu to open link
    pub fn link_with_icon(
        self,
        label: impl Into<SharedString>,
        icon: impl Into<Icon>,
        href: impl Into<String>,
    ) -> Self {
        self.link_with_icon_and_disabled(label, icon, href, false)
    }

    /// Add Menu to open link with icon and disabled state
    fn link_with_icon_and_disabled(
        mut self,
        label: impl Into<SharedString>,
        icon: impl Into<Icon>,
        href: impl Into<String>,
        disabled: bool,
    ) -> Self {
        let href = href.into();
        self.menu_items.push(
            PopupMenuItem::link(label, href)
                .icon(icon)
                .disabled(disabled),
        );
        self
    }

    /// Add Menu Item with Icon.
    pub fn menu_with_icon(
        self,
        label: impl Into<SharedString>,
        icon: impl Into<Icon>,
        action: Box<dyn Action>,
    ) -> Self {
        self.menu_with_icon_and_disabled(label, icon, action, false)
    }

    /// Add Menu Item with Icon and disabled state
    pub fn menu_with_icon_and_disabled(
        mut self,
        label: impl Into<SharedString>,
        icon: impl Into<Icon>,
        action: Box<dyn Action>,
        disabled: bool,
    ) -> Self {
        self.add_menu_item(label, Some(icon.into()), action, disabled, false);
        self
    }

    /// Add Menu Item with check icon
    pub fn menu_with_check(
        self,
        label: impl Into<SharedString>,
        checked: bool,
        action: Box<dyn Action>,
    ) -> Self {
        self.menu_with_check_and_disabled(label, checked, action, false)
    }

    /// Add Menu Item with check icon and disabled state
    pub fn menu_with_check_and_disabled(
        mut self,
        label: impl Into<SharedString>,
        checked: bool,
        action: Box<dyn Action>,
        disabled: bool,
    ) -> Self {
        self.add_menu_item(label, None, action, disabled, checked);
        self
    }

    /// Add Menu Item with custom element render.
    pub fn menu_element<F, E>(self, action: Box<dyn Action>, builder: F) -> Self
    where
        F: Fn(&mut Window, &mut App) -> E + 'static,
        E: IntoElement,
    {
        self.menu_element_with_check(false, action, builder)
    }

    /// Add Menu Item with custom element render with disabled state.
    pub fn menu_element_with_disabled<F, E>(
        self,
        action: Box<dyn Action>,
        disabled: bool,
        builder: F,
    ) -> Self
    where
        F: Fn(&mut Window, &mut App) -> E + 'static,
        E: IntoElement,
    {
        self.menu_element_with_check_and_disabled(false, action, disabled, builder)
    }

    /// Add Menu Item with custom element render with icon.
    pub fn menu_element_with_icon<F, E>(
        self,
        icon: impl Into<Icon>,
        action: Box<dyn Action>,
        builder: F,
    ) -> Self
    where
        F: Fn(&mut Window, &mut App) -> E + 'static,
        E: IntoElement,
    {
        self.menu_element_with_icon_and_disabled(icon, action, false, builder)
    }

    /// Add Menu Item with custom element render with check state
    pub fn menu_element_with_check<F, E>(
        self,
        checked: bool,
        action: Box<dyn Action>,
        builder: F,
    ) -> Self
    where
        F: Fn(&mut Window, &mut App) -> E + 'static,
        E: IntoElement,
    {
        self.menu_element_with_check_and_disabled(checked, action, false, builder)
    }

    /// Add Menu Item with custom element render with icon and disabled state
    fn menu_element_with_icon_and_disabled<F, E>(
        mut self,
        icon: impl Into<Icon>,
        action: Box<dyn Action>,
        disabled: bool,
        builder: F,
    ) -> Self
    where
        F: Fn(&mut Window, &mut App) -> E + 'static,
        E: IntoElement,
    {
        self.menu_items.push(
            PopupMenuItem::element(builder)
                .action(action)
                .icon(icon)
                .disabled(disabled),
        );
        self
    }

    /// Add Menu Item with custom element render with check state and disabled state
    fn menu_element_with_check_and_disabled<F, E>(
        mut self,
        checked: bool,
        action: Box<dyn Action>,
        disabled: bool,
        builder: F,
    ) -> Self
    where
        F: Fn(&mut Window, &mut App) -> E + 'static,
        E: IntoElement,
    {
        self.menu_items.push(
            PopupMenuItem::element(builder)
                .action(action)
                .checked(checked)
                .disabled(disabled),
        );
        self
    }

    /// Add a separator Menu Item
    pub fn separator(mut self) -> Self {
        if self.menu_items.is_empty() {
            return self;
        }

        if let Some(PopupMenuItem::Separator) = self.menu_items.last() {
            return self;
        }

        self.menu_items.push(PopupMenuItem::separator());
        self
    }

    /// Add a Submenu
    pub fn submenu(
        self,
        label: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
        f: impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static,
    ) -> Self {
        self.submenu_with_icon(None, label, window, cx, f)
    }

    /// Add a Submenu item with icon
    pub fn submenu_with_icon(
        mut self,
        icon: Option<Icon>,
        label: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
        f: impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static,
    ) -> Self {
        let submenu = PopupMenu::build(window, cx, f);
        let parent_menu = cx.entity().downgrade();
        submenu.update(cx, |view, _| {
            view.parent_menu = Some(parent_menu);
        });

        self.menu_items.push(
            PopupMenuItem::submenu(label, submenu).when_some(icon, |this, icon| this.icon(icon)),
        );
        self
    }

    /// Add menu item.
    pub fn item(mut self, item: impl Into<PopupMenuItem>) -> Self {
        let item: PopupMenuItem = item.into();
        self.menu_items.push(item);
        self
    }

    /// Use small size, the menu item will have smaller height.
    pub(crate) fn small(mut self) -> Self {
        self.size = Size::Small;
        self
    }

    fn add_menu_item(
        &mut self,
        label: impl Into<SharedString>,
        icon: Option<Icon>,
        action: Box<dyn Action>,
        disabled: bool,
        checked: bool,
    ) -> &mut Self {
        self.menu_items.push(
            PopupMenuItem::new(label)
                .when_some(icon, |item, icon| item.icon(icon))
                .disabled(disabled)
                .checked(checked)
                .action(action),
        );
        self
    }

    pub(super) fn with_menu_items<I>(
        mut self,
        items: impl IntoIterator<Item = I>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self
    where
        I: Into<OwnedMenuItem>,
    {
        for item in items {
            match item.into() {
                OwnedMenuItem::Action { name, action, .. } => {
                    self = self.menu(name, action.boxed_clone())
                }
                OwnedMenuItem::Separator => {
                    self = self.separator();
                }
                OwnedMenuItem::Submenu(submenu) => {
                    self = self.submenu(submenu.name, window, cx, move |menu, window, cx| {
                        menu.with_menu_items(submenu.items.clone(), window, cx)
                    })
                }
                OwnedMenuItem::SystemMenu(_) => {}
            }
        }

        if self.menu_items.len() > 20 {
            self.scrollable = true;
        }

        self
    }

    pub(crate) fn active_submenu(&self) -> Option<Entity<PopupMenu>> {
        if let Some(ix) = self.selected_index {
            if let Some(item) = self.menu_items.get(ix) {
                return match item {
                    PopupMenuItem::Submenu { menu, .. } => Some(menu.clone()),
                    _ => None,
                };
            }
        }

        None
    }

    pub fn is_empty(&self) -> bool {
        self.menu_items.is_empty()
    }

    fn clickable_menu_items(&self) -> impl Iterator<Item = (usize, &PopupMenuItem)> {
        self.menu_items
            .iter()
            .enumerate()
            .filter(|(_, item)| item.is_clickable())
    }

    fn on_click(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        window.prevent_default();
        self.selected_index = Some(ix);
        self.confirm(&Confirm { secondary: false }, window, cx);
    }

    fn confirm(&mut self, _: &Confirm, window: &mut Window, cx: &mut Context<Self>) {
        match self.selected_index {
            Some(index) => {
                let item = self.menu_items.get(index);
                match item {
                    Some(PopupMenuItem::Item {
                        handler, action, ..
                    }) => {
                        if let Some(handler) = handler {
                            handler(&ClickEvent::default(), window, cx);
                        } else if let Some(action) = action.as_ref() {
                            self.dispatch_confirm_action(action, window, cx);
                        }

                        self.dismiss(&Cancel, window, cx)
                    }
                    Some(PopupMenuItem::ElementItem {
                        handler, action, ..
                    }) => {
                        if let Some(handler) = handler {
                            handler(&ClickEvent::default(), window, cx);
                        } else if let Some(action) = action.as_ref() {
                            self.dispatch_confirm_action(action, window, cx);
                        }
                        self.dismiss(&Cancel, window, cx)
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn dispatch_confirm_action(
        &self,
        action: &Box<dyn Action>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(context) = self.action_context.as_ref() {
            context.focus(window);
        }

        window.dispatch_action(action.boxed_clone(), cx);
    }

    fn set_selected_index(&mut self, ix: usize, cx: &mut Context<Self>) {
        if self.selected_index != Some(ix) {
            self.selected_index = Some(ix);
            self.scroll_handle.scroll_to_item(ix);
            cx.notify();
        }
    }

    fn select_up(&mut self, _: &SelectUp, _: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        let ix = self.selected_index.unwrap_or(0);

        if let Some((prev_ix, _)) = self
            .menu_items
            .iter()
            .enumerate()
            .rev()
            .find(|(i, item)| *i < ix && item.is_clickable())
        {
            self.set_selected_index(prev_ix, cx);
            return;
        }

        let last_clickable_ix = self.clickable_menu_items().last().map(|(ix, _)| ix);
        self.set_selected_index(last_clickable_ix.unwrap_or(0), cx);
    }

    fn select_down(&mut self, _: &SelectDown, _: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        let Some(ix) = self.selected_index else {
            self.set_selected_index(0, cx);
            return;
        };

        if let Some((next_ix, _)) = self
            .menu_items
            .iter()
            .enumerate()
            .find(|(i, item)| *i > ix && item.is_clickable())
        {
            self.set_selected_index(next_ix, cx);
            return;
        }

        self.set_selected_index(0, cx);
    }

    fn select_left(&mut self, _: &SelectLeft, window: &mut Window, cx: &mut Context<Self>) {
        let handled = if matches!(self.submenu_anchor.0, Corner::TopLeft | Corner::BottomLeft) {
            self._unselect_submenu(window, cx)
        } else {
            self._select_submenu(window, cx)
        };

        if self.parent_side(cx).is_left() {
            self._focus_parent_menu(window, cx);
        }

        if handled {
            return;
        }

        // For parent AppMenuBar to handle.
        if self.parent_menu.is_none() {
            cx.propagate();
        }
    }

    fn select_right(&mut self, _: &SelectRight, window: &mut Window, cx: &mut Context<Self>) {
        let handled = if matches!(self.submenu_anchor.0, Corner::TopLeft | Corner::BottomLeft) {
            self._select_submenu(window, cx)
        } else {
            self._unselect_submenu(window, cx)
        };

        if self.parent_side(cx).is_right() {
            self._focus_parent_menu(window, cx);
        }

        if handled {
            return;
        }

        // For parent AppMenuBar to handle.
        if self.parent_menu.is_none() {
            cx.propagate();
        }
    }

    fn _select_submenu(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if let Some(active_submenu) = self.active_submenu() {
            // Focus the submenu, so that can be handle the action.
            active_submenu.update(cx, |view, cx| {
                view.set_selected_index(0, cx);
                view.focus_handle.focus(window);
            });
            cx.notify();
            return true;
        }

        return false;
    }

    fn _unselect_submenu(&mut self, _: &mut Window, cx: &mut Context<Self>) -> bool {
        if let Some(active_submenu) = self.active_submenu() {
            active_submenu.update(cx, |view, cx| {
                view.selected_index = None;
                cx.notify();
            });
            return true;
        }

        return false;
    }

    fn _focus_parent_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(parent) = self.parent_menu.as_ref() else {
            return;
        };
        let Some(parent) = parent.upgrade() else {
            return;
        };

        self.selected_index = None;
        parent.update(cx, |view, cx| {
            view.focus_handle.focus(window);
            cx.notify();
        });
    }

    fn parent_side(&self, cx: &App) -> Side {
        let Some(parent) = self.parent_menu.as_ref() else {
            return Side::Left;
        };

        let Some(parent) = parent.upgrade() else {
            return Side::Left;
        };

        match parent.read(cx).submenu_anchor.0 {
            Corner::TopLeft | Corner::BottomLeft => Side::Left,
            Corner::TopRight | Corner::BottomRight => Side::Right,
        }
    }

    fn dismiss(&mut self, _: &Cancel, window: &mut Window, cx: &mut Context<Self>) {
        if self.active_submenu().is_some() {
            return;
        }

        cx.emit(DismissEvent);

        // Focus back to the previous focused handle.
        if let Some(action_context) = self.action_context.as_ref() {
            window.focus(action_context);
        }

        let Some(parent_menu) = self.parent_menu.clone() else {
            return;
        };

        // Dismiss parent menu, when this menu is dismissed
        _ = parent_menu.update(cx, |view, cx| {
            view.selected_index = None;
            view.dismiss(&Cancel, window, cx);
        });
    }

    fn render_key_binding(
        &self,
        action: Option<Box<dyn Action>>,
        window: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Kbd> {
        let action = action?;

        match self
            .action_context
            .as_ref()
            .and_then(|handle| Kbd::binding_for_action_in(action.as_ref(), handle, window))
        {
            Some(kbd) => Some(kbd),
            // Fallback to App level key binding
            None => Kbd::binding_for_action(action.as_ref(), None, window),
        }
        .map(|this| {
            this.p_0()
                .flex_nowrap()
                .border_0()
                .bg(gpui::transparent_white())
        })
    }

    fn render_icon(
        has_icon: bool,
        checked: bool,
        icon: Option<Icon>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<impl IntoElement> {
        if !has_icon {
            return None;
        }

        let icon = if let Some(icon) = icon {
            icon.clone()
        } else if checked {
            Icon::new(IconName::Check)
        } else {
            Icon::empty()
        };

        Some(icon.xsmall())
    }

    #[inline]
    fn max_width(&self) -> Pixels {
        self.max_width.unwrap_or(px(500.))
    }

    fn is_accordion_label_open(&self, ix: usize) -> bool {
        !self.accordion_groups || self.open_accordion_labels.contains(&ix)
    }

    fn toggle_accordion_label(&mut self, ix: usize, cx: &mut Context<Self>) {
        if self.open_accordion_labels.contains(&ix) {
            self.open_accordion_labels.remove(&ix);
        } else {
            self.open_accordion_labels.clear();
            self.open_accordion_labels.insert(ix);
        }
        cx.notify();
    }

    #[allow(dead_code)]
    fn is_item_visible_in_accordion(&self, ix: usize, item: &PopupMenuItem) -> bool {
        if !self.accordion_groups || matches!(item, PopupMenuItem::Label(_)) {
            return true;
        }

        let Some(label_ix) = self.menu_items[..ix]
            .iter()
            .rposition(|item| matches!(item, PopupMenuItem::Label(_)))
        else {
            return true;
        };

        self.open_accordion_labels.contains(&label_ix)
    }

    fn render_items(
        &self,
        items_count: usize,
        options: RenderOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut elements: Vec<AnyElement> = Vec::new();
        let mut current_label_ix: Option<usize> = None;
        let mut current_query: Option<String> = None;
        let mut group_matched_any = false;
        let mut in_searchable_group = false;

        for (ix, item) in self.menu_items.iter().enumerate() {
            // Ignore last separator
            if ix + 1 == items_count && item.is_separator() {
                continue;
            }

            match item {
                PopupMenuItem::Label(_label) => {
                    if in_searchable_group && !group_matched_any {
                        if let Some(ref q) = current_query {
                            if !q.is_empty() {
                                elements.push(
                                    div()
                                        .px_3()
                                        .py_1()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child("No matching options")
                                        .into_any_element(),
                                );
                            }
                        }
                    }

                    current_label_ix = Some(ix);
                    group_matched_any = false;
                    let is_open = self.is_accordion_label_open(ix);

                    elements.push(self.render_item(ix, item, options, window, cx).into_any_element());

                    if self.accordion_groups && self.searchable_accordion && is_open {
                        let has_items = self.menu_items[ix + 1..]
                            .iter()
                            .take_while(|item| !matches!(item, PopupMenuItem::Label(_)))
                            .any(|item| item.is_clickable());

                        if has_items {
                            if let Some(input) = self.accordion_search_inputs.get(&ix) {
                                let q = input.read(cx).value().trim().to_lowercase();
                                current_query = Some(q);
                                in_searchable_group = true;

                                elements.push(
                                    div()
                                        .id(ElementId::Name(format!("accordion-search-wrap-{ix}").into()))
                                        .px_1()
                                        .py_0p5()
                                        .child(
                                            Input::new(input)
                                                .xsmall()
                                                .prefix(
                                                    Icon::new(IconName::Search)
                                                        .xsmall()
                                                        .text_color(cx.theme().muted_foreground),
                                                )
                                                .cleanable(true),
                                        )
                                        .into_any_element(),
                                );
                            } else {
                                current_query = None;
                                in_searchable_group = false;
                            }
                        } else {
                            current_query = None;
                            in_searchable_group = false;
                        }
                    } else {
                        current_query = None;
                        in_searchable_group = false;
                    }
                }
                _ => {
                    if self.accordion_groups {
                        let Some(label_ix) = current_label_ix else {
                            elements.push(self.render_item(ix, item, options, window, cx).into_any_element());
                            continue;
                        };

                        if !self.open_accordion_labels.contains(&label_ix) {
                            continue;
                        }

                        if let Some(ref q) = current_query {
                            if !q.is_empty() {
                                if item.is_separator() {
                                    continue;
                                }
                                if let Some(label) = item.label_text() {
                                    if !label.to_lowercase().contains(q) {
                                        continue;
                                    }
                                }
                            }
                        }
                    }

                    group_matched_any = true;
                    elements.push(self.render_item(ix, item, options, window, cx).into_any_element());
                }
            }
        }

        if in_searchable_group && !group_matched_any {
            if let Some(ref q) = current_query {
                if !q.is_empty() {
                    elements.push(
                        div()
                            .px_3()
                            .py_1()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("No matching options")
                            .into_any_element(),
                    );
                }
            }
        }

        elements
    }

    /// Calculate the anchor corner and left offset for child submenu
    fn update_submenu_menu_anchor(&mut self, window: &Window) {
        let bounds = self.bounds;
        let max_width = self.max_width();
        let (anchor, left) = if max_width + bounds.origin.x > window.bounds().size.width {
            (Corner::TopRight, -px(16.))
        } else {
            (Corner::TopLeft, bounds.size.width - px(8.))
        };

        let is_bottom_pos = bounds.origin.y + bounds.size.height > window.bounds().size.height;
        self.submenu_anchor = if is_bottom_pos {
            (anchor.other_side_corner_along(gpui::Axis::Vertical), left)
        } else {
            (anchor, left)
        };
    }

    fn render_item(
        &self,
        ix: usize,
        item: &PopupMenuItem,
        options: RenderOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> MenuItemElement {
        let has_left_icon = options.has_left_icon;
        let is_left_check = options.check_side.is_left() && item.is_checked();
        let right_check_icon = if options.check_side.is_right() && item.is_checked() {
            Some(Icon::new(IconName::Check).xsmall())
        } else {
            None
        };

        let selected = self.selected_index == Some(ix);
        const EDGE_PADDING: Pixels = px(4.);
        const INNER_PADDING: Pixels = px(8.);

        let is_submenu = matches!(item, PopupMenuItem::Submenu { .. });
        let group_name = format!("popup-menu-item-{}", ix);

        let (item_height, radius) = match self.size {
            Size::Small => (px(20.), options.radius.half()),
            _ => (px(26.), options.radius),
        };

        let this = MenuItemElement::new(ix, &group_name)
            .relative()
            .text_sm()
            .py_0()
            .px(INNER_PADDING)
            .rounded(radius)
            .items_center()
            .selected(selected)
            .on_hover(cx.listener(move |this, hovered, _, cx| {
                if *hovered {
                    this.selected_index = Some(ix);
                } else if !is_submenu && this.selected_index == Some(ix) {
                    // TODO: Better handle the submenu unselection when hover out
                    this.selected_index = None;
                }

                cx.notify();
            }));

        match item {
            PopupMenuItem::Separator => this
                .h_auto()
                .p_0()
                .my_0p5()
                .mx_neg_1()
                .h(px(1.))
                .bg(cx.theme().border)
                .disabled(true),
            PopupMenuItem::Label(label) => {
                let open = self.is_accordion_label_open(ix);
                this.cursor_default()
                    .when(self.accordion_groups, |this| {
                        this.on_click(cx.listener(move |this, _, _, cx| {
                            this.toggle_accordion_label(ix, cx)
                        }))
                    })
                    .when(!self.accordion_groups, |this| this.disabled(true))
                    .child(
                        h_flex()
                            .cursor_default()
                            .items_center()
                            .gap_x_1()
                            .children(Self::render_icon(has_left_icon, false, None, window, cx))
                            .child(div().flex_1().child(label.clone()))
                            .when(self.accordion_groups, |this| {
                                this.child(Icon::new(if open { IconName::ChevronUp } else { IconName::ChevronDown }).xsmall())
                            }),
                    )
            },
            PopupMenuItem::ElementItem {
                render,
                icon,
                disabled,
                ..
            } => this
                .when(!disabled, |this| {
                    this.on_click(
                        cx.listener(move |this, _, window, cx| this.on_click(ix, window, cx)),
                    )
                })
                .disabled(*disabled)
                .child(
                    h_flex()
                        .flex_1()
                        .min_h(item_height)
                        .items_center()
                        .gap_x_1()
                        .children(Self::render_icon(
                            has_left_icon,
                            is_left_check,
                            icon.clone(),
                            window,
                            cx,
                        ))
                        .child((render)(window, cx))
                        .children(right_check_icon.map(|icon| icon.ml_3())),
                ),
            PopupMenuItem::Item {
                icon,
                label,
                action,
                disabled,
                is_link,
                ..
            } => {
                let show_link_icon = *is_link && self.external_link_icon;
                let action = action.as_ref().map(|action| action.boxed_clone());
                let key = self.render_key_binding(action, window, cx);

                this.when(!disabled, |this| {
                    this.on_click(
                        cx.listener(move |this, _, window, cx| this.on_click(ix, window, cx)),
                    )
                })
                .disabled(*disabled)
                .h(item_height)
                .gap_x_1()
                .children(Self::render_icon(
                    has_left_icon,
                    is_left_check,
                    icon.clone(),
                    window,
                    cx,
                ))
                .child(
                    h_flex()
                        .w_full()
                        .gap_3()
                        .items_center()
                        .justify_between()
                        .when(!show_link_icon, |this| this.child(label.clone()))
                        .children(right_check_icon)
                        .when(show_link_icon, |this| {
                            this.child(
                                h_flex()
                                    .w_full()
                                    .justify_between()
                                    .gap_1p5()
                                    .child(label.clone())
                                    .child(
                                        Icon::new(IconName::ExternalLink)
                                            .xsmall()
                                            .text_color(cx.theme().muted_foreground),
                                    ),
                            )
                        })
                        .children(key),
                )
            }
            PopupMenuItem::Submenu {
                icon,
                label,
                menu,
                disabled,
            } => this
                .selected(selected)
                .disabled(*disabled)
                .items_start()
                .child(
                    h_flex()
                        .min_h(item_height)
                        .size_full()
                        .items_center()
                        .gap_x_1()
                        .children(Self::render_icon(
                            has_left_icon,
                            false,
                            icon.clone(),
                            window,
                            cx,
                        ))
                        .child(
                            h_flex()
                                .flex_1()
                                .gap_2()
                                .items_center()
                                .justify_between()
                                .child(label.clone())
                                .child(IconName::ChevronRight),
                        ),
                )
                .when(selected, |this| {
                    this.child({
                        let (anchor, left) = self.submenu_anchor;
                        let is_bottom_pos =
                            matches!(anchor, Corner::BottomLeft | Corner::BottomRight);
                        anchored()
                            .anchor(anchor)
                            .child(
                                div()
                                    .id("submenu")
                                    .occlude()
                                    .when(is_bottom_pos, |this| this.bottom_0())
                                    .when(!is_bottom_pos, |this| this.top_neg_1())
                                    .left(left)
                                    .child(menu.clone()),
                            )
                            .snap_to_window_with_margin(Edges::all(EDGE_PADDING))
                    })
                }),
        }
    }
}

impl FluentBuilder for PopupMenu {}
impl EventEmitter<DismissEvent> for PopupMenu {}
impl Focusable for PopupMenu {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

#[derive(Clone, Copy)]
struct RenderOptions {
    has_left_icon: bool,
    check_side: Side,
    radius: Pixels,
}

impl Render for PopupMenu {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.update_submenu_menu_anchor(window);

        let view = cx.entity().clone();
        let items_count = self.menu_items.len();

        let max_height = self.max_height.unwrap_or_else(|| {
            let window_half_height = window.window_bounds().get_bounds().size.height * 0.5;
            window_half_height.min(px(450.))
        });

        let has_left_icon = self
            .menu_items
            .iter()
            .any(|item| item.has_left_icon(self.check_side));

        let max_width = self.max_width();
        let options = RenderOptions {
            has_left_icon,
            check_side: self.check_side,
            radius: cx.theme().radius.min(px(8.)),
        };

        v_flex()
            .id("popup-menu")
            .key_context(CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::select_up))
            .on_action(cx.listener(Self::select_down))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::dismiss))
            .on_mouse_down_out(cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                // Do not dismiss, if click inside the parent menu
                if let Some(parent) = this.parent_menu.as_ref() {
                    if let Some(parent) = parent.upgrade() {
                        if parent.read(cx).bounds.contains(&ev.position) {
                            return;
                        }
                    }
                }

                this.dismiss(&Cancel, window, cx);
            }))
            .popover_style(cx)
            .text_color(cx.theme().popover_foreground)
            .relative()
            .child(
                v_flex()
                    .id("items")
                    .p_1()
                    .gap_y_0p5()
                    .min_w(if self.searchable_accordion { rems(16.) } else { rems(8.) })
                    .when_some(self.min_width, |this, min_width| this.min_w(min_width))
                    .max_w(max_width)
                    .when(self.scrollable, |this| {
                        this.max_h(max_height)
                            .overflow_y_scroll()
                            .track_scroll(&self.scroll_handle)
                    })
                    .children(self.render_items(items_count, options, window, cx))
                    .child({
                        canvas(
                            move |bounds, _, cx| view.update(cx, |r, _| r.bounds = bounds),
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .size_full()
                    }),
            )
            .when(self.scrollable, |this| {
                // TODO: When the menu is limited by `overflow_y_scroll`, the sub-menu will cannot be displayed.
                this.vertical_scrollbar(&self.scroll_handle)
            })
    }
}
