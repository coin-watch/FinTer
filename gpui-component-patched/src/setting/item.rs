use gpui::{
    AnyElement, App, Axis, Div, InteractiveElement as _, IntoElement, ParentElement, SharedString,
    Stateful, Styled, Window, div, prelude::FluentBuilder as _, px,
};
use std::{any::TypeId, ops::Deref, rc::Rc};

use crate::{
    ActiveTheme as _, AxisExt, StyledExt as _,
    label::Label,
    setting::{
        AnySettingField, ElementField, RenderOptions,
        fields::{
            BoolField, ComboboxField, DropdownField, NumberField, SettingFieldRender, StringField,
        },
    },
    text::Text,
    v_flex,
};

/// Setting item.
#[derive(Clone)]
pub enum SettingItem {
    /// A normal setting item with a title, description, and field.
    Item {
        title: SharedString,
        description: Option<Text>,
        layout: Axis,
        field: Rc<dyn AnySettingField>,
    },
    /// A full custom element to render.
    Element {
        render: Rc<dyn Fn(&RenderOptions, &mut Window, &mut App) -> AnyElement + 'static>,
    },
}

impl SettingItem {
    /// Create a new setting item.
    pub fn new<F>(title: impl Into<SharedString>, field: F) -> Self
    where
        F: AnySettingField + 'static,
    {
        SettingItem::Item {
            title: title.into(),
            description: None,
            layout: Axis::Horizontal,
            field: Rc::new(field),
        }
    }

    /// Create a new custom element setting item with a render closure.
    pub fn render<R, E>(render: R) -> Self
    where
        E: IntoElement,
        R: Fn(&RenderOptions, &mut Window, &mut App) -> E + 'static,
    {
        SettingItem::Element {
            render: Rc::new(move |options, window, cx| {
                render(options, window, cx).into_any_element()
            }),
        }
    }

    /// Set the description of the setting item.
    ///
    /// Only applies to [`SettingItem::Item`].
    pub fn description(mut self, description: impl Into<Text>) -> Self {
        match &mut self {
            SettingItem::Item { description: d, .. } => {
                *d = Some(description.into());
            }
            SettingItem::Element { .. } => {}
        }
        self
    }

    /// Set the layout of the setting item.
    ///
    /// Only applies to [`SettingItem::Item`].
    pub fn layout(mut self, layout: Axis) -> Self {
        match &mut self {
            SettingItem::Item { layout: l, .. } => {
                *l = layout;
            }
            SettingItem::Element { .. } => {}
        }
        self
    }

    pub(crate) fn is_match(&self, query: &str) -> bool {
        match self {
            SettingItem::Item {
                title, description, ..
            } => {
                title.to_lowercase().contains(&query.to_lowercase())
                    || description.as_ref().map_or(false, |d| {
                        d.as_str().to_lowercase().contains(&query.to_lowercase())
                    })
            }
            // We need to show all custom elements when not searching.
            SettingItem::Element { .. } => query.is_empty(),
        }
    }

    pub(crate) fn is_resettable(&self, cx: &App) -> bool {
        match self {
            SettingItem::Item { field, .. } => field.is_resettable(cx),
            SettingItem::Element { .. } => false,
        }
    }

    pub(crate) fn reset(&self, window: &mut Window, cx: &mut App) {
        match self {
            SettingItem::Item { field, .. } => field.reset(window, cx),
            SettingItem::Element { .. } => {}
        }
    }

    fn render_field(
        field: Rc<dyn AnySettingField>,
        options: RenderOptions,
        window: &mut Window,
        cx: &mut App,
    ) -> impl IntoElement {
        let field_type = field.field_type();
        let style = field.style().clone();
        let type_id = field.deref().type_id();
        let renderer: Box<dyn SettingFieldRender> = match type_id {
            t if t == std::any::TypeId::of::<bool>() => {
                Box::new(BoolField::new(field_type.is_switch()))
            }
            t if t == TypeId::of::<f64>() && field_type.is_number_input() => {
                Box::new(NumberField::new(field_type.number_input_options()))
            }
            t if t == TypeId::of::<SharedString>() && field_type.is_input() => {
                Box::new(StringField::<SharedString>::new())
            }
            t if t == TypeId::of::<String>() && field_type.is_input() => {
                Box::new(StringField::<String>::new())
            }
            t if t == TypeId::of::<SharedString>() && field_type.is_dropdown() => Box::new(
                DropdownField::<SharedString>::new(field_type.dropdown_options()),
            ),
            t if t == TypeId::of::<String>() && field_type.is_dropdown() => {
                Box::new(DropdownField::<String>::new(field_type.dropdown_options()))
            }
            t if t == TypeId::of::<SharedString>() && field_type.is_combobox() => Box::new(
                ComboboxField::<SharedString>::new(field_type.combobox_options()),
            ),
            t if t == TypeId::of::<String>() && field_type.is_combobox() => {
                Box::new(ComboboxField::<String>::new(field_type.combobox_options()))
            }
            _ if field_type.is_element() => Box::new(ElementField::new(field_type.element())),
            _ => unimplemented!("Unsupported setting type: {}", field.deref().type_name()),
        };

        renderer.render(field, &options, &style, window, cx)
    }

    pub(super) fn render_item(
        self,
        options: &RenderOptions,
        window: &mut Window,
        cx: &mut App,
    ) -> Stateful<Div> {
        div()
            .id(SharedString::from(format!("item-{}", options.item_ix)))
            .w_full()
            .child(match self {
                SettingItem::Item {
                    title,
                    description,
                    layout,
                    field,
                } => {
                    let field_type = field.field_type();
                    let wide_field = field_type.is_input()
                        || field_type.is_number_input()
                        || field_type.is_dropdown()
                        || field_type.is_combobox();
                    let is_element = field_type.is_element();

                    div()
                        .w_full()
                        .min_w_0()
                        .overflow_hidden()
                        .map(|this| {
                            if layout.is_horizontal() {
                                // Wrap the field onto its own line when the
                                // content area is narrower than label + field.
                                this.h_flex().justify_between().items_start().flex_wrap()
                            } else {
                                this.v_flex()
                            }
                        })
                        .gap_3()
                        .child(
                            v_flex()
                                .min_w_0()
                                .map(|this| {
                                    if layout.is_horizontal() {
                                        // Keep enough room for the title so it
                                        // wraps normally instead of collapsing
                                        // into a one-character-wide column, but
                                        // let it shrink on compact windows.
                                        // Do not cap the width: when the field
                                        // wraps onto its own line the title can
                                        // then use the full row instead of being
                                        // squeezed into a narrow left column.
                                        this.flex_1().min_w(px(120.))
                                    } else {
                                        this.w_full()
                                    }
                                })
                                .gap_1()
                                .child(Label::new(title.clone()).text_sm())
                                .when_some(description.clone(), |this, description| {
                                    this.child(
                                        div()
                                            .w_full()
                                            .min_w_0()
                                            .text_sm()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(description),
                                    )
                                }),
                        )
                        .child(
                            div()
                                .id("field")
                                .min_w_0()
                                .when(layout.is_vertical(), |this| this.w_full())
                                .when(layout.is_horizontal() && wide_field, |this| {
                                    // Keep a comfortable preferred width, but let
                                    // the field shrink with the row (flex-wrap
                                    // moves it onto its own line when needed) so
                                    // narrow windows never push horizontal scroll.
                                    this.w(px(256.)).min_w(px(140.)).max_w_full().flex_1()
                                })
                                .when(layout.is_horizontal() && is_element, |this| this.flex_1())
                                .child(Self::render_field(
                                    field,
                                    RenderOptions { layout, ..*options },
                                    window,
                                    cx,
                                )),
                        )
                        .into_any_element()
                }
                SettingItem::Element { render } => {
                    (render)(&options, window, cx).into_any_element()
                }
            })
    }
}
