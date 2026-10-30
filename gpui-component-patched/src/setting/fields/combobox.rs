use std::rc::Rc;

use gpui::{
    div, prelude::FluentBuilder as _, px, AnyElement, App, Corner, ElementId,
    Focusable as _, InteractiveElement as _, IntoElement, MouseButton, ParentElement, SharedString,
    StyleRefinement, Styled, Window,
};

use crate::{
    button::Button,
    h_flex,
    input::Input,
    popover::{Popover, PopoverState},
    scroll::ScrollableElement as _,
    setting::{
        fields::{get_value, set_value, SettingFieldRender},
        AnySettingField, RenderOptions,
    },
    v_flex, ActiveTheme, AxisExt, Icon, IconName, Sizable, StyledExt,
};

pub(crate) struct ComboboxField<T> {
    options: Vec<(SharedString, SharedString)>,
    _marker: std::marker::PhantomData<T>,
}

impl<T> ComboboxField<T> {
    pub(crate) fn new(options: Option<&Vec<(SharedString, SharedString)>>) -> Self {
        Self {
            options: options.cloned().unwrap_or_default(),
            _marker: std::marker::PhantomData,
        }
    }
}

impl<T> SettingFieldRender for ComboboxField<T>
where
    T: Into<SharedString> + From<SharedString> + Clone + 'static,
{
    fn render(
        &self,
        field: Rc<dyn AnySettingField>,
        options: &RenderOptions,
        style: &StyleRefinement,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let old_value = get_value::<T>(&field, cx);
        let set_value = set_value::<T>(&field, cx);
        let dropdown_options = self.options.clone();

        let old_label = dropdown_options
            .iter()
            .find(|(value, _)| *value == old_value.clone().into())
            .map(|(_, label)| label.clone())
            .unwrap_or_else(|| old_value.clone().into());

        let search_input_state = window.use_keyed_state(
            ElementId::Name(format!("combobox-search-{}", field.type_name()).into()),
            cx,
            |window, cx| crate::input::InputState::new(window, cx).placeholder("Search..."),
        );
        let popover_id = ElementId::Name(format!("combobox-field-{}", field.type_name()).into());
        let search_focus = search_input_state.focus_handle(cx);

        // Do not focus the combobox search field while rendering the closed
        // control. Settings rerender whenever an input changes; forcing focus
        // here steals keyboard focus from every other field on the page.
        Popover::new(popover_id)
            .anchor(Corner::TopRight)
            .appearance(false)
            .track_focus(&search_focus)
            .trigger(
                Button::new("btn")
                    .when(options.layout.is_vertical(), |this| this.w_full())
                    .label(old_label)
                    .dropdown_caret(true)
                    .outline()
                    .with_size(options.size)
                    .refine_style(style),
            )
            .content({
                let dropdown_options = dropdown_options.clone();
                let set_value = set_value.clone();
                let old_value_str: SharedString = old_value.into();
                let search_input_state = search_input_state.clone();

                move |_popover_state, _win, cx| {
                    let theme = cx.theme().clone();
                    let query = search_input_state.read(cx).text().to_string().to_lowercase();

                    let filtered_options: Vec<_> = dropdown_options
                        .iter()
                        .filter(|(val, lbl)| {
                            if query.is_empty() {
                                true
                            } else {
                                val.to_lowercase().contains(&query)
                                    || lbl.to_lowercase().contains(&query)
                            }
                        })
                        .cloned()
                        .collect();

                    let mut items_list = v_flex().gap(px(2.0)).p(px(4.0)).w_full();

                    if filtered_options.is_empty() {
                        items_list = items_list.child(
                            div()
                                .px(px(8.0))
                                .py(px(6.0))
                                .text_sm()
                                .text_color(theme.colors.muted_foreground)
                                .child("No options found"),
                        );
                    } else {
                        for (value, label) in filtered_options {
                            let checked = value == old_value_str;
                            let set_value = set_value.clone();
                            let value_clone = value.clone();

                            items_list = items_list.child(
                                h_flex()
                                    .w_full()
                                    .px(px(8.0))
                                    .py(px(6.0))
                                    .rounded_sm()
                                    .items_center()
                                    .justify_between()
                                    .cursor_pointer()
                                    .hover(|s| s.bg(theme.colors.accent))
                                    .when(checked, |s| s.bg(theme.colors.muted))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(
                                            move |popover: &mut PopoverState, _ev, window, cx| {
                                                set_value(T::from(value_clone.clone()), cx);
                                                popover.dismiss(window, cx);
                                            },
                                        ),
                                    )
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(if checked {
                                                theme.colors.foreground
                                            } else {
                                                theme.colors.muted_foreground
                                            })
                                            .child(label),
                                    )
                                    .when(checked, |s| {
                                        s.child(
                                            Icon::new(IconName::Check)
                                                .size(px(14.0))
                                                .text_color(theme.colors.primary),
                                        )
                                    }),
                            );
                        }
                    }

                    v_flex()
                        .w(px(320.0))
                        .bg(theme.colors.background)
                        .border_1()
                        .border_color(theme.colors.border)
                        .rounded_md()
                        .shadow_lg()
                        .child(
                            div()
                                .p(px(6.0))
                                .border_b_1()
                                .border_color(theme.colors.border)
                                .child(
                                    Input::new(&search_input_state)
                                        .cleanable(true)
                                        .small()
                                        .w_full(),
                                ),
                        )
                        .child(
                            div()
                                .max_h(px(280.0))
                                .overflow_y_scrollbar()
                                .w_full()
                                .child(items_list),
                        )
                        .into_any_element()
                }
            })
            .into_any_element()
    }
}
