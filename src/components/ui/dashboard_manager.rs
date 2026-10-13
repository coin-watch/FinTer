//! Dashboard manager for cloud dashboards and saved layout presets.

use crate::data::models::{LayoutPreset, Module};
use gpui::{
    App, Context, EventEmitter, FocusHandle, Focusable, InteractiveElement, IntoElement,
    ParentElement, Render, Styled, Window, actions, div, prelude::*, px,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::theme::ActiveTheme;
use gpui_component::{Selectable, Sizable};

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
}

pub struct DashboardManager {
    focus_handle: FocusHandle,
    modules: Vec<Module>,
    layout_presets: Vec<LayoutPreset>,
    active_module: Option<String>,
    pub api_error: Option<String>,
}

impl DashboardManager {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            modules: Vec::new(),
            layout_presets: Vec::new(),
            active_module: None,
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

    pub fn active_module(&self) -> Option<&String> {
        self.active_module.as_ref()
    }

    #[allow(clippy::too_many_arguments)]
    fn render_module_item(
        &self,
        module: Module,
        cx: &mut Context<Self>,
        is_active: bool,
        list_bg: gpui::Hsla,
        border_color: gpui::Hsla,
        list_hover: gpui::Hsla,
        primary: gpui::Hsla,
        muted_fg: gpui::Hsla,
        foreground: gpui::Hsla,
    ) -> gpui::Stateful<gpui::Div> {
        let module_id = module.id.clone().unwrap_or_default();
        let module_id_for_delete = module_id.clone();
        let module_name = module.module_name.clone();
        let module_name_for_delete = module_name.clone();

        div()
            .id(gpui::ElementId::Name(gpui::SharedString::from(format!(
                "module-{module_id}"
            ))))
            .flex()
            .flex_col()
            .gap(px(6.0))
            .p(px(10.0))
            .rounded(px(6.0))
            .cursor_pointer()
            .bg(list_bg)
            .border_1()
            .border_color(if is_active { primary } else { border_color })
            .hover(|style| style.bg(list_hover))
            .on_click(cx.listener(move |_this, _event, _window, cx| {
                cx.emit(DashboardManagerEvent::LoadModule(module_id.clone()));
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .w(px(8.0))
                                    .h(px(8.0))
                                    .rounded(px(4.0))
                                    .bg(if is_active { primary } else { muted_fg }),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(foreground)
                                    .font_weight(if is_active {
                                        gpui::FontWeight::SEMIBOLD
                                    } else {
                                        gpui::FontWeight::NORMAL
                                    })
                                    .child(module_name),
                            ),
                    )
                    .child(
                        Button::new(gpui::SharedString::from(format!(
                            "delete-module-{module_id_for_delete}"
                        )))
                        .icon(gpui_component::Icon::default().path("icons/delete.svg"))
                        .xsmall()
                        .ghost()
                        .on_click(cx.listener(
                            move |_this, _event, _window, cx| {
                                cx.emit(DashboardManagerEvent::OpenDeleteDialog(
                                    module_id_for_delete.clone(),
                                    module_name_for_delete.clone(),
                                ));
                            },
                        )),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(muted_fg)
                    .child("Cloud dashboard"),
            )
    }

    #[allow(clippy::too_many_arguments)]
    fn render_preset_item(
        &self,
        preset: LayoutPreset,
        cx: &mut Context<Self>,
        list_bg: gpui::Hsla,
        border_color: gpui::Hsla,
        list_hover: gpui::Hsla,
        primary: gpui::Hsla,
        foreground: gpui::Hsla,
    ) -> gpui::Stateful<gpui::Div> {
        let preset_id = preset.id.clone().unwrap_or_default();
        let delete_id = preset_id.clone();
        let tree = preset.tree.clone();

        div()
            .id(gpui::ElementId::Name(gpui::SharedString::from(format!(
                "preset-{preset_id}"
            ))))
            .flex()
            .items_center()
            .justify_between()
            .gap(px(6.0))
            .px(px(10.0))
            .py(px(8.0))
            .rounded(px(6.0))
            .bg(list_bg)
            .border_1()
            .border_color(border_color)
            .hover(|style| style.bg(list_hover))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(div().w(px(8.0)).h(px(8.0)).rounded(px(4.0)).bg(primary))
                    .child(div().text_sm().text_color(foreground).child(preset.name)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .child(
                        Button::new(gpui::SharedString::from(format!("load-preset-{preset_id}")))
                            .label("Load")
                            .xsmall()
                            .secondary_selected(false)
                            .on_click(cx.listener(move |_this, _event, _window, cx| {
                                cx.emit(DashboardManagerEvent::LoadPreset(tree.clone()));
                            })),
                    )
                    .child(
                        Button::new(gpui::SharedString::from(format!(
                            "delete-preset-{delete_id}"
                        )))
                        .icon(gpui_component::Icon::default().path("icons/delete.svg"))
                        .xsmall()
                        .ghost()
                        .on_click(cx.listener(
                            move |_this, _event, _window, cx| {
                                cx.emit(DashboardManagerEvent::DeletePreset(delete_id.clone()));
                            },
                        )),
                    ),
            )
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
        let module_count = self.modules.len();
        let preset_count = self.layout_presets.len();
        let colors = cx.theme().colors;

        let mut module_items = Vec::new();
        for module in self.modules.clone() {
            let is_active = self
                .active_module
                .as_ref()
                .map(|id| module.id.as_ref() == Some(id))
                .unwrap_or(false);
            module_items.push(self.render_module_item(
                module,
                cx,
                is_active,
                if is_active {
                    colors.list_active
                } else {
                    colors.list
                },
                colors.border,
                colors.list_hover,
                colors.primary,
                colors.muted_foreground,
                colors.foreground,
            ));
        }

        let mut preset_items = Vec::new();
        for preset in self.layout_presets.clone() {
            preset_items.push(self.render_preset_item(
                preset,
                cx,
                colors.list,
                colors.border,
                colors.list_hover,
                colors.primary,
                colors.foreground,
            ));
        }

        div()
            .id("dashboard-manager-scroll")
            .flex()
            .flex_col()
            .size_full()
            .p(px(16.0))
            .overflow_y_scroll()
            .gap(px(24.0))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(colors.foreground)
                            .child("Dashboards Manager"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(colors.muted_foreground)
                            .child(format!(
                                "{} dashboard{}",
                                module_count,
                                if module_count == 1 { "" } else { "s" }
                            )),
                    )
                    .child(
                        Button::new("create-dashboard")
                            .label("New Dashboard")
                            .primary()
                            .small()
                            .w_full()
                            .on_click(cx.listener(|_this, _event, _window, cx| {
                                cx.emit(DashboardManagerEvent::OpenCreateDialog)
                            })),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(px(8.0))
                            .child(
                                Button::new("export-dashboard")
                                    .label("Export")
                                    .secondary_selected(false)
                                    .xsmall()
                                    .on_click(cx.listener(|_this, _event, _window, cx| {
                                        cx.emit(DashboardManagerEvent::ExportDashboard)
                                    })),
                            )
                            .child(
                                Button::new("import-dashboard")
                                    .label("Import")
                                    .secondary_selected(false)
                                    .xsmall()
                                    .on_click(cx.listener(|_this, _event, _window, cx| {
                                        cx.emit(DashboardManagerEvent::ImportDashboard(None))
                                    })),
                            ),
                    )
                    .when(module_count > 0, |el| {
                        el.child(div().flex().flex_col().gap(px(8.0)).children(module_items))
                    })
                    .when(module_count == 0, |el| {
                        el.child(
                            div()
                                .text_sm()
                                .text_color(colors.muted_foreground)
                                .child("No dashboards yet."),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(colors.foreground)
                            .child("Layout Presets"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(colors.muted_foreground)
                            .child(format!(
                                "{} preset{}",
                                preset_count,
                                if preset_count == 1 { "" } else { "s" }
                            )),
                    )
                    .when(preset_count > 0, |el| {
                        el.child(div().flex().flex_col().gap(px(8.0)).children(preset_items))
                    })
                    .when(preset_count == 0, |el| {
                        el.child(
                            div()
                                .text_sm()
                                .text_color(colors.muted_foreground)
                                .child("No saved presets yet."),
                        )
                    }),
            )
    }
}
