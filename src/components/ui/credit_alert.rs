use gpui::*;
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::Sizable;
use gpui_component::{Icon, IconName};
use crate::data::credits::{CreditExhaustedInfo, CreditStore};

/// Renders a full-pane empty state when a module cannot display data because credits have run out.
pub fn render_credits_exhausted_empty(
    module_name: &str,
    info: Option<&CreditExhaustedInfo>,
    compact: bool,
    theme: &gpui_component::theme::Theme,
) -> Div {
    let billing_url = info
        .map(|i| i.billing_url.clone())
        .unwrap_or_else(|| format!("{}/account/credits", crate::data::client::service_base_url().trim_end_matches('/')));
    let url_clone = billing_url.clone();
    let detail_msg = info
        .map(|i| i.message.clone())
        .unwrap_or_else(|| "Market data requests on this module require credits. Your balance has run out.".to_string());

    if compact {
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .p(px(8.0))
            .gap(px(4.0))
            .bg(theme.colors.background)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme.colors.danger)
                    .child(Icon::new(IconName::TriangleAlert).size(px(12.0)).text_color(theme.colors.danger))
                    .child("CREDITS RUN OUT"),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.colors.muted_foreground)
                    .child(format!("{module_name} paused")),
            )
            .child(
                Button::new(ElementId::Name(format!("credit-manage-compact-{}", module_name).into()))
                    .label("Manage Credits")
                    .xsmall()
                    .primary()
                    .on_click(move |_, _, _| {
                        CreditStore::open_billing_page(Some(&url_clone));
                    }),
            )
    } else {
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .p(px(16.0))
            .gap(px(10.0))
            .bg(theme.colors.background)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .px(px(8.0))
                    .py(px(3.0))
                    .rounded(px(4.0))
                    .bg(theme.colors.danger.opacity(0.12))
                    .border_1()
                    .border_color(theme.colors.danger.opacity(0.3))
                    .child(Icon::new(IconName::TriangleAlert).size(px(13.0)).text_color(theme.colors.danger))
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme.colors.danger)
                            .child("CREDITS RUN OUT"),
                    ),
            )
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.colors.foreground)
                    .child(format!("{module_name} Market Data Paused")),
            )
            .child(
                div()
                    .max_w(px(280.0))
                    .text_xs()
                    .text_align(TextAlign::Center)
                    .text_color(theme.colors.muted_foreground)
                    .child(detail_msg),
            )
            .child(
                Button::new(ElementId::Name(format!("credit-manage-btn-{}", module_name).into()))
                    .label("Manage Credits")
                    .small()
                    .primary()
                    .on_click(move |_, _, _| {
                        CreditStore::open_billing_page(Some(&url_clone));
                    }),
            )
    }
}

/// Renders a top notification banner over a module when requests are blocked due to credits,
/// but existing cached data can still be displayed.
pub fn render_credits_banner(
    module_name: &str,
    info: Option<&CreditExhaustedInfo>,
    theme: &gpui_component::theme::Theme,
) -> Div {
    let billing_url = info
        .map(|i| i.billing_url.clone())
        .unwrap_or_else(|| format!("{}/account/credits", crate::data::client::service_base_url().trim_end_matches('/')));
    let url_clone = billing_url.clone();

    div()
        .w_full()
        .min_w_0()
        .px(px(8.0))
        .py(px(4.0))
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .gap(px(6.0))
        .bg(theme.colors.danger.opacity(0.15))
        .border_b_1()
        .border_color(theme.colors.danger.opacity(0.35))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.0))
                .min_w_0()
                .flex_1()
                .child(
                    Icon::new(IconName::TriangleAlert)
                        .size(px(12.0))
                        .text_color(theme.colors.danger)
                        .flex_shrink_0(),
                )
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.colors.foreground)
                        .truncate()
                        .child(format!("Credits run out: {module_name} requests paused")),
                ),
        )
        .child(
            Button::new(ElementId::Name(format!("credit-banner-btn-{}", module_name).into()))
                .label("Manage")
                .xsmall()
                .primary()
                .flex_shrink_0()
                .on_click(move |_, _, _| {
                    CreditStore::open_billing_page(Some(&url_clone));
                }),
        )
}

/// Dashboard-level banner shown across the top of the main dashboard workspace.
pub struct DashboardCreditBanner;

impl DashboardCreditBanner {
    pub fn render_opt() -> Option<AnyElement> {
        if !CreditStore::is_out_of_credits() || CreditStore::is_banner_dismissed() {
            return None;
        }

        let billing_url = CreditStore::global_error()
            .map(|i| i.billing_url)
            .unwrap_or_else(|| format!("{}/account/credits", crate::data::client::service_base_url().trim_end_matches('/')));
        let url_clone = billing_url.clone();

        Some(
            div()
                .id("dashboard-credit-banner")
                .w_full()
                .min_w_0()
                .px(px(16.0))
                .py(px(7.0))
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap(px(12.0))
                .bg(rgb(0x2d1a10))
                .border_b_1()
                .border_color(rgb(0x78350f))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(8.0))
                        .min_w_0()
                        .flex_1()
                        .child(
                            Icon::new(IconName::TriangleAlert)
                                .size(px(14.0))
                                .text_color(rgb(0xfbbf24))
                                .flex_shrink_0(),
                        )
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(0xfef3c7))
                                .child("Credits Exhausted:"),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(0xfde68a))
                                .truncate()
                                .child("Market-data requests on dashboard modules are paused until credits are topped up."),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(8.0))
                        .flex_shrink_0()
                        .child(
                            Button::new("dashboard-manage-credits-btn")
                                .label("Manage Credits")
                                .xsmall()
                                .primary()
                                .on_click(move |_, _, _| {
                                    CreditStore::open_billing_page(Some(&url_clone));
                                }),
                        )
                        .child(
                            Button::new("dashboard-dismiss-credits-btn")
                                .icon(IconName::Close)
                                .ghost()
                                .xsmall()
                                .on_click(|_, _, _| {
                                    CreditStore::dismiss_banner();
                                }),
                        ),
                )
                .into_any_element(),
        )
    }
}
