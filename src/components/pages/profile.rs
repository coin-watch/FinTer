use crate::data::models::{AuthSession, SettingsData};
use gpui::prelude::*;
use gpui::*;
use gpui_component::{ActiveTheme, button::{Button, ButtonVariants}};
use serde_json::Value;

pub struct ProfilePage;

fn value(data: &Value, path: &[&str]) -> String {
    let value = path.iter().fold(data, |value, key| &value[*key]);
    match value {
        Value::Null => "—".into(),
        Value::String(value) if value.is_empty() => "—".into(),
        Value::String(value) => value.clone(),
        Value::Bool(value) => if *value { "Yes" } else { "No" }.into(),
        _ => value.to_string(),
    }
}

fn metric_card(
    label: &'static str,
    amount: String,
    detail: &'static str,
    featured: bool,
    cx: &App,
) -> Div {
    let colors = &cx.theme().colors;
    let background = if featured { colors.primary.opacity(0.08) } else { colors.group_box };
    let border = if featured { colors.primary.opacity(0.28) } else { colors.border };

    div()
        .min_w(px(0.))
        .flex_1()
        .p(px(20.))
        .bg(background)
        .border_1()
        .border_color(border)
        .child(div().text_sm().text_color(colors.muted_foreground).child(label))
        .child(
            div()
                .mt(px(7.))
                .text_3xl()
                .font_weight(FontWeight::BOLD)
                .child(amount),
        )
        .child(
            div()
                .mt(px(8.))
                .text_xs()
                .text_color(colors.muted_foreground)
                .child(detail),
        )
}

fn detail_row(label: &'static str, content: String, last: bool, cx: &App) -> Div {
    let colors = &cx.theme().colors;
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(16.))
        .py(px(10.))
        .when(!last, |this| this.border_b_1().border_color(colors.border))
        .child(div().text_sm().text_color(colors.muted_foreground).child(label))
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .text_color(colors.foreground)
                .child(content),
        )
}

impl ProfilePage {
    pub fn build_element(
        session: Option<AuthSession>,
        settings: Option<SettingsData>,
        summary: Option<Value>,
        loading: bool,
        authenticated: bool,
        cx: &App,
    ) -> impl IntoElement {
        let colors = &cx.theme().colors;
        let data = summary.unwrap_or(Value::Null);
        let name = data["user"]["name"]
            .as_str()
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
            .or_else(|| session.as_ref().and_then(|session| session.name.clone()))
            .unwrap_or_else(|| if authenticated { "Your account" } else { "Guest workspace" }.into());
        let email = if authenticated {
            session.as_ref().map(|session| session.email.clone()).unwrap_or_else(|| "Email unavailable".into())
        } else {
            "Local preferences only".into()
        };
        let balance = value(&data, &["credits", "balance"]);
        let free_allowance = value(&data, &["credits", "monthly_free_remaining"]);
        let subscription_credits = if data["subscription"].is_null() {
            "—".into()
        } else {
            value(&data, &["subscription", "status"])
        };
        let plan = if data["subscription"].is_null() { "Free".into() } else { value(&data, &["subscription", "status"]) };
        let workspace_count = value(&data, &["stats", "modules"]);
        let indicator_count = value(&data, &["stats", "indicators"]);
        let preferences = if authenticated {
            data["settings"].clone()
        } else {
            serde_json::to_value(settings.unwrap_or_default()).unwrap_or(Value::Null)
        };
        let theme = preferences["theme"].as_str().unwrap_or("Current app theme").to_owned();
        let status = if loading {
            "Syncing latest account data…"
        } else if authenticated && data.is_null() {
            "Account data is currently unavailable"
        } else if authenticated {
            "Live account snapshot"
        } else {
            "Local workspace"
        };

        div()
            .id("profile-page")
            .size_full()
            .overflow_hidden()
            .bg(colors.background)
            .text_color(colors.foreground)
            .child(
                div()
                    .size_full()
                    .max_w(px(1120.))
                    .mx_auto()
                    .px(px(28.))
                    .py(px(24.))
                    .flex()
                    .flex_col()
                    .gap(px(18.))
                    .child(
                        div()
                            .flex()
                            .items_end()
                            .justify_between()
                            .gap(px(20.))
                            .child(
                                div()
                                    .min_w(px(0.))
                                    .child(
                                        div()
                                            .mb(px(7.))
                                            .text_xs()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(colors.primary)
                                            .child("ACCOUNT / USAGE"),
                                    )
                                    .child(div().text_3xl().font_weight(FontWeight::SEMIBOLD).child("Profile & credits"))
                                    .child(div().mt(px(5.)).text_sm().text_color(colors.muted_foreground).child("Your market-data balance, plan, and workspace at a glance.")),
                            )
                            .when(authenticated, |this| this.child(
                                Button::new("profile-billing")
                                    .label("Manage billing")
                                    .primary()
                                    .on_click(|_, _, _| { let _ = webbrowser::open("https://coinwat.ch/account/credits"); }),
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(px(12.))
                            .child(metric_card("Available balance", balance, "Credits available for market-data requests.", true, cx))
                            .child(metric_card("Free allowance", free_allowance, "Resets with your monthly free period.", false, cx))
                            .child(metric_card("Current plan", subscription_credits, "Subscription status and rollover eligibility.", false, cx)),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(px(12.))
                            .flex_1()
                            .min_h(px(0.))
                            .child(
                                div()
                                    .min_w(px(0.))
                                    .flex_1()
                                    .p(px(20.))
                                    .bg(colors.group_box)
                                    .border_1()
                                    .border_color(colors.border)
                                    .child(div().text_base().font_weight(FontWeight::SEMIBOLD).child("Account"))
                                    .child(div().mt(px(3.)).text_sm().text_color(colors.muted_foreground).child(email))
                                    .child(div().mt(px(14.)).child(detail_row("Plan", plan, false, cx)))
                                    .child(detail_row("Security", value(&data, &["security", "mfa_enabled"]), false, cx))
                                    .child(detail_row("Status", status.into(), true, cx)),
                            )
                            .child(
                                div()
                                    .min_w(px(0.))
                                    .flex_1()
                                    .p(px(20.))
                                    .bg(colors.group_box)
                                    .border_1()
                                    .border_color(colors.border)
                                    .child(div().text_base().font_weight(FontWeight::SEMIBOLD).child("Workspace"))
                                    .child(div().mt(px(3.)).text_sm().text_color(colors.muted_foreground).child(name))
                                    .child(div().mt(px(14.)).child(detail_row("Saved dashboards", workspace_count, false, cx)))
                                    .child(detail_row("Saved indicators", indicator_count, false, cx))
                                    .child(detail_row("Selected theme", theme, true, cx)),
                            ),
                    )
                    .child(
                        div()
                            .pt(px(2.))
                            .text_xs()
                            .text_color(colors.muted_foreground)
                            .child("This page refreshes automatically while open. View detailed credit activity in Coinwatch."),
                    ),
            )
    }
}
