use crate::data::client::ApiClient;
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::theme::ActiveTheme;
use gpui_component::Icon;

/// The login page -- displayed when the user is not authenticated.
/// Features a centered card with email/password fields and login/signup buttons.
pub struct LoginPage {
    api_client: ApiClient,
    email: Option<Entity<InputState>>,
    password: Option<Entity<InputState>>,
    error_message: Option<String>,
    info_message: Option<String>,
    is_loading: bool,
}

impl LoginPage {
    /// Create a new LoginPage. Input states are created lazily on first render
    /// because `InputState::new` requires `&mut Window` which is only available
    /// in `Render::render()`.
    pub fn new(api_client: ApiClient, _cx: &mut Context<Self>) -> Self {
        Self {
            api_client,
            email: None,
            password: None,
            error_message: None,
            info_message: None,
            is_loading: false,
        }
    }

    /// Lazily initializes the input state entities on first render.
    fn ensure_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.email.is_none() {
            let email = cx.new(|cx| InputState::new(window, cx).placeholder("Email address"));
            self.email = Some(email);
        }
        if self.password.is_none() {
            let password = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("Password")
                    .masked(true)
            });
            // Submit on Enter in password field -> trigger login
            cx.subscribe(&password, |this, _entity, event: &InputEvent, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.do_login(cx);
                }
            })
            .detach();
            self.password = Some(password);
        }
    }

    fn do_login(&mut self, cx: &mut Context<Self>) {
        if self.is_loading {
            return;
        }

        let email = self
            .email
            .as_ref()
            .map(|e| e.read(cx).value().to_string())
            .unwrap_or_default();
        let password = self
            .password
            .as_ref()
            .map(|p| p.read(cx).value().to_string())
            .unwrap_or_default();

        if email.is_empty() || password.is_empty() {
            self.error_message = Some("Please enter email and password".into());
            cx.notify();
            return;
        }

        self.is_loading = true;
        self.error_message = None;
        self.info_message = None;
        cx.notify();

        let client = self.api_client.clone();
        cx.spawn(async move |this, cx| {
            let result = client.login(&email, &password);
            _ = this.update(cx, |this, cx| {
                this.is_loading = false;
                match result {
                    Ok(_session) => {
                        this.info_message = Some("Login successful!".into());
                        cx.emit(AuthEvent::LoggedIn);
                    }
                    Err(e) => {
                        this.error_message = Some(format!("{}", e));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn do_signup(&mut self, cx: &mut Context<Self>) {
        if self.is_loading {
            return;
        }

        self.is_loading = true;
        self.error_message = None;
        self.info_message = Some("Opening signup page...".into());
        cx.notify();

        let client = self.api_client.clone();
        cx.spawn(async move |this, cx| {
            let result = client.open_signup_page();
            _ = this.update(cx, |this, cx| {
                this.is_loading = false;
                match result {
                    Ok(_) => {
                        this.info_message = Some("Signup page opened in browser.".into());
                    }
                    Err(e) => {
                        this.error_message = Some(format!("Failed to open signup page: {}", e));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn do_google_login(&mut self, cx: &mut Context<Self>) {
        if self.is_loading {
            return;
        }

        self.is_loading = true;
        self.error_message = None;
        self.info_message = Some("Opening browser for Google login...".into());
        cx.notify();

        let client = self.api_client.clone();
        cx.spawn(async move |this, cx| {
            let result = client.login_with_google();
            _ = this.update(cx, |this, cx| {
                this.is_loading = false;
                match result {
                    Ok(_session) => {
                        this.info_message = Some("Login successful!".into());
                        cx.emit(AuthEvent::LoggedIn);
                    }
                    Err(e) => {
                        this.error_message = Some(format!("Google login failed: {}", e));
                        this.info_message = None;
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
}

/// Event emitted by LoginPage when auth state changes.
#[derive(Clone)]
pub enum AuthEvent {
    /// User logged in (existing account)
    LoggedIn,
    /// User signed up (new account) - should create default module
    SignedUp,
    /// User chose to continue as guest / skip login
    ContinueAsGuest,
}

impl EventEmitter<AuthEvent> for LoginPage {}

impl Render for LoginPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_inputs(window, cx);

        let error_message = self.error_message.clone();
        let info_message = self.info_message.clone();
        let is_loading = self.is_loading;

        let email_input = self.email.as_ref().unwrap().clone();
        let password_input = self.password.as_ref().unwrap().clone();

        let colors = &cx.theme().colors;
        let primary = colors.primary;
        let primary_fg = colors.primary_foreground;
        let bg = colors.background;
        let border = colors.border;
        let muted = colors.muted_foreground;
        let fg = colors.foreground;
        let bearish = colors.bearish;
        let bullish = colors.bullish;

        let is_desktop = window.viewport_size().width >= px(840.0);

        // Feature highlight row in the left hero panel
        let feature_row = |icon_path: &'static str, title: &'static str, desc: &'static str| {
            div()
                .flex()
                .items_start()
                .gap(px(12.0))
                .child(
                    div()
                        .w(px(32.0))
                        .h(px(32.0))
                        .rounded(px(8.0))
                        .bg(colors.muted.opacity(0.2))
                        .border_1()
                        .border_color(border.opacity(0.6))
                        .flex()
                        .items_center()
                        .justify_center()
                        .flex_shrink_0()
                        .child(Icon::default().path(icon_path).size_4().text_color(primary)),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .child(
                            div()
                                .text_sm()
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(fg)
                                .child(title),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(muted)
                                .line_height(gpui::relative(1.4))
                                .child(desc),
                        ),
                )
        };

        // Left brand hero panel (desktop)
        let left_hero = div()
            .w(px(380.0))
            .p(px(36.0))
            .flex()
            .flex_col()
            .justify_between()
            .border_r_1()
            .border_color(border.opacity(0.6))
            .bg(colors.muted.opacity(0.08))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(16.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(12.0))
                            .child(
                                div()
                                    .w(px(36.0))
                                    .h(px(36.0))
                                    .rounded(px(8.0))
                                    .bg(primary)
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(
                                        Icon::default()
                                            .path("icons/bull-mark.svg")
                                            .size_5()
                                            .text_color(primary_fg),
                                    ),
                            )
                            .child(
                                div()
                                    .text_lg()
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .text_color(fg)
                                    .child("FinTer"),
                            ),
                    )
                    .child(
                        div()
                            .text_2xl()
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(fg)
                            .line_height(gpui::relative(1.2))
                            .child("GPU-accelerated trading terminal"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .line_height(gpui::relative(1.5))
                            .child("Real-time orderbook depth, native multi-series charts, and local-first workspace persistence."),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(18.0))
                    .my(px(24.0))
                    .child(feature_row(
                        "icons/cpu.svg",
                        "60 FPS GPU Rendering",
                        "Sub-millisecond candlestick charts & technical indicators with zero viewport lag.",
                    ))
                    .child(feature_row(
                        "icons/hard-drive.svg",
                        "Local-First Vault",
                        "Workspaces, layouts, and API keys stored locally with client-side security.",
                    ))
                    .child(feature_row(
                        "icons/network.svg",
                        "Unified Exchange Streams",
                        "Direct streaming WebSockets across Binance, OKX, Bybit, and Coinbase.",
                    )),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .text_xs()
                    .text_color(muted)
                    .child(
                        div()
                            .w(px(6.0))
                            .h(px(6.0))
                            .rounded_full()
                            .bg(gpui::rgba(0x2ebd85ff)),
                    )
                    .child("High-performance workstation active"),
            );

        // Right authentication card / form
        let auth_form = div()
            .flex_1()
            .p(if is_desktop { px(36.0) } else { px(24.0) })
            .flex()
            .flex_col()
            .justify_center()
            .gap(px(16.0))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(
                        div()
                            .text_xl()
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(fg)
                            .child("Sign in"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child("Sync your layouts across workstations, or continue in offline mode."),
                    ),
            )
            .when(error_message.is_some(), |this| {
                this.child(
                    div()
                        .w_full()
                        .px(px(12.0))
                        .py(px(8.0))
                        .rounded(px(8.0))
                        .bg(bearish.opacity(0.12))
                        .border_1()
                        .border_color(bearish.opacity(0.35))
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(
                            Icon::default()
                                .path("icons/triangle-alert.svg")
                                .size_3()
                                .text_color(bearish),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(bearish)
                                .child(error_message.unwrap_or_default()),
                        ),
                )
            })
            .when(info_message.is_some(), |this| {
                this.child(
                    div()
                        .w_full()
                        .px(px(12.0))
                        .py(px(8.0))
                        .rounded(px(8.0))
                        .bg(bullish.opacity(0.12))
                        .border_1()
                        .border_color(bullish.opacity(0.35))
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(
                            Icon::default()
                                .path("icons/circle-check.svg")
                                .size_3()
                                .text_color(bullish),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(bullish)
                                .child(info_message.unwrap_or_default()),
                        ),
                )
            })
            // Google OAuth button
            .child(
                div()
                    .id("google-login-btn")
                    .w_full()
                    .h(px(40.0))
                    .rounded(px(8.0))
                    .bg(colors.muted.opacity(0.2))
                    .hover(|s| s.bg(colors.muted.opacity(0.35)))
                    .active(|s| s.bg(colors.muted.opacity(0.45)))
                    .border_1()
                    .border_color(border)
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap(px(8.0))
                    .cursor_pointer()
                    .child(
                        Icon::default()
                            .path("icons/globe.svg")
                            .size_4()
                            .text_color(fg),
                    )
                    .child(
                        div()
                            .text_xs()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(fg)
                            .child("Continue with Google"),
                    )
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.do_google_login(cx);
                    })),
            )
            // Divider
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .my(px(4.0))
                    .child(div().flex_1().h(px(1.0)).bg(border.opacity(0.6)))
                    .child(
                        div()
                            .text_size(px(10.0))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(muted)
                            .child("or with email"),
                    )
                    .child(div().flex_1().h(px(1.0)).bg(border.opacity(0.6))),
            )
            // Email Input
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(
                        div()
                            .text_xs()
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(fg)
                            .child("Email address"),
                    )
                    .child(Input::new(&email_input)),
            )
            // Password Input
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(
                        div()
                            .text_xs()
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(fg)
                            .child("Password"),
                    )
                    .child(Input::new(&password_input).mask_toggle()),
            )
            // Action Buttons
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .mt(px(4.0))
                    .child(
                        div()
                            .id("login-btn")
                            .w_full()
                            .h(px(40.0))
                            .rounded(px(8.0))
                            .bg(primary)
                            .hover(|s| s.bg(primary.opacity(0.85)))
                            .active(|s| s.bg(primary.opacity(0.7)))
                            .flex()
                            .items_center()
                            .justify_center()
                            .gap(px(8.0))
                            .cursor_pointer()
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .text_color(primary_fg)
                                    .child(if is_loading {
                                        "Signing in..."
                                    } else {
                                        "Sign in"
                                    }),
                            )
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.do_login(cx);
                            })),
                    )
                    .child(
                        div()
                            .id("signup-btn")
                            .w_full()
                            .h(px(36.0))
                            .rounded(px(8.0))
                            .border_1()
                            .border_color(border)
                            .bg(colors.muted.opacity(0.08))
                            .hover(|s| s.bg(colors.muted.opacity(0.2)).border_color(fg.opacity(0.4)))
                            .flex()
                            .items_center()
                            .justify_center()
                            .gap(px(6.0))
                            .cursor_pointer()
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(fg)
                                    .child("Create an account"),
                            )
                            .child(
                                Icon::default()
                                    .path("icons/external-link.svg")
                                    .size_3()
                                    .text_color(muted),
                            )
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.do_signup(cx);
                            })),
                    ),
            )
            // Guest access divider & button
            .child(
                div()
                    .pt(px(8.0))
                    .border_t_1()
                    .border_color(border.opacity(0.4))
                    .child(
                        div()
                            .id("guest-btn")
                            .w_full()
                            .h(px(36.0))
                            .rounded(px(8.0))
                            .bg(colors.muted.opacity(0.12))
                            .hover(|s| s.bg(colors.muted.opacity(0.25)))
                            .border_1()
                            .border_color(border.opacity(0.6))
                            .flex()
                            .items_center()
                            .justify_center()
                            .gap(px(6.0))
                            .cursor_pointer()
                            .child(
                                Icon::default()
                                    .path("icons/circle-user.svg")
                                    .size_3()
                                    .text_color(primary),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(fg)
                                    .child("Continue as guest"),
                            )
                            .child(
                                div()
                                    .text_size(px(10.0))
                                    .text_color(muted)
                                    .child("(Offline mode)"),
                            )
                            .on_click(cx.listener(|_this, _event, _window, cx| {
                                cx.emit(AuthEvent::ContinueAsGuest);
                            })),
                    ),
            );

        div()
            .id("login_page_container")
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .size_full()
            .bg(bg)
            .p(px(16.0))
            .child(
                div()
                    .flex()
                    .when(is_desktop, |this| this.flex_row().w(px(820.0)))
                    .when(!is_desktop, |this| this.flex_col().w(px(400.0)))
                    .max_w_full()
                    .bg(colors.muted.opacity(0.12))
                    .rounded(px(16.0))
                    .border_1()
                    .border_color(border)
                    .shadow_xl()
                    .overflow_hidden()
                    .when(is_desktop, |this| this.child(left_hero))
                    .when(!is_desktop, |this| {
                        this.child(
                            div()
                                .p(px(20.0))
                                .pb(px(0.0))
                                .flex()
                                .items_center()
                                .gap(px(10.0))
                                .child(
                                    div()
                                        .w(px(32.0))
                                        .h(px(32.0))
                                        .rounded(px(8.0))
                                        .bg(primary)
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .child(
                                            Icon::default()
                                                .path("icons/bull-mark.svg")
                                                .size_4()
                                                .text_color(primary_fg),
                                        ),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .child(
                                            div()
                                                .text_base()
                                                .font_weight(gpui::FontWeight::BOLD)
                                                .text_color(fg)
                                                .child("FinTer"),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(muted)
                                                .child("GPU Trading Terminal"),
                                        ),
                                ),
                        )
                    })
                    .child(auth_form),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_auth_event_variants() {
        let logged_in = AuthEvent::LoggedIn;
        let signed_up = AuthEvent::SignedUp;
        let guest = AuthEvent::ContinueAsGuest;
        assert!(matches!(logged_in, AuthEvent::LoggedIn));
        assert!(matches!(signed_up, AuthEvent::SignedUp));
        assert!(matches!(guest, AuthEvent::ContinueAsGuest));
    }
}
