use crate::data::models::{AuthSession, SettingsData};
use gpui::*;
use gpui_component::button::{Button, ButtonVariants};
use serde_json::Value;

pub struct ProfilePage;

fn text(value: &Value) -> String {
    match value {
        Value::Null => "Not available".into(),
        Value::String(value) if value.is_empty() => "Not set".into(),
        Value::String(value) => value.clone(),
        Value::Bool(value) => if *value { "Yes" } else { "No" }.into(),
        _ => value.to_string(),
    }
}

fn row(title: impl Into<SharedString>, value: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .flex_wrap()
        .justify_between()
        .gap(px(12.))
        .py(px(9.))
        .border_b_1()
        .border_color(rgb(0x2a3835))
        .child(
            div()
                .text_sm()
                .text_color(rgb(0x9eafa6))
                .child(title.into()),
        )
        .child(div().text_sm().child(value.into()))
}

fn section(title: &'static str, rows: Vec<Div>) -> Div {
    div()
        .w_full()
        .p(px(22.))
        .rounded(px(8.))
        .bg(rgb(0x17201f))
        .border_1()
        .border_color(rgb(0x2a3835))
        .child(
            div()
                .mb(px(12.))
                .text_lg()
                .font_weight(FontWeight::BOLD)
                .child(title),
        )
        .children(rows)
}

fn records(summary: &Value, key: &str, fields: &'static [(&'static str, &'static str)]) -> Vec<Div> {
    let Some(items) = summary[key].as_array() else {
        return vec![row("Status", "Not available")];
    };
    if items.is_empty() {
        return vec![row("Status", "No records")];
    }
    items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            div()
                .mb(px(14.))
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(0xd7a84b))
                        .child(format!("RECORD {:02}", index + 1)),
                )
                .children(
                    fields
                        .iter()
                        .map(|(key, label)| row(*label, text(&item[*key]))),
                )
        })
        .collect()
}

impl ProfilePage {
    pub fn build_element(
        session: Option<AuthSession>,
        settings: Option<SettingsData>,
        summary: Option<Value>,
        loading: bool,
        authenticated: bool,
    ) -> impl IntoElement {
        let data = summary.unwrap_or(Value::Null);
        let name = data["user"]["name"]
            .as_str()
            .map(str::to_owned)
            .or_else(|| session.as_ref().and_then(|s| s.name.clone()))
            .unwrap_or_else(|| {
                if authenticated {
                    "Your account"
                } else {
                    "Guest workspace"
                }
                .into()
            });
        let email = if authenticated {
            session
                .as_ref()
                .map(|s| s.email.clone())
                .unwrap_or_default()
        } else {
            "Local workspace • no cloud account".into()
        };
        let mut sections = vec![];
        if authenticated && !data.is_null() {
            sections.push(section(
                "01 / Identity",
                vec![
                    row("Account ID", text(&data["user"]["id"])),
                    row("Email", text(&data["user"]["email"])),
                    row("Name", text(&data["user"]["name"])),
                    row("MFA enabled", text(&data["security"]["mfa_enabled"])),
                    row(
                        "Verified factors",
                        text(&data["security"]["verified_mfa_factors"]),
                    ),
                ],
            ));
            sections.push(section(
                "02 / Credits & subscription",
                vec![
                    row("Credit balance", text(&data["credits"]["balance"])),
                    row(
                        "Monthly free remaining",
                        text(&data["credits"]["monthly_free_remaining"]),
                    ),
                    row(
                        "Monthly free period",
                        text(&data["credits"]["monthly_free_period"]),
                    ),
                    row("Credits last updated", text(&data["credits"]["updated_at"])),
                    row(
                        "Plan",
                        if data["subscription"].is_null() {
                            "No subscription".into()
                        } else {
                            text(&data["subscription"]["status"])
                        },
                    ),
                    row(
                        "Current period ends",
                        text(&data["subscription"]["current_period_end"]),
                    ),
                    row(
                        "Cancels at period end",
                        text(&data["subscription"]["cancel_at_period_end"]),
                    ),
                ],
            ));
            sections.push(section(
                "03 / Workspace",
                vec![
                    row("Saved dashboards", text(&data["stats"]["modules"])),
                    row("Saved indicators", text(&data["stats"]["indicators"])),
                ],
            ));
            sections.push(section(
                "04 / Recent dashboards · latest 5",
                records(
                    &data,
                    "recent_modules",
                    &[
                        ("module_name", "Name"),
                        ("created_at", "Created"),
                        ("updated_at", "Updated"),
                    ],
                ),
            ));
            sections.push(section(
                "05 / Recent indicators · latest 5",
                records(
                    &data,
                    "recent_indicators",
                    &[
                        ("indicator_name", "Name"),
                        ("type", "Type"),
                        ("public", "Public"),
                        ("created_at", "Created"),
                    ],
                ),
            ));
            sections.push(section(
                "06 / Recent activity · latest 10",
                records(
                    &data,
                    "recent_activity",
                    &[("activity_type", "Activity"), ("created_at", "Time")],
                ),
            ));
            sections.push(section(
                "07 / API keys · metadata only",
                records(
                    &data,
                    "api_keys",
                    &[
                        ("key_name", "Name"),
                        ("permissions", "Permissions"),
                        ("is_active", "Active"),
                        ("last_used_at", "Last used"),
                        ("expires_at", "Expires"),
                        ("created_at", "Created"),
                    ],
                ),
            ));
        }
        let settings = if authenticated {
            data["settings"].clone()
        } else {
            serde_json::to_value(settings.unwrap_or_default()).unwrap_or(Value::Null)
        };
        let settings_rows = settings
            .as_object()
            .map(|object| {
                object
                    .iter()
                    .filter(|(_, value)| !value.is_null())
                    .map(|(key, value)| row(key.clone(), text(value)))
                    .collect()
            })
            .unwrap_or_else(|| vec![row("Status", "Not available")]);
        sections.push(section("08 / Saved preferences", settings_rows));
        div()
            .id("profile-scroll")
            .size_full()
            .overflow_y_scroll()
            .bg(rgb(0x101715))
            .text_color(rgb(0xe8f0e7))
            .child(
                div()
                    .max_w(px(980.))
                    .mx_auto()
                    .w_full()
                    .p(px(28.))
                    .flex()
                    .flex_col()
                    .gap(px(18.))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(0xd7a84b))
                            .child("FINTER / ACCOUNT RECORD"),
                    )
                    .child(div().text_3xl().font_weight(FontWeight::BOLD).child(name))
                    .child(div().text_sm().text_color(rgb(0x9eafa6)).child(email))
                    .child(div().text_sm().child(if loading {
                        "Loading your database records…"
                    } else if authenticated && data.is_null() {
                        "Account data is unavailable. The server must support /api/account-summary."
                    } else if authenticated {
                        "Database snapshot • sensitive credentials are never displayed"
                    } else {
                        "Guest mode • preferences are stored on this device"
                    }))
                    .children(sections)
                    .child(
                        Button::new("profile-billing")
                            .label("Manage billing on CoinWatch")
                            .primary()
                            .on_click(|_, _, _| {
                                let _ = webbrowser::open("https://coinwat.ch/account/credits");
                            }),
                    ),
            )
    }
}
