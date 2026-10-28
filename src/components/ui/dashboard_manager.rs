//! Compact editor-style dashboard explorer for FinTer.
//! Provides a file tree view reminiscent of modern code editors (VS Code, Zed, Sublime),
//! featuring collapsible folders, tree guides, row action hover states, and status badges.

use crate::data::models::{LayoutPreset, Module};
use gpui::{
    actions, div, prelude::*, px, App, Context, EventEmitter, FocusHandle, Focusable,
    FontWeight, InteractiveElement, IntoElement, MouseButton, ParentElement, Render,
    StatefulInteractiveElement, Styled, Window,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::{ActiveTheme, Icon, Sizable};

actions!(dashboard_manager, [RefreshDashboards]);

pub enum DashboardManagerEvent {
    LoadModule(String),
    LoadPreset(serde_json::Value),
    ExportDashboard,
    ImportDashboard(Option<String>),
    CreateDashboard(String),
    DeleteDashboard(String, String),
    OpenCreateDialog,
    OpenDeleteDialog(String, String),
    DeletePreset(String),
    CloseDashboard(String),
    DetachDashboard(String),
}

pub struct DashboardManager {
    focus_handle: FocusHandle,
    modules: Vec<Module>,
    layout_presets: Vec<LayoutPreset>,
    active_module: Option<String>,
    open_module_ids: Vec<String>,
    dashboards_expanded: bool,
    presets_expanded: bool,
    pub api_error: Option<String>,
}

impl DashboardManager {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            modules: Vec::new(),
            layout_presets: Vec::new(),
            active_module: None,
            open_module_ids: Vec::new(),
            dashboards_expanded: true,
            presets_expanded: true,
            api_error: None,
        }
    }

    pub fn update_error(&mut self, err: String, cx: &mut Context<Self>) {
        self.api_error = Some(err);
        cx.notify();
    }

    pub fn update_modules(&mut self, modules: Vec<Module>, cx: &mut Context<Self>) {
        self.modules = modules;
        cx.notify();
    }

    pub fn update_layout_presets(&mut self, presets: Vec<LayoutPreset>, cx: &mut Context<Self>) {
        self.layout_presets = presets;
        cx.notify();
    }

    pub fn set_active_module(&mut self, module_id: String, cx: &mut Context<Self>) {
        self.active_module = Some(module_id);
        cx.notify();
    }

    pub fn clear_active_module(&mut self, cx: &mut Context<Self>) {
        self.active_module = None;
        cx.notify();
    }

    pub fn set_open_modules(&mut self, open_ids: Vec<String>, cx: &mut Context<Self>) {
        self.open_module_ids = open_ids;
        cx.notify();
    }

    pub fn active_module(&self) -> Option<&String> {
        self.active_module.as_ref()
    }
}

impl EventEmitter<DashboardManagerEvent> for DashboardManager {}

impl Focusable for DashboardManager {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for DashboardManager {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let active_id = self.active_module.clone();
        let open_ids = self.open_module_ids.clone();

        // Render dashboard file items
        let module_rows: Vec<_> = self
            .modules
            .clone()
            .into_iter()
            .map(|module| {
                let id = module.id.clone().unwrap_or_default();
                let delete_id = id.clone();
                let name = module.module_name.clone();
                let is_active = active_id.as_ref() == Some(&id);
                let is_open = open_ids.contains(&id);

                div()
                    .id(gpui::ElementId::Name(gpui::SharedString::from(format!(
                        "file-tree-dash-{id}"
                    ))))
                    .flex()
                    .items_center()
                    .justify_between()
                    .h(px(28.0))
                    .pl(px(26.0))
                    .pr(px(8.0))
                    .cursor_pointer()
                    .border_l_2()
                    .border_color(if is_active {
                        colors.primary
                    } else {
                        gpui::transparent_black()
                    })
                    .bg(if is_active {
                        colors.list_active
                    } else {
                        gpui::transparent_black()
                    })
                    .hover(|style| {
                        if !is_active {
                            style.bg(colors.list_hover)
                        } else {
                            style
                        }
                    })
                    .on_click(cx.listener({
                        let id = id.clone();
                        move |_this, _event, _window, cx| {
                            cx.emit(DashboardManagerEvent::LoadModule(id.clone()));
                        }
                    }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .overflow_hidden()
                            .child(
                                Icon::default()
                                    .path("icons/layout-dashboard.svg")
                                    .xsmall()
                                    .text_color(if is_active {
                                        colors.primary
                                    } else {
                                        colors.muted_foreground
                                    }),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(if is_active {
                                        FontWeight::SEMIBOLD
                                    } else {
                                        FontWeight::NORMAL
                                    })
                                    .text_color(if is_active {
                                        colors.foreground
                                    } else {
                                        colors.muted_foreground
                                    })
                                    .overflow_hidden()
                                    .child(name.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(10.0))
                                    .text_color(colors.muted_foreground.opacity(0.6))
                                    .child(".dash"),
                            )
                            .when(is_open && !is_active, |this| {
                                this.child(
                                    div()
                                        .w(px(5.0))
                                        .h(px(5.0))
                                        .bg(colors.muted_foreground.opacity(0.4)),
                                )
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(2.0))
                            .child(
                                div()
                                    .id(gpui::ElementId::Name(gpui::SharedString::from(format!(
                                        "del-btn-{delete_id}"
                                    ))))
                                    .w(px(20.0))
                                    .h(px(20.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .cursor_pointer()
                                    .hover(|style| {
                                        style
                                            .bg(colors.danger.opacity(0.15))
                                            .text_color(colors.danger)
                                    })
                                    .text_color(colors.muted_foreground.opacity(0.5))
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                        cx.stop_propagation();
                                    })
                                    .on_click(cx.listener({
                                        let delete_id = delete_id.clone();
                                        let name = name.clone();
                                        move |_this, _event, _window, cx| {
                                            cx.stop_propagation();
                                            cx.emit(DashboardManagerEvent::OpenDeleteDialog(
                                                delete_id.clone(),
                                                name.clone(),
                                            ));
                                        }
                                    }))
                                    .child(Icon::default().path("icons/delete.svg").xsmall()),
                            ),
                    )
            })
            .collect();

        // Render layout preset items
        let preset_rows: Vec<_> = self
            .layout_presets
            .clone()
            .into_iter()
            .map(|preset| {
                let id = preset.id.clone().unwrap_or_default();
                let tree = preset.tree;
                let name = preset.name;

                div()
                    .id(gpui::ElementId::Name(gpui::SharedString::from(format!(
                        "file-tree-preset-{id}"
                    ))))
                    .flex()
                    .items_center()
                    .justify_between()
                    .h(px(28.0))
                    .pl(px(26.0))
                    .pr(px(8.0))
                    .cursor_pointer()
                    .hover(|style| style.bg(colors.list_hover))
                    .on_click(cx.listener({
                        let tree = tree.clone();
                        move |_this, _event, _window, cx| {
                            cx.emit(DashboardManagerEvent::LoadPreset(tree.clone()));
                        }
                    }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .overflow_hidden()
                            .child(
                                Icon::default()
                                    .path("icons/blocks.svg")
                                    .xsmall()
                                    .text_color(colors.muted_foreground),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(colors.muted_foreground)
                                    .overflow_hidden()
                                    .child(name),
                            )
                            .child(
                                div()
                                    .text_size(px(10.0))
                                    .text_color(colors.muted_foreground.opacity(0.6))
                                    .child(".preset"),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(2.0))
                            .child(
                                div()
                                    .id(gpui::ElementId::Name(gpui::SharedString::from(format!(
                                        "del-preset-{id}"
                                    ))))
                                    .w(px(20.0))
                                    .h(px(20.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .cursor_pointer()
                                    .hover(|style| {
                                        style
                                            .bg(colors.danger.opacity(0.15))
                                            .text_color(colors.danger)
                                    })
                                    .text_color(colors.muted_foreground.opacity(0.5))
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                        cx.stop_propagation();
                                    })
                                    .on_click(cx.listener({
                                        let id = id.clone();
                                        move |_this, _event, _window, cx| {
                                            cx.stop_propagation();
                                            cx.emit(DashboardManagerEvent::DeletePreset(id.clone()));
                                        }
                                    }))
                                    .child(Icon::default().path("icons/delete.svg").xsmall()),
                            ),
                    )
            })
            .collect();

        div()
            .id("dashboard-file-explorer")
            .size_full()
            .flex()
            .flex_col()
            .bg(colors.sidebar)
            .text_color(colors.foreground)
            // Explorer Toolbar Header
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .h(px(32.0))
                    .px(px(12.0))
                    .border_b_1()
                    .border_color(colors.border.opacity(0.5))
                    .child(
                        div()
                            .text_size(px(10.0))
                            .font_weight(FontWeight::BOLD)
                            .text_color(colors.muted_foreground)
                            .child("WORKSPACE EXPLORER"),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .child(
                                Button::new("toolbar-new-dashboard")
                                    .icon(Icon::default().path("icons/plus.svg"))
                                    .xsmall()
                                    .ghost()
                                    .on_click(cx.listener(|_this, _event, _window, cx| {
                                        cx.emit(DashboardManagerEvent::CreateDashboard("Untitled Dashboard".to_string()));
                                    })),
                            )
                            .child(
                                Button::new("toolbar-import")
                                    .icon(Icon::default().path("icons/undo-2.svg"))
                                    .xsmall()
                                    .ghost()
                                    .on_click(cx.listener(|_this, _event, _window, cx| {
                                        cx.emit(DashboardManagerEvent::ImportDashboard(None));
                                    })),
                            )
                            .child(
                                Button::new("toolbar-export")
                                    .icon(Icon::default().path("icons/external-link.svg"))
                                    .xsmall()
                                    .ghost()
                                    .on_click(cx.listener(|_this, _event, _window, cx| {
                                        cx.emit(DashboardManagerEvent::ExportDashboard);
                                    })),
                            ),
                    ),
            )
            // Tree View Area with vertical guides
            .child(
                div()
                    .id("explorer-tree-scroll")
                    .flex_1()
                    .overflow_y_scroll()
                    .py(px(6.0))
                    // Section 1: Dashboards Folder
                    .child(
                        div()
                            .id("dashboards-folder-header")
                            .flex()
                            .items_center()
                            .justify_between()
                            .h(px(26.0))
                            .px(px(8.0))
                            .cursor_pointer()
                            .hover(|style| style.bg(colors.list_hover))
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.dashboards_expanded = !this.dashboards_expanded;
                                cx.notify();
                            }))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(4.0))
                                    .child(
                                        Icon::default()
                                            .path(if self.dashboards_expanded {
                                                "icons/chevron-down.svg"
                                            } else {
                                                "icons/chevron-right.svg"
                                            })
                                            .xsmall()
                                            .text_color(colors.muted_foreground),
                                    )
                                    .child(
                                        Icon::default()
                                            .path(if self.dashboards_expanded {
                                                "icons/folder-open.svg"
                                            } else {
                                                "icons/folder-closed.svg"
                                            })
                                            .xsmall()
                                            .text_color(colors.primary),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(colors.foreground)
                                            .child("dashboards"),
                                    ),
                            )
                            .child(
                                div()
                                    .px(px(6.0))
                                    .py(px(1.0))
                                    .bg(colors.border.opacity(0.3))
                                    .text_size(px(10.0))
                                    .text_color(colors.muted_foreground)
                                    .child(self.modules.len().to_string()),
                            ),
                    )
                    // Dashboards children container with indentation guide
                    .when(self.dashboards_expanded, |el| {
                        el.child(
                            div()
                                .relative()
                                .child(
                                    // Subtle vertical tree hierarchy guide
                                    div()
                                        .absolute()
                                        .top(px(0.0))
                                        .bottom(px(0.0))
                                        .left(px(15.0))
                                        .w(px(1.0))
                                        .bg(colors.border.opacity(0.4)),
                                )
                                .children(module_rows)
                                .when(self.modules.is_empty(), |empty_el| {
                                    empty_el.child(
                                        div()
                                            .pl(px(32.0))
                                            .py(px(8.0))
                                            .pr(px(12.0))
                                            .text_xs()
                                            .text_color(colors.muted_foreground.opacity(0.7))
                                            .child("No dashboards found. Click + to create one."),
                                    )
                                }),
                        )
                    })
                    // Section 2: Layout Presets Folder
                    .child(
                        div()
                            .id("presets-folder-header")
                            .flex()
                            .items_center()
                            .justify_between()
                            .h(px(26.0))
                            .px(px(8.0))
                            .mt(px(4.0))
                            .cursor_pointer()
                            .hover(|style| style.bg(colors.list_hover))
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.presets_expanded = !this.presets_expanded;
                                cx.notify();
                            }))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(4.0))
                                    .child(
                                        Icon::default()
                                            .path(if self.presets_expanded {
                                                "icons/chevron-down.svg"
                                            } else {
                                                "icons/chevron-right.svg"
                                            })
                                            .xsmall()
                                            .text_color(colors.muted_foreground),
                                    )
                                    .child(
                                        Icon::default()
                                            .path(if self.presets_expanded {
                                                "icons/folder-open.svg"
                                            } else {
                                                "icons/folder-closed.svg"
                                            })
                                            .xsmall()
                                            .text_color(colors.muted_foreground),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(colors.foreground)
                                            .child("presets"),
                                    ),
                            )
                            .child(
                                div()
                                    .px(px(6.0))
                                    .py(px(1.0))
                                    .bg(colors.border.opacity(0.3))
                                    .text_size(px(10.0))
                                    .text_color(colors.muted_foreground)
                                    .child(self.layout_presets.len().to_string()),
                            ),
                    )
                    // Presets children container with indentation guide
                    .when(self.presets_expanded, |el| {
                        el.child(
                            div()
                                .relative()
                                .child(
                                    div()
                                        .absolute()
                                        .top(px(0.0))
                                        .bottom(px(0.0))
                                        .left(px(15.0))
                                        .w(px(1.0))
                                        .bg(colors.border.opacity(0.4)),
                                )
                                .children(preset_rows)
                                .when(self.layout_presets.is_empty(), |empty_el| {
                                    empty_el.child(
                                        div()
                                            .pl(px(32.0))
                                            .py(px(8.0))
                                            .pr(px(12.0))
                                            .text_xs()
                                            .text_color(colors.muted_foreground.opacity(0.7))
                                            .child("No layout presets available."),
                                    )
                                }),
                        )
                    })
                    // API Error notification banner if present
                    .when_some(self.api_error.clone(), |el, error| {
                        el.child(
                            div()
                                .m(px(10.0))
                                .p(px(8.0))
                                .bg(colors.danger.opacity(0.1))
                                .border_1()
                                .border_color(colors.danger.opacity(0.3))
                                .text_xs()
                                .text_color(colors.danger)
                                .child(error),
                        )
                    }),
            )
            // Footer status line
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .h(px(26.0))
                    .px(px(10.0))
                    .border_t_1()
                    .border_color(colors.border.opacity(0.4))
                    .text_size(px(10.0))
                    .text_color(colors.muted_foreground)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .child(Icon::default().path("icons/hard-drive.svg").xsmall())
                            .child(format!("{} items", self.modules.len())),
                    )
                    .child(
                        Button::new("footer-new-dash")
                            .label("+ New")
                            .xsmall()
                            .ghost()
                            .on_click(cx.listener(|_this, _event, _window, cx| {
                                cx.emit(DashboardManagerEvent::CreateDashboard("Untitled Dashboard".to_string()));
                            })),
                    ),
            )
    }
}
