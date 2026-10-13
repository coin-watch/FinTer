use chrono::Utc;
use chrono_tz::Tz;
use gpui::{div, px, Context, IntoElement, ParentElement, Render, Styled, Window};
use gpui_component::{theme::ActiveTheme, v_flex};

pub struct ClockPanel {
    timezone: String,
}

impl ClockPanel {
    pub fn new(_pane_id: String, timezone: String, cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(1000))
                    .await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        })
        .detach();

        Self { timezone }
    }

    pub fn set_timezone(&mut self, timezone: impl Into<String>) {
        self.timezone = timezone.into();
    }
}

impl Render for ClockPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = &cx.theme().colors;
        let now = Utc::now();
        let tz: Option<Tz> = self.timezone.parse().ok();
        let (date_str, time_str) = match tz {
            Some(tz) => {
                let local = now.with_timezone(&tz);
                (
                    local.format("%d/%m/%Y").to_string(),
                    local.format("%H:%M:%S").to_string(),
                )
            }
            None => (
                now.format("%d/%m/%Y").to_string(),
                now.format("%H:%M:%S").to_string(),
            ),
        };

        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .bg(colors.background)
            .gap_1()
            .child(
                div()
                    .text_sm()
                    .text_color(colors.muted_foreground)
                    .child(date_str),
            )
            .child(
                div()
                    .text_size(px(36.0))
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(colors.foreground)
                    .child(time_str),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(colors.muted_foreground)
                    .child(self.timezone.clone()),
            )
    }
}
