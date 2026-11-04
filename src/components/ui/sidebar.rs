use crate::components::mainview::{MainView, Page, SecondarySidebar};
use crate::components::ui::title_bar::TITLE_BAR_HEIGHT;
use gpui::{
    prelude::FluentBuilder as _, App, AppContext, ClickEvent, Context, Corner, Hsla,
    InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement,
    StyleRefinement, Styled, StyledImage, WeakEntity, Window, WindowControlArea, div, img, px,
};
use gpui_component::Icon;
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::menu::{DropdownMenu, PopupMenu, PopupMenuItem};
use gpui_component::theme::ActiveTheme;

/// User info for the sidebar avatar
#[derive(Clone, Default)]
pub struct SidebarUserInfo {
    pub email: Option<String>,
    pub name: Option<String>,
    pub avatar_url: Option<String>,
}

/// Icon-only sidebar width (48px = 3rem)
const ICON_SIDEBAR_WIDTH: f32 = 48.0;

/// Secondary sidebar width (256px = 16rem)
const SECONDARY_SIDEBAR_WIDTH: f32 = 256.0;

/// A single sidebar menu item with icon, optional label, and click handler
struct SidebarItem {
    id: &'static str,
    icon_path: &'static str,
    #[allow(dead_code)]
    label: &'static str,
    sidebar_type: Option<SecondarySidebar>,
}

impl SidebarItem {
    const fn new(
        id: &'static str,
        icon_path: &'static str,
        label: &'static str,
        sidebar_type: Option<SecondarySidebar>,
    ) -> Self {
        Self {
            id,
            icon_path,
            label,
            sidebar_type,
        }
    }
}

/// Main navigation items (top section)
const MAIN_NAV_ITEMS: &[SidebarItem] = &[
    SidebarItem::new(
        "dashboard",
        "icons/layout-dashboard.svg",
        "Dashboard",
        Some(SecondarySidebar::Dashboard),
    ),
    SidebarItem::new(
        "alerts",
        "icons/alarm-clock.svg",
        "Alerts",
        Some(SecondarySidebar::Notifications),
    ),
    SidebarItem::new(
        "feeds",
        "icons/megaphone.svg",
        "Feeds",
        Some(SecondarySidebar::Feeds),
    ),
];

/// Overlay/drawing tools (bottom section before user)
#[allow(dead_code)]
const OVERLAY_TOOLS: &[SidebarItem] = &[
    SidebarItem::new("cursor", "icons/mouse-pointer.svg", "Cursor", None),
    SidebarItem::new("crosshair", "icons/crosshair.svg", "Crosshair", None),
    SidebarItem::new("trend-line", "icons/trending-up.svg", "Trend Line", None),
    SidebarItem::new("magnet", "icons/magnet.svg", "Magnet", None),
    SidebarItem::new("trash", "icons/delete.svg", "Delete", None),
];

/// Build a single icon button for the sidebar
fn build_icon_button(
    item: &SidebarItem,
    is_active: bool,
    main_view_entity: WeakEntity<MainView>,
    sidebar_accent: Hsla,
    sidebar_fg: Hsla,
    muted_fg: Hsla,
) -> impl IntoElement {
    let entity = main_view_entity.clone();
    let sidebar_type = item.sidebar_type;
    let icon_path = item.icon_path;

    let base = div()
        .id(item.id)
        .w(px(28.0))
        .h(px(28.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.0))
        .cursor_pointer();

    let styled = if is_active {
        base.bg(sidebar_accent)
    } else {
        base
    };

    let with_hover = styled.hover(move |style: StyleRefinement| style.bg(sidebar_accent));

    let with_icon = with_hover.child(
        Icon::default()
            .path(icon_path)
            .size_4()
            .text_color(if is_active { sidebar_fg } else { muted_fg }),
    );

    if let Some(st) = sidebar_type {
        with_icon.on_click(move |_: &ClickEvent, _: &mut Window, app: &mut App| {
            if let Some(e) = entity.upgrade() {
                app.update_entity(&e, |view: &mut MainView, cx: &mut Context<MainView>| {
                    view.toggle_secondary_sidebar(st, cx);
                    cx.notify();
                });
            }
        })
    } else {
        with_icon
    }
}

/// Build the logo/brand element
fn build_logo(sidebar_primary: Hsla, sidebar_primary_fg: Hsla) -> impl IntoElement {
    div()
        .w(px(28.0))
        .h(px(28.0))
        .flex()
        .items_center()
        .justify_center()
        .bg(sidebar_primary)
        .rounded(px(4.0))
        .child(
            Icon::default()
                .path("icons/bull-mark.svg")
                .size_6()
                .text_color(sidebar_primary_fg),
        )
}

/// Build a horizontal separator
fn build_separator(sidebar_border: Hsla) -> impl IntoElement {
    div()
        .w(px(24.0))
        .h(px(1.0))
        .mx_auto()
        .my(px(4.0))
        .bg(sidebar_border)
}

/// Extract initials from a name or email
fn get_user_initials(user_info: &SidebarUserInfo) -> String {
    // Try to get initials from name first
    if let Some(name) = &user_info.name {
        let parts: Vec<&str> = name.split_whitespace().collect();
        match parts.len() {
            0 => {}
            1 => {
                return parts[0]
                    .chars()
                    .next()
                    .unwrap_or('?')
                    .to_uppercase()
                    .to_string();
            }
            _ => {
                let first = parts[0].chars().next().unwrap_or('?');
                let last = parts[parts.len() - 1].chars().next().unwrap_or('?');
                return format!("{}{}", first.to_uppercase(), last.to_uppercase());
            }
        }
    }

    // Fall back to email
    if let Some(email) = &user_info.email {
        return email
            .chars()
            .next()
            .unwrap_or('G')
            .to_uppercase()
            .to_string();
    }

    "G".to_string()
}

/// Build the icon-only sidebar (left-most, always visible)
#[allow(clippy::too_many_arguments)]
fn build_icon_sidebar(
    main_view_entity: WeakEntity<MainView>,
    _current_page: Page,
    active_secondary_sidebar: Option<SecondarySidebar>,
    user_info: SidebarUserInfo,
    sidebar_bg: Hsla,
    sidebar_fg: Hsla,
    sidebar_accent: Hsla,
    sidebar_border: Hsla,
    sidebar_primary: Hsla,
    sidebar_primary_fg: Hsla,
    muted_fg: Hsla,
) -> impl IntoElement {
    let is_macos = cfg!(target_os = "macos");
    let icon_sidebar = div()
        .w(px(ICON_SIDEBAR_WIDTH))
        .h_full()
        .flex_shrink_0()
        .bg(sidebar_bg)
        .border_r_1()
        .border_color(sidebar_border);

    if is_macos {
        icon_sidebar.border_l_1().border_color(sidebar_border)
    } else {
        icon_sidebar
    }
    .flex()
    .flex_col()
    .child(
        // Top section: Logo header + main nav
        div()
            .flex()
            .flex_col()
            .items_center()
            .child(
                // Logo Header (matches TITLE_BAR_HEIGHT) — draggable like title bar
                div()
                    .id("sidebar-logo-header")
                    .window_control_area(WindowControlArea::Drag)
                    .when(cfg!(target_os = "linux"), |this| {
                        this.on_mouse_down(gpui::MouseButton::Left, |_, window, _| {
                            window.start_window_move();
                        })
                    })
                    .w_full()
                    .h(TITLE_BAR_HEIGHT)
                    .flex()
                    .items_center()
                    .justify_center()
                    .border_b_1()
                    .border_color(sidebar_border)
                    .child(build_logo(sidebar_primary, sidebar_primary_fg)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(4.0))
                    .py(px(8.0))
                    .children(MAIN_NAV_ITEMS.iter().map(|item| {
                        let is_active = active_secondary_sidebar == item.sidebar_type;
                        build_icon_button(
                            item,
                            is_active,
                            main_view_entity.clone(),
                            sidebar_accent,
                            sidebar_fg,
                            muted_fg,
                        )
                    })),
            ),
    )
    .child(
        // Spacer
        div().flex_1(),
    )
    .child(
        // Bottom section: Overlay tools + user
        div()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(4.0))
            .py(px(8.0))
            // .children(OVERLAY_TOOLS.iter().map(|item| {
            //     build_icon_button(
            //         item,
            //         false,
            //         main_view_entity.clone(),
            //         sidebar_accent,
            //         sidebar_fg,
            //         muted_fg,
            //     )
            // }))
            .child(build_separator(sidebar_border))
            .child(build_user_button(
                main_view_entity.clone(),
                user_info,
                sidebar_accent,
                sidebar_fg,
                sidebar_primary,
                sidebar_primary_fg,
            )),
    )
}

/// Build the user avatar/profile button at the bottom of the sidebar
fn build_user_button(
    main_view_entity: WeakEntity<MainView>,
    user_info: SidebarUserInfo,
    sidebar_accent: Hsla,
    _sidebar_fg: Hsla,
    sidebar_primary: Hsla,
    sidebar_primary_fg: Hsla,
) -> impl IntoElement {
    let initials = get_user_initials(&user_info);
    let avatar_url = user_info.avatar_url.clone();

    // Avatar container - rounded square (6px radius matching logo)
    let avatar_content = match avatar_url {
        Some(url) => div()
            .id("avatar-img-container")
            .w(px(28.0))
            .h(px(28.0))
            .rounded(px(6.0))
            .overflow_hidden()
            .flex_shrink_0()
            .bg(sidebar_accent)
            .child(
                img(url)
                    .id("avatar-img")
                    .w(px(28.0))
                    .h(px(28.0))
                    .rounded(px(6.0))
                    .object_fit(gpui::ObjectFit::Cover),
            ),
        None => div()
            .id("avatar-initials-container")
            .w(px(28.0))
            .h(px(28.0))
            .rounded(px(6.0))
            .overflow_hidden()
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(sidebar_primary)
            .text_color(sidebar_primary_fg)
            .text_size(px(11.0))
            .font_weight(gpui::FontWeight::BOLD)
            .child(initials),
    };

    div()
        .w(px(28.0))
        .h(px(28.0))
        .flex()
        .items_center()
        .justify_center()
        .child(
            // Ghost button that contains the avatar, with dropdown menu
            Button::new("avatar-menu")
                .ghost()
                .p(px(0.0))
                .rounded(px(6.0))
                .w(px(28.0))
                .h(px(28.0))
                .child(avatar_content)
                .dropdown_menu_side(Corner::BottomLeft, {
                    let entity = main_view_entity.clone();
                    let is_logged_in = user_info.email.is_some();
                    move |menu: PopupMenu, _window: &mut Window, _cx: &mut Context<PopupMenu>| {
                        let entity_profile = entity.clone();
                        let entity_login = entity.clone();
                        let entity_settings = entity.clone();
                        let entity_logout = entity.clone();

                        let mut menu = menu;
                        if is_logged_in {
                            menu = menu.item(
                                PopupMenuItem::new("Profile")
                                    .icon(Icon::default().path("icons/user.svg"))
                                    .on_click(move |_event, _window, app| {
                                        if let Some(e) = entity_profile.upgrade() {
                                            app.update_entity(
                                                &e,
                                                |view: &mut MainView, cx: &mut Context<MainView>| {
                                                    view.switch_page(Page::Profile);
                                                    cx.notify();
                                                },
                                            );
                                        }
                                    }),
                            );
                        } else {
                            menu = menu.item(
                                PopupMenuItem::new("Sign In")
                                    .icon(Icon::default().path("icons/user.svg"))
                                    .on_click(move |_event, _window, app| {
                                        if let Some(e) = entity_login.upgrade() {
                                            app.update_entity(
                                                &e,
                                                |view: &mut MainView, cx: &mut Context<MainView>| {
                                                    view.switch_page(Page::Login);
                                                    cx.notify();
                                                },
                                            );
                                        }
                                    }),
                            );
                        }

                        menu = menu
                            .item(
                                PopupMenuItem::new("Settings")
                                    .icon(Icon::default().path("icons/settings.svg"))
                                    .on_click(move |_event, _window, app| {
                                        if let Some(e) = entity_settings.upgrade() {
                                            app.update_entity(
                                                &e,
                                                |view: &mut MainView, cx: &mut Context<MainView>| {
                                                    view.switch_page(Page::Settings);
                                                    cx.notify();
                                                },
                                            );
                                        }
                                    }),
                            );

                        if is_logged_in {
                            menu = menu.separator().item(
                                PopupMenuItem::new("Sign Out")
                                    .icon(Icon::default().path("icons/log-out.svg"))
                                    .on_click(move |_event, _window, app| {
                                        if let Some(e) = entity_logout.upgrade() {
                                            app.update_entity(
                                                &e,
                                                |view: &mut MainView, cx: &mut Context<MainView>| {
                                                    view.do_logout(cx);
                                                },
                                            );
                                        }
                                    }),
                            );
                        }

                        menu
                    }
                }),
        )
}

/// Get the secondary sidebar width
pub fn get_secondary_sidebar_width() -> f32 {
    SECONDARY_SIDEBAR_WIDTH
}

/// Build the complete sidebar element (icon sidebar + optional secondary sidebar)
/// This is the main entry point called from MainView
pub fn build_sidebar_element(
    main_view_entity: WeakEntity<MainView>,
    current_page: Page,
    active_secondary_sidebar: Option<SecondarySidebar>,
    user_info: &SidebarUserInfo,
) -> impl IntoElement {
    // Create a wrapper that will get theme colors when rendered
    SidebarWrapper {
        main_view_entity,
        current_page,
        active_secondary_sidebar,
        user_info: user_info.clone(),
    }
}

/// Wrapper struct to defer theme access to render time
struct SidebarWrapper {
    main_view_entity: WeakEntity<MainView>,
    current_page: Page,
    active_secondary_sidebar: Option<SecondarySidebar>,
    user_info: SidebarUserInfo,
}

impl IntoElement for SidebarWrapper {
    type Element = gpui::Component<Self>;

    fn into_element(self) -> Self::Element {
        gpui::Component::new(self)
    }
}

impl gpui::RenderOnce for SidebarWrapper {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();

        // Extract theme colors
        let sidebar_bg = theme.colors.sidebar;
        let sidebar_fg = theme.colors.sidebar_foreground;
        let sidebar_accent = theme.colors.sidebar_accent;
        let sidebar_border = theme.colors.sidebar_border;
        let sidebar_primary = theme.colors.sidebar_primary;
        let sidebar_primary_fg = theme.colors.sidebar_primary_foreground;
        let muted_fg = theme.colors.muted_foreground;

        build_icon_sidebar(
            self.main_view_entity,
            self.current_page,
            self.active_secondary_sidebar,
            self.user_info,
            sidebar_bg,
            sidebar_fg,
            sidebar_accent,
            sidebar_border,
            sidebar_primary,
            sidebar_primary_fg,
            muted_fg,
        )
    }
}
