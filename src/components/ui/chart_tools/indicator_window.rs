//! Floating indicator manager for a single chart pane.
//!
//! The window never owns indicator state: it renders from the chart entity
//! looked up through `ChartPaneBinding` on every frame and writes mutations
//! back through the chart's public indicator API.

use std::collections::BTreeMap;

use gpui::{
    AnyElement, App, AppContext, ClickEvent, Context, ElementId, Entity, FocusHandle, Focusable,
    InteractiveElement, IntoElement, KeyDownEvent, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, WeakEntity, Window, div, prelude::FluentBuilder, px,
};
use gpui_component::PixelsExt;
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::menu::{DropdownMenu, PopupMenuItem};
use gpui_component::scroll::ScrollableElement;
use gpui_component::tab::{Tab, TabBar};
use gpui_component::theme::ActiveTheme;
use gpui_component::{Icon, Sizable, h_flex, v_flex};

use super::ChartPaneBinding;
use crate::chart::model::{IndicatorCategory, IndicatorInstance, IndicatorKind};
use crate::components::ui::chart_panel::ChartPanel;
use crate::components::ui::pane_grid::renderer::PaneGridRenderer;

const CATEGORY_FILTERS: [Option<IndicatorCategory>; 5] = [
    None,
    Some(IndicatorCategory::Trend),
    Some(IndicatorCategory::Momentum),
    Some(IndicatorCategory::Volatility),
    Some(IndicatorCategory::Volume),
];

struct ParamInput {
    key: String,
    label: String,
    input: Entity<InputState>,
}

struct EditingIndicator {
    id: String,
    inputs: Vec<ParamInput>,
    error: Option<String>,
}

/// The real indicator window body.
pub struct IndicatorWindow {
    grid: WeakEntity<PaneGridRenderer>,
    pane_id: String,
    focus_handle: FocusHandle,
    search: Entity<InputState>,
    category: Option<IndicatorCategory>,
    editing: Option<EditingIndicator>,
    confirm_remove_id: Option<String>,
    selected_active: Option<usize>,
}

impl IndicatorWindow {
    pub fn new(binding: ChartPaneBinding, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let ChartPaneBinding { grid, pane_id } = binding;
        let has_chart = grid.read(cx).chart_for_pane(&pane_id).is_some();
        let weak_grid = grid.downgrade();
        cx.observe(&grid, |_, _, cx| cx.notify()).detach();

        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search indicators"));
        cx.subscribe(&search, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.selected_active = None;
                cx.notify();
            }
        })
        .detach();

        let focus_handle = cx.focus_handle();
        if has_chart {
            let search_for_focus = search.clone();
            window.defer(cx, move |window, cx| {
                search_for_focus.update(cx, |state, cx| state.focus(window, cx));
            });
        } else {
            let focus_handle = focus_handle.clone();
            window.defer(cx, move |window, _| {
                window.focus(&focus_handle);
            });
        }

        Self {
            grid: weak_grid,
            pane_id,
            focus_handle,
            search,
            category: None,
            editing: None,
            confirm_remove_id: None,
            selected_active: None,
        }
    }

    fn find_chart(&self, cx: &App) -> Option<Entity<ChartPanel>> {
        let grid = self.grid.upgrade()?;
        let grid = grid.read(cx);
        let module = grid.pane_module_type(&self.pane_id)?;
        if !module.eq_ignore_ascii_case("chart") {
            return None;
        }
        grid.chart_for_pane(&self.pane_id)
    }

    fn notify_grid(&self, cx: &mut App) {
        if let Some(grid) = self.grid.upgrade() {
            grid.update(cx, |_, cx| cx.notify());
        }
    }

    fn search_query(&self, cx: &App) -> String {
        self.search.read(cx).value().trim().to_string()
    }

    fn focus_search(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.search.update(cx, |state, cx| state.focus(window, cx));
    }

    fn clear_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search
            .update(cx, |state, cx| state.set_value("", window, cx));
        self.selected_active = None;
        self.focus_search(window, cx);
        cx.notify();
    }

    fn instances(&self, cx: &App) -> Vec<IndicatorInstance> {
        self.find_chart(cx)
            .map(|chart| chart.read(cx).indicator_instances().to_vec())
            .unwrap_or_default()
    }

    fn filtered_active(&self, instances: &[IndicatorInstance], cx: &App) -> Vec<usize> {
        let query = self.search_query(cx).to_lowercase();
        instances
            .iter()
            .enumerate()
            .filter(|(_, instance)| instance_matches(instance, &query))
            .map(|(index, _)| index)
            .collect()
    }

    fn move_selection(&mut self, up: bool, cx: &mut Context<Self>) {
        let instances = self.instances(cx);
        let visible = self.filtered_active(&instances, cx);
        if visible.is_empty() {
            self.selected_active = None;
            cx.notify();
            return;
        }
        let position = self
            .selected_active
            .and_then(|selected| visible.iter().position(|index| *index == selected));
        let next = match (up, position) {
            (true, Some(0)) | (true, None) => 0,
            (true, Some(position)) => position - 1,
            (false, Some(position)) => (position + 1).min(visible.len() - 1),
            (false, None) => 0,
        };
        self.selected_active = Some(visible[next]);
        cx.notify();
    }

    fn toggle_editor_by_index(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let instances = self.instances(cx);
        let Some(instance) = instances.get(index).cloned() else {
            return;
        };
        self.selected_active = Some(index);
        window.focus(&self.focus_handle);
        self.toggle_editor(&instance, window, cx);
    }

    fn toggle_editor(
        &mut self,
        instance: &IndicatorInstance,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .editing
            .as_ref()
            .is_some_and(|editing| editing.id == instance.id)
        {
            self.cancel_editor(cx);
            return;
        }
        self.editing = Some(self.build_editor(instance, window, cx));
        self.confirm_remove_id = None;
        cx.notify();
    }

    fn build_editor(
        &self,
        instance: &IndicatorInstance,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> EditingIndicator {
        let mut inputs = Vec::with_capacity(instance.params.len());
        for (key, value) in &instance.params {
            let input =
                cx.new(|cx| InputState::new(window, cx).default_value(format_param(*value)));
            cx.subscribe(&input, |this, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change)
                    && let Some(editing) = &mut this.editing
                    && editing.error.take().is_some()
                {
                    cx.notify();
                }
            })
            .detach();
            inputs.push(ParamInput {
                key: key.clone(),
                label: param_label(key),
                input,
            });
        }
        EditingIndicator {
            id: instance.id.clone(),
            inputs,
            error: None,
        }
    }

    fn cancel_editor(&mut self, cx: &mut Context<Self>) {
        self.editing = None;
        self.confirm_remove_id = None;
        cx.notify();
    }

    fn apply_editor(&mut self, cx: &mut Context<Self>) {
        let Some(chart) = self.find_chart(cx) else {
            return;
        };
        let Some(editing) = self.editing.as_mut() else {
            return;
        };

        let mut params = BTreeMap::new();
        for param in &editing.inputs {
            let raw = param.input.read(cx).value().trim().to_string();
            match raw.parse::<f64>() {
                Ok(value) if value.is_finite() => {
                    params.insert(param.key.clone(), value);
                }
                _ => {
                    editing.error = Some(format!("{} must be a number", param.label));
                    cx.notify();
                    return;
                }
            }
        }

        let id = editing.id.clone();
        let result = chart.update(cx, |panel, cx| {
            let result = panel.update_indicator_params(&id, params);
            cx.notify();
            result
        });
        let succeeded = match result {
            Ok(()) => {
                editing.error = None;
                true
            }
            Err(error) => {
                editing.error = Some(error);
                false
            }
        };
        if succeeded {
            self.confirm_remove_id = None;
            self.notify_grid(cx);
        }
        cx.notify();
    }

    fn confirm_or_remove(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.confirm_remove_id.as_deref() != Some(id) {
            self.confirm_remove_id = Some(id.to_string());
            cx.notify();
            return;
        }
        self.remove_indicator(id, cx);
    }

    fn remove_indicator(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(chart) = self.find_chart(cx) else {
            return;
        };
        chart.update(cx, |panel, cx| {
            panel.remove_indicator_instance(id);
            cx.notify();
        });
        if self
            .editing
            .as_ref()
            .is_some_and(|editing| editing.id == id)
        {
            self.editing = None;
        }
        self.confirm_remove_id = None;
        self.notify_grid(cx);
        cx.notify();
    }

    fn toggle_visibility(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(chart) = self.find_chart(cx) else {
            return;
        };
        chart.update(cx, |panel, cx| {
            panel.toggle_indicator_instance(id);
            cx.notify();
        });
        self.notify_grid(cx);
        cx.notify();
    }

    fn toggle_visibility_by_index(&mut self, index: usize, cx: &mut Context<Self>) {
        let instances = self.instances(cx);
        if let Some(instance) = instances.get(index) {
            let id = instance.id.clone();
            self.toggle_visibility(&id, cx);
        }
    }

    fn add_indicator(&mut self, kind: IndicatorKind, cx: &mut Context<Self>) {
        let Some(chart) = self.find_chart(cx) else {
            return;
        };
        chart.update(cx, |panel, cx| {
            panel.add_indicator_instance(kind);
            cx.notify();
        });
        self.notify_grid(cx);
        cx.notify();
    }

    fn first_add_candidate(&self, cx: &App) -> Option<IndicatorKind> {
        filtered_add_kinds(&self.search_query(cx), self.category)
            .into_iter()
            .next()
    }

    fn handle_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        let search_focused = self.search.read(cx).focus_handle(cx).is_focused(window);
        let editing_input_focused = self.editing.as_ref().is_some_and(|editing| {
            editing
                .inputs
                .iter()
                .any(|param| param.input.read(cx).focus_handle(cx).is_focused(window))
        });
        let text_input_focused = search_focused || editing_input_focused;

        if key == "escape" {
            if !self.search_query(cx).is_empty() {
                self.clear_search(window, cx);
            } else if self.editing.is_some() {
                self.cancel_editor(cx);
            } else if self.confirm_remove_id.is_some() {
                self.confirm_remove_id = None;
                cx.notify();
            } else {
                window.remove_window();
            }
            cx.stop_propagation();
            return;
        }

        if !text_input_focused
            && (key == "/" || (key == "f" && event.keystroke.modifiers.secondary()))
        {
            self.focus_search(window, cx);
            cx.stop_propagation();
            return;
        }

        if self.find_chart(cx).is_none() {
            return;
        }

        match key {
            "up" | "down" if !editing_input_focused => {
                self.move_selection(key == "up", cx);
                cx.stop_propagation();
            }
            "enter" => {
                if self.editing.is_some() {
                    self.apply_editor(cx);
                } else if let Some(index) = self.selected_active {
                    self.toggle_editor_by_index(index, window, cx);
                } else if let Some(kind) = self.first_add_candidate(cx) {
                    self.add_indicator(kind, cx);
                }
                cx.stop_propagation();
            }
            "space" if !text_input_focused => {
                if let Some(index) = self.selected_active {
                    self.toggle_visibility_by_index(index, cx);
                }
                cx.stop_propagation();
            }
            _ => {}
        }
    }

    fn render_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors;
        let drag_region = h_flex()
            .flex_1()
            .min_w_0()
            .gap(px(8.0))
            .items_center()
            .child(Icon::default().path("icons/chart-pie.svg").size(px(15.0)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_sm()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child("Indicators"),
            );

        #[cfg(target_os = "linux")]
        let drag_region = drag_region.on_mouse_down(gpui::MouseButton::Left, |_, window, _| {
            window.start_window_move();
        });
        #[cfg(not(target_os = "linux"))]
        let drag_region = drag_region.window_control_area(gpui::WindowControlArea::Drag);

        h_flex()
            .id(ElementId::Name(SharedString::from(format!(
                "indicator-window-header-{}",
                self.pane_id
            ))))
            .flex_shrink_0()
            .h(px(36.0))
            .px(px(10.0))
            .gap(px(8.0))
            .items_center()
            .border_b_1()
            .border_color(colors.title_bar_border)
            .bg(colors.title_bar)
            .child(drag_region)
            .child(
                Button::new(ElementId::Name(SharedString::from(format!(
                    "indicator-window-close-{}",
                    self.pane_id
                ))))
                .icon(Icon::default().path("icons/close.svg"))
                .ghost()
                .xsmall()
                .tooltip("Close")
                .on_click(|_, window, cx| {
                    cx.stop_propagation();
                    window.remove_window();
                }),
            )
            .into_any_element()
    }

    fn render_context(
        &self,
        chart: Option<&Entity<ChartPanel>>,
        instances: &[IndicatorInstance],
        cx: &App,
    ) -> AnyElement {
        let colors = cx.theme().colors;
        let (context, shown, hidden) = if let Some(chart) = chart {
            let chart = chart.read(cx);
            let (exchange, symbol, timeframe) = chart.market_context();
            (
                format!("{} · {} · [{}]", exchange.to_uppercase(), symbol, timeframe),
                instances.iter().filter(|instance| instance.visible).count(),
                instances
                    .iter()
                    .filter(|instance| !instance.visible)
                    .count(),
            )
        } else {
            ("No chart bound".to_string(), 0, 0)
        };

        h_flex()
            .flex_shrink_0()
            .h(px(26.0))
            .px(px(10.0))
            .gap(px(8.0))
            .items_center()
            .border_b_1()
            .border_color(colors.border)
            .text_size(px(10.0))
            .text_color(colors.muted_foreground)
            .child(div().flex_1().min_w_0().truncate().child(context))
            .when(!instances.is_empty(), |this| {
                this.child(div().flex_shrink_0().child(format!("{shown} shown")))
                    .when(hidden > 0, |this| {
                        // Hidden indicators are a state, not an alert: lift them
                        // with the neutral foreground instead of a hue.
                        this.child(
                            div()
                                .flex_shrink_0()
                                .text_color(colors.foreground)
                                .child(format!("{hidden} hidden")),
                        )
                    })
            })
            .into_any_element()
    }

    fn render_search(&self) -> AnyElement {
        div()
            .px(px(10.0))
            .pt(px(8.0))
            .pb(px(6.0))
            .child(
                Input::new(&self.search)
                    .prefix(Icon::default().path("icons/search.svg").size(px(12.0)))
                    .small()
                    .cleanable(true)
                    .w_full(),
            )
            .into_any_element()
    }

    fn render_category_tabs(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors;
        let compact = window.bounds().size.width.as_f32() < 380.0;
        let selected_index = CATEGORY_FILTERS
            .iter()
            .position(|category| *category == self.category)
            .unwrap_or(0);

        let current_label = CATEGORY_FILTERS[selected_index]
            .map(|category| category.label())
            .unwrap_or("All");
        let current_category = self.category;

        let body = if compact {
            let view = cx.entity().downgrade();
            Button::new(ElementId::Name(SharedString::from(format!(
                "indicator-category-menu-{}",
                self.pane_id
            ))))
            .label(current_label)
            .xsmall()
            .ghost()
            .dropdown_caret(true)
            .dropdown_menu_with_anchor(gpui::Corner::BottomLeft, move |mut menu, _window, _cx| {
                for category in CATEGORY_FILTERS {
                    let view = view.clone();
                    menu = menu.item(
                        PopupMenuItem::new(
                            category.map(|category| category.label()).unwrap_or("All"),
                        )
                        .checked(category == current_category)
                        .on_click(move |_, _, cx| {
                            let _ = view.update(cx, |this, cx| {
                                this.category = category;
                                this.selected_active = None;
                                cx.notify();
                            });
                        }),
                    );
                }
                menu
            })
            .into_any_element()
        } else {
            let view = cx.entity().downgrade();
            let tabs: Vec<Tab> = CATEGORY_FILTERS
                .iter()
                .map(|category| {
                    Tab::new().label(category.map(|category| category.label()).unwrap_or("All"))
                })
                .collect();
            TabBar::new(ElementId::Name(SharedString::from(format!(
                "indicator-category-tabs-{}",
                self.pane_id
            ))))
            .segmented()
            .xsmall()
            .selected_index(selected_index)
            .children(tabs)
            .on_click(move |index, _, cx| {
                let category = CATEGORY_FILTERS[*index];
                let _ = view.update(cx, |this, cx| {
                    this.category = category;
                    this.selected_active = None;
                    cx.notify();
                });
            })
            .into_any_element()
        };

        div()
            .px(px(10.0))
            .pb(px(6.0))
            .border_b_1()
            .border_color(colors.border)
            .child(body)
            .into_any_element()
    }

    /// One empty state for "search matched nothing anywhere", instead of the
    /// two per-section no-match hints repeating each other.
    fn render_no_results(&self, query: &str, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors;
        v_flex()
            .items_center()
            .gap(px(4.0))
            .px(px(16.0))
            .pt(px(32.0))
            .pb(px(16.0))
            .child(
                Icon::default()
                    .path("icons/search.svg")
                    .size(px(20.0))
                    .text_color(colors.muted_foreground),
            )
            .child(
                div()
                    .pt(px(4.0))
                    .text_sm()
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .child("No indicators match"),
            )
            .child(
                div()
                    .max_w_full()
                    .truncate()
                    .text_xs()
                    .text_color(colors.muted_foreground)
                    .child(format!("\u{201c}{query}\u{201d}")),
            )
            .child(
                h_flex()
                    .pt(px(8.0))
                    .gap(px(6.0))
                    .child(
                        Button::new(ElementId::Name(SharedString::from(format!(
                            "indicator-clear-search-{}",
                            self.pane_id
                        ))))
                        .label("Clear search")
                        .xsmall()
                        .ghost()
                        .on_click(cx.listener(
                            |this, _: &ClickEvent, window, cx| {
                                this.clear_search(window, cx);
                            },
                        )),
                    )
                    // A category tab can hide matches on its own, so give the
                    // same empty state an escape from the filter too.
                    .when_some(self.category, |this, _| {
                        this.child(
                            Button::new(ElementId::Name(SharedString::from(format!(
                                "indicator-clear-category-{}",
                                self.pane_id
                            ))))
                            .label("All categories")
                            .xsmall()
                            .ghost()
                            .on_click(cx.listener(
                                |this, _: &ClickEvent, _, cx| {
                                    this.category = None;
                                    this.selected_active = None;
                                    cx.notify();
                                },
                            )),
                        )
                    }),
            )
            .into_any_element()
    }

    fn render_closed_state(&self, cx: &App) -> AnyElement {
        let colors = cx.theme().colors;
        v_flex()
            .flex_1()
            .min_h_0()
            .items_center()
            .justify_center()
            .gap(px(6.0))
            .p(px(24.0))
            .child(
                Icon::default()
                    .path("icons/chart-candlestick.svg")
                    .size(px(24.0))
                    .text_color(colors.muted_foreground),
            )
            .child(
                div()
                    .text_sm()
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .child("Chart pane closed"),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(colors.muted_foreground)
                    .child("Open a chart pane to manage its indicators."),
            )
            .into_any_element()
    }

    fn render_empty_hint(
        &self,
        text: impl IntoElement,
        colors: &gpui_component::theme::ThemeColor,
    ) -> AnyElement {
        div()
            .px(px(8.0))
            .py(px(8.0))
            .text_xs()
            .text_color(colors.muted_foreground)
            .child(text)
            .into_any_element()
    }

    fn render_active_section(
        &self,
        instances: &[IndicatorInstance],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = cx.theme().colors;
        let mut section = v_flex().child(section_label(
            format!("ACTIVE ON CHART ({})", instances.len()),
            colors.muted_foreground,
        ));

        if instances.is_empty() {
            section = section.child(
                self.render_empty_hint("No indicators on this chart yet. Add one below.", &colors),
            );
            return section.into_any_element();
        }

        let visible = self.filtered_active(instances, cx);
        if visible.is_empty() {
            section = section
                .child(self.render_empty_hint("No active indicators match your search.", &colors));
            return section.into_any_element();
        }

        for index in visible {
            let instance = &instances[index];
            let is_editing = self
                .editing
                .as_ref()
                .is_some_and(|editing| editing.id == instance.id);
            let is_selected = self.selected_active == Some(index);
            section = section.child(self.render_active_row(
                instance,
                index,
                is_selected,
                is_editing,
                &colors,
                cx,
            ));
            if is_editing && let Some(editing) = &self.editing {
                section = section.child(self.render_editor(editing, &colors, cx));
            }
        }
        section.into_any_element()
    }

    fn render_active_row(
        &self,
        instance: &IndicatorInstance,
        index: usize,
        selected: bool,
        expanded: bool,
        colors: &gpui_component::theme::ThemeColor,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = instance.id.clone();
        let visible = instance.visible;
        let label = instance.display_label();
        let category = instance.kind.category();
        let row_id = format!("indicator-active-{}-{}", self.pane_id, instance.id);

        let eye = Button::new(ElementId::Name(SharedString::from(format!(
            "indicator-eye-{}-{}",
            self.pane_id, instance.id
        ))))
        .icon(Icon::default().path(if visible {
            "icons/eye.svg"
        } else {
            "icons/eye-off.svg"
        }))
        .ghost()
        .xsmall()
        .tooltip(if visible {
            "Hide indicator"
        } else {
            "Show indicator"
        })
        .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
            cx.stop_propagation();
            this.toggle_visibility(&id, cx);
        }));

        let row = h_flex()
            .id(ElementId::Name(SharedString::from(row_id)))
            .h(px(30.0))
            .px(px(4.0))
            .gap(px(6.0))
            .items_center()
            .rounded(px(3.0))
            .cursor_pointer()
            .when(selected, |this| this.bg(colors.primary.opacity(0.15)))
            // Keep the keyboard selection visible while hovering.
            .when(!selected, |this| {
                this.hover(|style| style.bg(colors.muted.opacity(0.45)))
            })
            .child(eye)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(12.0))
                    .when(!visible, |this| this.text_color(colors.muted_foreground))
                    .child(label),
            )
            .when(!visible, |this| {
                this.child(
                    div()
                        .flex_shrink_0()
                        .text_size(px(9.0))
                        .text_color(colors.muted_foreground)
                        .child("hidden"),
                )
            })
            .child(category_chip(category, colors))
            .child(
                Icon::default()
                    .path(if expanded {
                        "icons/chevron-up.svg"
                    } else {
                        "icons/chevron-down.svg"
                    })
                    .size(px(12.0))
                    .text_color(colors.muted_foreground),
            )
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                this.toggle_editor_by_index(index, window, cx);
            }));

        row.into_any_element()
    }

    fn render_editor(
        &self,
        editing: &EditingIndicator,
        colors: &gpui_component::theme::ThemeColor,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = editing.id.clone();
        let remove_id = id.clone();
        let confirming = self.confirm_remove_id.as_deref() == Some(id.as_str());

        let remove = Button::new(ElementId::Name(SharedString::from(format!(
            "indicator-remove-{}",
            id
        ))))
        .label(if confirming { "Remove?" } else { "Remove" })
        .xsmall()
        .when(confirming, |button| button.danger())
        .when(!confirming, |button| button.ghost())
        .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
            this.confirm_or_remove(&remove_id, cx);
        }));

        let inputs = editing.inputs.iter().map(|param| {
            h_flex()
                .items_center()
                .justify_between()
                .gap(px(8.0))
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(colors.muted_foreground)
                        .child(param.label.clone()),
                )
                .child(Input::new(&param.input).xsmall().w(px(88.0)))
        });

        // 22 px + 8 px padding lines the editor text up with the row label,
        // which starts after the 4 px row inset, the 20 px eye and its gap.
        v_flex()
            .ml(px(22.0))
            .mb(px(8.0))
            .p(px(8.0))
            .gap(px(6.0))
            .rounded(px(4.0))
            .bg(colors.muted.opacity(0.18))
            .border_1()
            .border_color(colors.border)
            .children(inputs)
            .when_some(editing.error.clone(), |this, error| {
                this.child(
                    div()
                        .text_size(px(10.0))
                        .text_color(colors.danger)
                        .child(error),
                )
            })
            .child(
                h_flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(remove)
                    .child(div().flex_1())
                    .child(
                        Button::new(ElementId::Name(SharedString::from(format!(
                            "indicator-cancel-{}",
                            id
                        ))))
                        .label("Cancel")
                        .xsmall()
                        .ghost()
                        .on_click(cx.listener(
                            |this, _: &ClickEvent, _window, cx| {
                                this.cancel_editor(cx);
                            },
                        )),
                    )
                    .child(
                        Button::new(ElementId::Name(SharedString::from(format!(
                            "indicator-apply-{}",
                            id
                        ))))
                        .label("Apply")
                        .xsmall()
                        .primary()
                        .on_click(cx.listener(
                            |this, _: &ClickEvent, _window, cx| {
                                this.apply_editor(cx);
                            },
                        )),
                    ),
            )
            .into_any_element()
    }

    fn render_add_section(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors;
        let query = self.search_query(cx);
        let kinds = filtered_add_kinds(&query, self.category);
        let mut section = v_flex().child(section_label(
            format!("ADD INDICATOR ({})", kinds.len()),
            colors.muted_foreground,
        ));

        if kinds.is_empty() {
            section =
                section.child(self.render_empty_hint("No indicators match your search.", &colors));
            return section.into_any_element();
        }

        for category in [
            IndicatorCategory::Trend,
            IndicatorCategory::Momentum,
            IndicatorCategory::Volatility,
            IndicatorCategory::Volume,
        ] {
            let group: Vec<IndicatorKind> = kinds
                .iter()
                .filter(|kind| kind.category() == category)
                .cloned()
                .collect();
            if group.is_empty() {
                continue;
            }
            section = section.child(
                div()
                    .px(px(8.0))
                    .pt(px(8.0))
                    .pb(px(2.0))
                    .text_size(px(9.0))
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(colors.muted_foreground)
                    .child(category.label().to_uppercase()),
            );
            for kind in group {
                section = section.child(self.render_add_row(kind, &colors, window, cx));
            }
        }

        section.into_any_element()
    }

    fn render_add_row(
        &self,
        kind: IndicatorKind,
        colors: &gpui_component::theme::ThemeColor,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let element_id = format!("indicator-add-{}-{}", self.pane_id, kind.label());
        let add_kind = kind.clone();
        let category = kind.category();
        let label = kind.label().to_string();

        h_flex()
            .id(ElementId::Name(SharedString::from(element_id)))
            .h(px(28.0))
            .px(px(8.0))
            .gap(px(8.0))
            .items_center()
            .rounded(px(3.0))
            .cursor_pointer()
            .hover(|style| style.bg(colors.muted.opacity(0.45)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(12.0))
                    .child(label),
            )
            .child(category_chip(category, colors))
            .child(
                Icon::default()
                    .path("icons/plus.svg")
                    .size(px(12.0))
                    .text_color(colors.muted_foreground),
            )
            .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                this.add_indicator(add_kind.clone(), cx);
            }))
            .into_any_element()
    }
}

impl Focusable for IndicatorWindow {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for IndicatorWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let chart = self.find_chart(cx);
        let instances = chart
            .as_ref()
            .map(|chart| chart.read(cx).indicator_instances().to_vec())
            .unwrap_or_default();

        if let Some(editing) = &self.editing
            && !instances.iter().any(|instance| instance.id == editing.id)
        {
            self.editing = None;
            self.confirm_remove_id = None;
        }

        let body = if chart.is_some() {
            let query = self.search_query(cx);
            let any_active_match = !self.filtered_active(&instances, cx).is_empty();
            let any_add_match = !filtered_add_kinds(&query, self.category).is_empty();
            let no_results = !query.is_empty() && !any_active_match && !any_add_match;

            let body: AnyElement = if no_results {
                self.render_no_results(&query, cx)
            } else {
                v_flex()
                    .child(self.render_active_section(&instances, cx))
                    .child(self.render_add_section(window, cx))
                    .into_any_element()
            };

            v_flex()
                .flex_1()
                .min_h_0()
                .child(self.render_search())
                .child(self.render_category_tabs(window, cx))
                .child(
                    v_flex()
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scrollbar()
                        .px(px(10.0))
                        .pb(px(10.0))
                        .child(body),
                )
                .into_any_element()
        } else {
            self.render_closed_state(cx)
        };

        v_flex()
            .size_full()
            .bg(colors.background)
            .text_color(colors.foreground)
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                this.handle_key(event, window, cx);
            }))
            .child(self.render_header(cx))
            .child(self.render_context(chart.as_ref(), &instances, cx))
            .child(body)
    }
}

fn section_label(text: String, color: gpui::Hsla) -> AnyElement {
    div()
        .px(px(8.0))
        .pt(px(8.0))
        .pb(px(4.0))
        .text_size(px(9.0))
        .font_weight(gpui::FontWeight::BOLD)
        .text_color(color)
        .child(text)
        .into_any_element()
}

/// Category is metadata, not a market signal: it uses the same quiet chip as
/// exchange tags elsewhere (`muted` surface, `muted_foreground` text). Colour
/// stays reserved for direction and alerts.
fn category_chip(
    category: IndicatorCategory,
    colors: &gpui_component::theme::ThemeColor,
) -> AnyElement {
    div()
        .flex_shrink_0()
        .px(px(5.0))
        .h(px(15.0))
        .flex()
        .items_center()
        .rounded(px(3.0))
        .bg(colors.muted)
        .text_size(px(9.0))
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(colors.muted_foreground)
        .child(category.label())
        .into_any_element()
}

fn param_label(key: &str) -> String {
    match key {
        "period" => "Period".to_string(),
        "deviations" => "Std dev".to_string(),
        "short" => "Fast".to_string(),
        "long" => "Slow".to_string(),
        "signal" => "Signal".to_string(),
        "k_period" => "K period".to_string(),
        "d_period" => "D period".to_string(),
        "step" => "Step".to_string(),
        "max_step" => "Max step".to_string(),
        other => other.to_string(),
    }
}

fn format_param(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{}", value as i64)
    } else {
        value.to_string()
    }
}

fn instance_matches(instance: &IndicatorInstance, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    instance.display_label().to_lowercase().contains(query)
        || instance
            .kind
            .category()
            .label()
            .to_lowercase()
            .contains(query)
}

fn filtered_add_kinds(query: &str, category: Option<IndicatorCategory>) -> Vec<IndicatorKind> {
    let query = query.trim().to_lowercase();
    IndicatorKind::all()
        .into_iter()
        .filter(|kind| category.is_none_or(|category| kind.category() == category))
        .filter(|kind| {
            query.is_empty()
                || kind.label().to_lowercase().contains(&query)
                || kind.category().label().to_lowercase().contains(&query)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_matches_indicator_and_category_labels() {
        assert!(
            filtered_add_kinds("ema", None)
                .iter()
                .any(|kind| *kind == IndicatorKind::Ema)
        );
        assert_eq!(
            filtered_add_kinds("momentum", None),
            IndicatorKind::all_by_category(IndicatorCategory::Momentum)
        );
        assert!(filtered_add_kinds("no-such-indicator", None).is_empty());
    }

    #[test]
    fn category_filter_limits_results() {
        let volume = filtered_add_kinds("", Some(IndicatorCategory::Volume));
        assert!(!volume.is_empty());
        assert!(
            volume
                .iter()
                .all(|kind| kind.category() == IndicatorCategory::Volume)
        );
    }

    #[test]
    fn parameter_labels_and_formatting_are_stable() {
        assert_eq!(param_label("period"), "Period");
        assert_eq!(param_label("unknown"), "unknown");
        assert_eq!(format_param(20.0), "20");
        assert_eq!(format_param(0.02), "0.02");
    }
}
