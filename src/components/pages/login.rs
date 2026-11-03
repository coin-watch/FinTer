use crate::data::client::ApiClient;
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::scroll::ScrollableElement;
use gpui_component::theme::ActiveTheme;
use gpui_component::Icon;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// The login page -- displayed when the user is not authenticated.
/// Features a fully responsive, clean, user-friendly full-screen layout.
pub struct LoginPage {
    api_client: ApiClient,
    email: Option<Entity<InputState>>,
    password: Option<Entity<InputState>>,
    error_message: Option<String>,
    info_message: Option<String>,
    is_loading: bool,
    /// Set while a Google OAuth flow is waiting for the browser callback.
    google_in_progress: bool,
    /// Cancellation token for the in-flight Google OAuth flow.
    google_cancel: Option<Arc<AtomicBool>>,
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
            google_in_progress: false,
            google_cancel: None,
        }
    }

    /// Lazily initializes the input state entities on first render.
    fn ensure_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.email.is_none() {
            let email = cx.new(|cx| InputState::new(window, cx).placeholder("name@example.com"));
            self.email = Some(email);
        }
        if self.password.is_none() {
            let password = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("••••••••••••")
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
            self.error_message = Some("Please enter both email and password.".into());
            cx.notify();
            return;
        }

        self.is_loading = true;
        self.error_message = None;
        self.info_message = None;
        cx.notify();

        let client = self.api_client.clone();
        cx.spawn(async move |this, cx| {
            // `login` blocks on HTTP, so keep it off the UI thread.
            let result = cx
                .background_executor()
                .spawn(async move { client.login(&email, &password) })
                .await;
            _ = this.update(cx, |this, cx| {
                this.is_loading = false;
                match result {
                    Ok(_session) => {
                        this.info_message = Some("Signed in successfully!".into());
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
        self.info_message = Some("Opening registration in your browser...".into());
        cx.notify();

        let client = self.api_client.clone();
        cx.spawn(async move |this, cx| {
            let result = client.open_signup_page();
            _ = this.update(cx, |this, cx| {
                this.is_loading = false;
                match result {
                    Ok(_) => {
                        this.info_message = Some("Registration page opened in browser.".into());
                    }
                    Err(e) => {
                        this.error_message = Some(format!("Could not open browser: {}", e));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn do_google_login(&mut self, cx: &mut Context<Self>) {
        if self.is_loading || self.google_in_progress {
            return;
        }

        self.is_loading = true;
        self.google_in_progress = true;
        self.error_message = None;
        self.info_message = Some("Connecting to Google in browser...".into());

        let cancel = Arc::new(AtomicBool::new(false));
        self.google_cancel = Some(cancel.clone());
        cx.notify();

        let client = self.api_client.clone();
        cx.spawn(async move |this, cx| {
            // `login_with_google` blocks until the browser hands back a session
            // (or the timeout expires). Run it on the background executor so the
            // UI stays responsive while the user finishes or abandons sign-in.
            let cancel_for_task = cancel.clone();
            let result = cx
                .background_executor()
                .spawn(async move { client.login_with_google_cancellable(cancel_for_task) })
                .await;
            let was_cancelled = cancel.load(Ordering::Relaxed);

            _ = this.update(cx, |this, cx| {
                this.is_loading = false;
                this.google_in_progress = false;
                this.google_cancel = None;
                match result {
                    Ok(_session) => {
                        this.info_message = Some("Signed in with Google!".into());
                        cx.emit(AuthEvent::LoggedIn);
                    }
                    Err(_) if was_cancelled => {
                        // User-initiated cancel: not an error worth alarming about.
                        this.info_message = Some("Sign in cancelled.".into());
                    }
                    Err(e) => {
                        this.error_message = Some(format!("Google sign in failed: {}", e));
                        this.info_message = None;
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Stop waiting for the Google OAuth callback. The background task notices
    /// the flag within ~250ms, closes the loopback listener, and clears the
    /// loading state.
    fn cancel_google_login(&mut self, cx: &mut Context<Self>) {
        if let Some(cancel) = self.google_cancel.as_ref() {
            cancel.store(true, Ordering::Relaxed);
            self.info_message = Some("Cancelling sign in...".into());
            cx.notify();
        }
    }
}

impl Drop for LoginPage {
    fn drop(&mut self) {
        // Make sure an abandoned OAuth listener does not outlive the page.
        if let Some(cancel) = self.google_cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
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
        let google_in_progress = self.google_in_progress;

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

        let viewport = window.viewport_size();
        // The split layout only makes sense when the hero panel still has room
        // to breathe next to the fixed-width auth card. Below this width we
        // collapse to the centred, single-column form.
        let is_desktop = viewport.width >= px(1000.0);
        let is_compact_height = viewport.height < px(700.0);
        let is_wide = viewport.width >= px(1120.0);

        // The hero panel receives whatever width the fixed-width auth card
        // leaves over. Derive its metrics from that actual space (and the
        // available height) so the copy, icons and spacing grow/shrink to fit
        // the container instead of overflowing it.
        let auth_card_width = if is_wide { 520.0 } else { 460.0 };
        let hero_width = if is_desktop {
            (viewport.width - px(auth_card_width)).max(px(0.0))
        } else {
            viewport.width
        };
        let hero_height =
            (viewport.height - crate::components::ui::title_bar::TITLE_BAR_HEIGHT).max(px(0.0));

        // 0.0 = tightest layout, 1.0 = roomiest layout.
        let hero_width_px = f32::from(hero_width);
        let hero_height_px = f32::from(hero_height);
        let width_scale = ((hero_width_px - 480.0) / (900.0 - 480.0)).clamp(0.0, 1.0);
        let height_scale = ((hero_height_px - 480.0) / (800.0 - 480.0)).clamp(0.0, 1.0);
        let hero_scale = width_scale.min(height_scale);
        let scaled = move |min: f32, max: f32| px(min + (max - min) * hero_scale);

        let hero_padding = scaled(24.0, 56.0);
        let hero_section_gap = scaled(18.0, 32.0);
        let hero_top_gap = scaled(14.0, 24.0);
        let hero_headline_gap = scaled(6.0, 10.0);
        let hero_features_gap = scaled(12.0, 20.0);
        let hero_feature_gap = scaled(10.0, 14.0);
        let hero_feature_text_gap = scaled(2.0, 3.0);
        let hero_brand_gap = scaled(8.0, 12.0);
        let hero_brand_icon = scaled(32.0, 40.0);
        let hero_brand_glyph = scaled(19.0, 24.0);
        let hero_brand_text = scaled(20.0, 24.0);
        let hero_headline_size = scaled(22.0, 30.0);
        let hero_subtitle_size = scaled(12.0, 14.0);
        let hero_feature_icon = scaled(30.0, 38.0);
        let hero_feature_glyph = scaled(14.0, 16.0);
        let hero_feature_title = scaled(12.0, 14.0);
        let hero_feature_desc = scaled(10.5, 12.0);
        let hero_note_gap = scaled(6.0, 8.0);
        let hero_note_padding_x = scaled(9.0, 12.0);
        let hero_note_padding_y = scaled(6.0, 8.0);
        let hero_note_dot = scaled(6.0, 7.0);
        let hero_note_text = scaled(10.5, 12.0);
        let hero_brand_radius = scaled(8.0, 10.0);
        let hero_feature_radius = scaled(8.0, 10.0);

        // Friendly feature highlight row
        let feature_item = |icon_path: &'static str, title: &'static str, desc: &'static str| {
            div()
                .flex()
                .items_start()
                .gap(hero_feature_gap)
                .w_full()
                .min_w_0()
                .child(
                    div()
                        .w(hero_feature_icon)
                        .h(hero_feature_icon)
                        .rounded(hero_feature_radius)
                        .bg(colors.muted.opacity(0.18))
                        .border_1()
                        .border_color(border.opacity(0.6))
                        .flex()
                        .items_center()
                        .justify_center()
                        .flex_shrink_0()
                        .child(
                            Icon::default()
                                .path(icon_path)
                                .size(hero_feature_glyph)
                                .text_color(primary),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .gap(hero_feature_text_gap)
                        .min_w_0()
                        .child(
                            div()
                                .w_full()
                                .min_w_0()
                                .text_size(hero_feature_title)
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(fg)
                                .whitespace_normal()
                                .child(title),
                        )
                        .child(
                            div()
                                .w_full()
                                .min_w_0()
                                .text_size(hero_feature_desc)
                                .text_color(muted)
                                .line_height(gpui::relative(1.4))
                                .whitespace_normal()
                                .child(desc),
                        ),
                )
        };

        // Left brand hero panel (desktop)
        // `min_w_0` is important: without it the flex item refuses to shrink
        // below the text's min-content width and pushes the auth card off the
        // right edge on narrow/high-DPI windows.
        let left_hero = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_x_hidden()
            .overflow_y_scrollbar()
            .border_r_1()
            .border_color(border.opacity(0.6))
            .bg(colors.muted.opacity(0.08))
            .child(
                div()
                    .min_h_full()
                    .w_full()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .justify_between()
                    .p(hero_padding)
                    .gap(hero_section_gap)
                    // Top: Brand & Welcoming message
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(hero_top_gap)
                            .w_full()
                            .min_w_0()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(hero_brand_gap)
                                    .w_full()
                                    .min_w_0()
                                    .child(
                                        div()
                                            .w(hero_brand_icon)
                                            .h(hero_brand_icon)
                                            .rounded(hero_brand_radius)
                                            .bg(primary)
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .shadow_md()
                                            .child(
                                                Icon::default()
                                                    .path("icons/bull-mark.svg")
                                                    .size(hero_brand_glyph)
                                                    .text_color(primary_fg),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .text_size(hero_brand_text)
                                            .font_weight(gpui::FontWeight::BOLD)
                                            .text_color(fg)
                                            .child("FinTer"),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(hero_headline_gap)
                                    .w_full()
                                    .min_w_0()
                                    .child(
                                        div()
                                            .w_full()
                                            .min_w_0()
                                            .text_size(hero_headline_size)
                                            .font_weight(gpui::FontWeight::BOLD)
                                            .text_color(fg)
                                            .line_height(gpui::relative(1.2))
                                            .whitespace_normal()
                                            .child("All your markets in one clean view"),
                                    )
                                    .child(
                                        div()
                                            .w_full()
                                            .min_w_0()
                                            .text_size(hero_subtitle_size)
                                            .text_color(muted)
                                            .line_height(gpui::relative(1.5))
                                            .whitespace_normal()
                                            .child("Follow live cryptocurrency prices, explore multi-timeframe charts, and organize your favorite trading pairs."),
                                    ),
                            ),
                    )
                    // Middle: Simple everyday user benefits (no technical jargon)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(hero_features_gap)
                            .w_full()
                            .min_w_0()
                            .child(feature_item(
                                "icons/chart-candlestick.svg",
                                "Live Market Charts",
                                "Track price action with clean candlestick charts and customizable visual indicators.",
                            ))
                            .child(feature_item(
                                "icons/globe.svg",
                                "Multiple Exchanges",
                                "Follow trading pairs seamlessly across Binance, OKX, Bybit, Kraken, and more.",
                            ))
                            .child(feature_item(
                                "icons/layout-dashboard.svg",
                                "Customizable Layouts",
                                "Split your screens, organize watchlists, and build the perfect trading workspace.",
                            )),
                    )
                    // Bottom: Simple friendly note
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(hero_note_gap)
                            .w_full()
                            .min_w_0()
                            .px(hero_note_padding_x)
                            .py(hero_note_padding_y)
                            .rounded(px(8.0))
                            .bg(colors.muted.opacity(0.12))
                            .border_1()
                            .border_color(border.opacity(0.4))
                            .child(
                                div()
                                    .w(hero_note_dot)
                                    .h(hero_note_dot)
                                    .rounded_full()
                                    .bg(bullish),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_size(hero_note_text)
                                    .text_color(muted)
                                    .whitespace_normal()
                                    .child("Free to use • Your settings stay saved on your device"),
                            ),
                    ),
            );

        // Right authentication card / form (fully responsive with vertical scrolling)
        // Allow the card (and the single-column mobile form) to shrink instead
        // of overflowing when the window is small.
        let auth_form = div()
            .when(is_desktop, |this| {
                if is_wide {
                    this.w(px(520.0)).flex_shrink_0()
                } else {
                    this.w(px(460.0)).flex_shrink_0()
                }
            })
            .when(!is_desktop, |this| this.flex_1())
            .min_w_0()
            .h_full()
            .overflow_x_hidden()
            .overflow_y_scrollbar()
            .bg(bg)
            .child(
                div()
                    .min_h_full()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .justify_center()
                    .items_center()
                    .p(if is_compact_height { px(24.0) } else { px(36.0) })
                    .child(
                        div()
                            .w_full()
                            .max_w(px(380.0))
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(if is_compact_height { px(16.0) } else { px(20.0) })
                            // Mobile brand header (shown only when left hero is hidden)
                            .when(!is_desktop, |this| {
                                this.child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(10.0))
                                        .mb(px(4.0))
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
                                                .text_xl()
                                                .font_weight(gpui::FontWeight::BOLD)
                                                .text_color(fg)
                                                .child("FinTer"),
                                        ),
                                )
                            })
                            // Form Title & Subtitle
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(4.0))
                                    .min_w_0()
                                    .child(
                                        div()
                                            .text_2xl()
                                            .font_weight(gpui::FontWeight::BOLD)
                                            .text_color(fg)
                                            .child("Sign in"),
                                    )
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(muted)
                                            .child("Sign in to sync your workspaces across devices."),
                                    ),
                            )
                            // Feedback messages
                            .when(error_message.is_some(), |this| {
                                this.child(
                                    div()
                                        .w_full()
                                        .min_w_0()
                                        .px(px(12.0))
                                        .py(px(8.0))
                                        .rounded(px(8.0))
                                        .bg(bearish.opacity(0.1))
                                        .border_1()
                                        .border_color(bearish.opacity(0.35))
                                        .flex()
                                        .items_center()
                                        .gap(px(8.0))
                                        .child(
                                            Icon::default()
                                                .path("icons/triangle-alert.svg")
                                                .size_4()
                                                .flex_shrink_0()
                                                .text_color(bearish),
                                        )
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
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
                                        .min_w_0()
                                        .px(px(12.0))
                                        .py(px(8.0))
                                        .rounded(px(8.0))
                                        .bg(bullish.opacity(0.1))
                                        .border_1()
                                        .border_color(bullish.opacity(0.35))
                                        .flex()
                                        .items_center()
                                        .gap(px(8.0))
                                        .child(
                                            Icon::default()
                                                .path("icons/circle-check.svg")
                                                .size_4()
                                                .flex_shrink_0()
                                                .text_color(bullish),
                                        )
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .text_xs()
                                                .text_color(bullish)
                                                .child(info_message.unwrap_or_default()),
                                        ),
                                )
                            })
                            // Google Login Button
                            .child(
                                div()
                                    .id("google-login-btn")
                                    .w_full()
                                    .h(px(42.0))
                                    .min_h(px(42.0))
                                    .flex_shrink_0()
                                    .rounded(px(8.0))
                                    .bg(colors.muted.opacity(0.14))
                                    .hover(|s| s.bg(colors.muted.opacity(0.24)))
                                    .active(|s| s.bg(colors.muted.opacity(0.34)))
                                    .border_1()
                                    .border_color(border)
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .gap(px(10.0))
                                    .cursor_pointer()
                                    .child(
                                        Icon::default()
                                            .path("icons/globe.svg")
                                            .size_4()
                                            .text_color(fg),
                                    )
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(gpui::FontWeight::MEDIUM)
                                            .text_color(fg)
                                            .child(if google_in_progress {
                                                "Cancel sign in"
                                            } else {
                                                "Continue with Google"
                                            }),
                                    )
                                    .on_click(cx.listener(|this, _event, _window, cx| {
                                        if this.google_in_progress {
                                            this.cancel_google_login(cx);
                                        } else {
                                            this.do_google_login(cx);
                                        }
                                    })),
                            )
                            // Divider
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(12.0))
                                    .my(px(2.0))
                                    .child(div().flex_1().h(px(1.0)).bg(border.opacity(0.5)))
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(muted)
                                            .child("or with email"),
                                    )
                                    .child(div().flex_1().h(px(1.0)).bg(border.opacity(0.5))),
                            )
                            // Email & Password Fields
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(14.0))
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
                                                    .child("Email"),
                                            )
                                            .child(Input::new(&email_input).h(px(42.0))),
                                    )
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
                                            .child(Input::new(&password_input).h(px(42.0)).mask_toggle()),
                                    ),
                            )
                            // Sign In Button & Registration Link
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(12.0))
                                    .child(
                                        div()
                                            .id("login-btn")
                                            .w_full()
                                            .h(px(42.0))
                                            .min_h(px(42.0))
                                            .flex_shrink_0()
                                            .rounded(px(8.0))
                                            .bg(primary)
                                            .hover(|s| s.bg(primary.opacity(0.88)))
                                            .active(|s| s.bg(primary.opacity(0.75)))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .gap(px(8.0))
                                            .cursor_pointer()
                                            .child(
                                                div()
                                                    .text_sm()
                                                    .font_weight(gpui::FontWeight::SEMIBOLD)
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
                                            .flex()
                                            .flex_wrap()
                                            .items_center()
                                            .justify_center()
                                            .gap(px(4.0))
                                            .min_w_0()
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(muted)
                                                    .child("Don't have an account?"),
                                            )
                                            .child(
                                                div()
                                                    .id("signup-btn")
                                                    .flex()
                                                    .items_center()
                                                    .gap(px(3.0))
                                                    .cursor_pointer()
                                                    .child(
                                                        div()
                                                            .text_xs()
                                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                                            .text_color(primary)
                                                            .hover(|s| s.underline())
                                                            .child("Create one"),
                                                    )
                                                    .child(
                                                        Icon::default()
                                                            .path("icons/external-link.svg")
                                                            .size_3()
                                                            .text_color(primary),
                                                    )
                                                    .on_click(cx.listener(|this, _event, _window, cx| {
                                                        this.do_signup(cx);
                                                    })),
                                            ),
                                    ),
                            )
                            // Guest Option
                            .child(
                                div()
                                    .pt(px(14.0))
                                    .border_t_1()
                                    .border_color(border.opacity(0.4))
                                    .child(
                                        div()
                                            .id("guest-btn")
                                            .w_full()
                                            .h(px(42.0))
                                            .min_h(px(42.0))
                                            .flex_shrink_0()
                                            .rounded(px(8.0))
                                            .bg(colors.muted.opacity(0.1))
                                            .hover(|s| s.bg(colors.muted.opacity(0.2)))
                                            .active(|s| s.bg(colors.muted.opacity(0.3)))
                                            .border_1()
                                            .border_color(border.opacity(0.5))
                                            .flex()
                                            .items_center()
                                            .justify_between()
                                            .px(px(14.0))
                                            .cursor_pointer()
                                            .child(
                                                div()
                                                    .flex()
                                                    .items_center()
                                                    .gap(px(8.0))
                                                    .child(
                                                        Icon::default()
                                                            .path("icons/circle-user.svg")
                                                            .size_4()
                                                            .text_color(primary),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_xs()
                                                            .font_weight(gpui::FontWeight::MEDIUM)
                                                            .text_color(fg)
                                                            .child("Continue as guest"),
                                                    ),
                                            )
                                            .child(
                                                Icon::default()
                                                    .path("icons/arrow-right.svg")
                                                    .size_3()
                                                    .text_color(muted),
                                            )
                                            .on_click(cx.listener(|this, _event, _window, cx| {
                                                // Leaving for guest mode abandons any
                                                // in-flight Google sign-in.
                                                this.cancel_google_login(cx);
                                                cx.emit(AuthEvent::ContinueAsGuest);
                                            })),
                                    ),
                            ),
                    ),
            );

        div()
            .id("login_page_fullscreen")
            .size_full()
            .flex()
            .flex_row()
            .bg(bg)
            .overflow_hidden()
            .when(is_desktop, |this| this.child(left_hero))
            .child(auth_form)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[core::prelude::v1::test]
    fn test_auth_event_variants() {
        let logged_in = AuthEvent::LoggedIn;
        let signed_up = AuthEvent::SignedUp;
        let guest = AuthEvent::ContinueAsGuest;
        assert!(matches!(logged_in, AuthEvent::LoggedIn));
        assert!(matches!(signed_up, AuthEvent::SignedUp));
        assert!(matches!(guest, AuthEvent::ContinueAsGuest));
    }
}
