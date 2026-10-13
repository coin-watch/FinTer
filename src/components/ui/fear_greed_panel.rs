use gpui::{
    div, px, Context, IntoElement, ParentElement, Render, Styled, Window,
    SharedString, rgba
};
use gpui_component::theme::ActiveTheme;
use serde::Deserialize;

use crate::data::client::tokio_runtime;

#[derive(Deserialize, Debug, Clone)]
struct FngData {
    value: String,
    value_classification: String,
    timestamp: String,
}

#[derive(Deserialize, Debug, Clone)]
struct FngResponse {
    data: Vec<FngData>,
}

#[derive(Clone)]

pub struct FearGreedPanel {
    value: Option<f32>,
    classification: Option<SharedString>,
    updated_at: Option<chrono::DateTime<chrono::Local>>,
    is_loading: bool,
    error: Option<SharedString>,
    
}

impl FearGreedPanel {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let mut panel = Self {
            value: None,
            classification: None,
            updated_at: None,
            is_loading: true,
            error: None,
            
        };

        panel.fetch_data(cx);
        panel
    }

    fn fetch_data(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let result = tokio_runtime().spawn(async move {
                let res = reqwest::get("https://api.alternative.me/fng/?limit=1")
                    .await
                    .and_then(|r| r.error_for_status());
                    
                match res {
                    Ok(r) => {
                        match r.json::<FngResponse>().await {
                            Ok(json) => Ok(json),
                            Err(_) => Err("Failed to parse JSON".to_string()),
                        }
                    }
                    Err(e) => Err(e.to_string()),
                }
            }).await.unwrap_or_else(|e| Err(e.to_string()));

            let _ = this.update(cx, |this, cx| {
                this.is_loading = false;
                match result {
                    Ok(resp) => {
                        if let Some(d) = resp.data.first() {
                            this.value = d.value.parse::<f32>().ok();
                            this.classification = Some(d.value_classification.clone().into());
                            
                            if let Ok(ts) = d.timestamp.parse::<i64>() {
                                this.updated_at = chrono::DateTime::from_timestamp(ts, 0)
                                    .map(|dt| dt.with_timezone(&chrono::Local));
                            }
                            this.error = None;
                        } else {
                            this.error = Some("No data returned".into());
                        }
                    }
                    Err(e) => {
                        this.error = Some(e.into());
                    }
                }
                cx.notify();
            });
        }).detach();
    }
}

impl Render for FearGreedPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = &cx.theme().colors;

        if self.is_loading {
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(colors.background)
                .child(
                    div()
                        .text_sm()
                        .text_color(colors.muted_foreground)
                        .child("Loading index..."),
                )
                .into_any_element();
        }

        if let Some(err) = &self.error {
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(colors.background)
                .child(
                    div()
                        .text_sm()
                        .text_color(rgba(0xef4444ff))
                        .child(err.clone()),
                )
                .into_any_element();
        }

        let val = self.value.unwrap_or(0.0);
        let classification = self.classification.as_ref().map(|s| s.as_ref()).unwrap_or("Unknown");
        let val_pct = (val / 100.0).clamp(0.0, 1.0);

        div()
            .size_full()
            .bg(colors.background)
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .p(px(16.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .child(
                        div()
                            .text_3xl()
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(colors.foreground)
                            .child(format!("{:.0}", val)),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(colors.muted_foreground)
                            .child(classification.to_string()),
                    ),
            )
            .child(
                div()
                    .w_full()
                    .mt(px(24.))
                    .px(px(24.))
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .relative()
                            .w_full()
                            .h(px(16.))
                            .child(
                                // Track
                                div()
                                    .absolute()
                                    .top(px(4.))
                                    .w_full()
                                    .h(px(8.))
                                    .flex()
                                    .rounded_full()
                                    .overflow_hidden()
                                    // 5 distinct FNG segments mapped to exact width percentages
                                    .child(div().w(gpui::relative(0.25)).h_full().bg(gpui::rgba(0xe85d38ff))) // Extreme Fear (0-25)
                                    .child(div().w(gpui::relative(0.20)).h_full().bg(gpui::rgba(0xf18545ff))) // Fear (25-45)
                                    .child(div().w(gpui::relative(0.10)).h_full().bg(gpui::rgba(0x9ca3afff))) // Neutral (45-55)
                                    .child(div().w(gpui::relative(0.20)).h_full().bg(gpui::rgba(0x8bc34aff))) // Greed (55-75)
                                    .child(div().w(gpui::relative(0.25)).h_full().bg(gpui::rgba(0x28a745ff))), // Extreme Greed (75-100)
                            )
                            .child(
                                // Thumb
                                div()
                                    .absolute()
                                    .top(px(0.))
                                    .left(gpui::relative(val_pct))
                                    .ml(px(-4.))
                                    .w(px(8.))
                                    .h(px(16.))
                                    .bg(gpui::white())
                                    .border_1()
                                    .border_color(gpui::rgba(0x00000044))
                                    .rounded_sm()
                                    .shadow_md(),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .w_full()
                            .mt(px(8.))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(gpui::rgba(0xe85d38ff))
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .child("Ext. Fear"),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(gpui::rgba(0x9ca3afff))
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .child("Neutral"),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(gpui::rgba(0x28a745ff))
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .child("Ext. Greed"),
                            ),
                    ),
            )
            .child(
                div()
                    .absolute()
                    .bottom(px(6.))
                    .w_full()
                    .flex()
                    .justify_center()
                    .child(
                        div()
                            .text_color(colors.muted_foreground)
                            .text_size(px(10.))
                            .child(if let Some(dt) = self.updated_at {
                                format!("Updated {}", dt.format("%b %-d, %I:%M %p"))
                            } else {
                                "".to_string()
                            })
                    )
            )
            .into_any_element()
    }
}
