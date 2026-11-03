use crate::components::pages::login::AuthEvent;
use crate::components::pages::{
    dashboard::DashboardPage,
    home::{HomePage, HomePageEvent},
    login::LoginPage,
    profile::ProfilePage,
    settings::SettingsPage,
};
use crate::components::ui::delete_dashboard_dialog::{
    DeleteDashboardDialogEvent, open_delete_dashboard_dialog,
};
use crate::components::ui::sidebar::{self, SidebarUserInfo};
use crate::components::ui::title_bar::FinTerTitleBar;
use crate::components::ui::update_banner::{UpdateBanner, UpdateBannerEvent};
use crate::data::client::ApiClient;
use crate::data::models::{LayoutPreset, Module, SettingsData};
use crate::data::pane_tree::PaneNode;
use crate::data::theme::apply_theme;
use crate::data::update_manager::UpdateManager;
use crate::data::version::UpdateState;
use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, App, AppContext, Context, Entity, InteractiveElement, IntoElement, ParentElement,
    Render, StatefulInteractiveElement, StyleRefinement, Styled, Task, Window, WindowControlArea, div, px, rgb,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::{Icon, IconName};
use gpui_component::Sizable;
use gpui_component::theme::ActiveTheme;
use std::sync::Arc;

#[derive(PartialEq, Clone, Copy)]
pub enum Page {
    Login,
    Home,
    Dashboard,
    SimpleDashboard, // New PaneGrid implementation
    Feeds,
    Superchat,
    Notification,
    Settings,
    Profile,
}

/// Type of secondary sidebar to show
#[derive(PartialEq, Clone, Copy)]
pub enum SecondarySidebar {
    Dashboard,
    Feeds,
    Superchat,
    Notifications,
    Settings,
}

/// Pending dialog actions (since subscribe doesn't have Window access)
enum PendingDialogAction {
    OpenDeleteDashboard {
        module_id: String,
        module_name: String,
    },
}

/// Pending module load (for startup before HomePage is created)
#[derive(Clone)]
struct PendingModuleLoad {
    module_id: String,
    module: Module,
}

/// Payload used to reorder dashboard tabs by drag and drop.
#[derive(Clone, Debug)]
struct DragDashboardTab(String);

struct DetachedDashboard {
    module_id: String,
    title: String,
    home: Entity<HomePage>,
    main_view: Option<gpui::WeakEntity<MainView>>,
}

impl Render for DetachedDashboard {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let main_view = self.main_view.clone();
        let module_id = self.module_id.clone();
        let attach_btn = gpui_component::button::Button::new("attach-tab-btn")
            .icon(gpui_component::Icon::default().path("icons/panel-left.svg"))
            .xsmall()
            .ghost()
            .tooltip("Attach to Main Window")
            .on_click(move |_event, window, app| {
                if let Some(main) = main_view.as_ref().and_then(|w| w.upgrade()) {
                    let id = module_id.clone();
                    app.update_entity(&main, |view, cx| {
                        view.closed_dashboard_tabs.remove(&id);
                        view.dashboard_views.remove(&id);
                        // The detached view is a separate GPUI entity; restore its latest
                        // payload into the main window when reattaching.
                        view.load_and_apply_module(id, cx);
                    });
                }
                window.remove_window();
            });

        div()
            .id("detached-dashboard-root")
            .size_full()
            .flex()
            .flex_col()
            .bg(colors.background)
            .child(FinTerTitleBar::new(self.title.clone()).child_before_title(attach_btn))
            .child(div().flex_1().overflow_hidden().child(self.home.clone()))
    }
}

struct TabDragPreview(String);
impl Render for TabDragPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().w(px(160.0)).p(px(10.0)).bg(cx.theme().colors.background)
            .text_xs().child(self.0.clone())
    }
}

pub struct MainView {
    current_page: Page,
    /// Active secondary sidebar (if any)
    active_secondary_sidebar: Option<SecondarySidebar>,
    dashboard_page: Option<Entity<DashboardPage>>,
    login_page: Option<Entity<LoginPage>>,
    home_page: Option<Entity<HomePage>>,
    /// Live dashboard views keyed by module id. Keeping these entities alive prevents
    /// panels and their network subscriptions from being recreated on every tab switch.
    dashboard_views: std::collections::HashMap<String, Entity<HomePage>>,
    settings_page: Option<Entity<SettingsPage>>,
    dashboard_manager: Option<Entity<crate::components::ui::dashboard_manager::DashboardManager>>,
    api_client: ApiClient,
    is_authenticated: bool,
    /// Redacted account snapshot for the native Profile page.
    account_summary: Option<serde_json::Value>,
    account_summary_loading: bool,
    account_refresh_task: Option<Task<()>>,
    /// Periodically republishes live panel data to every Agent pane.
    agent_context_task: Option<Task<()>>,
    /// True while the background session-restore task is running.
    is_restoring: bool,
    /// User settings from database
    user_settings: Option<SettingsData>,
    /// Currently active cloud module (if any)
    active_module: Option<Module>,
    /// Pending dialog action to execute in render phase
    pending_dialog_action: Option<PendingDialogAction>,
    /// Pending module load to execute after HomePage is created
    pending_module_load: Option<PendingModuleLoad>,
    /// Dashboard list retained until the lazy secondary sidebar exists.
    dashboard_modules: Vec<Module>,
    /// Layout presets retained until the lazy secondary sidebar exists.
    layout_presets: Vec<LayoutPreset>,
    /// Cached layout data (module_id -> Module) for instant switching
    layout_cache: std::collections::HashMap<String, Module>,
    /// Dashboards explicitly closed from the title-bar tab strip.
    closed_dashboard_tabs: std::collections::HashSet<String>,
    /// Auto-update manager
    update_manager: Arc<UpdateManager>,
    /// Update notification banner entity
    update_banner: Option<Entity<UpdateBanner>>,
    update_check_started: bool,
    /// Whether the guest "no data" banner has been dismissed for the current session.
    guest_banner_dismissed: bool,
}

impl MainView {
    pub fn new(api_client: ApiClient) -> Self {
        Self {
            current_page: Page::Home,
            active_secondary_sidebar: None,
            dashboard_page: None,
            login_page: None,
            home_page: None,
            dashboard_views: std::collections::HashMap::new(),
            settings_page: None,
            dashboard_manager: None,
            api_client,
            is_authenticated: false,
            account_summary: None,
            account_summary_loading: false,
            account_refresh_task: None,
            agent_context_task: None,
            is_restoring: false,
            user_settings: None,
            active_module: None,
            pending_dialog_action: None,
            pending_module_load: None,
            dashboard_modules: Vec::new(),
            layout_presets: Vec::new(),
            layout_cache: std::collections::HashMap::new(),
            closed_dashboard_tabs: std::collections::HashSet::new(),
            update_manager: Arc::new(UpdateManager::new()),
            update_banner: None,
            update_check_started: false,
            guest_banner_dismissed: false,
        }
    }

    /// Get the path to the guest/offline settings cache file
    fn guest_settings_path() -> Option<std::path::PathBuf> {
        dirs::config_dir().map(|mut p| {
            p.push("finter");
            p.push("guest_settings.json");
            p
        })
    }

    /// Get the path to the authenticated cloud settings cache file
    fn cloud_settings_path() -> Option<std::path::PathBuf> {
        dirs::config_dir().map(|mut p| {
            p.push("finter");
            p.push("cloud_settings.json");
            p
        })
    }

    /// Active settings file based on authentication state
    #[allow(dead_code)]
    fn active_settings_path(&self) -> Option<std::path::PathBuf> {
        if self.is_authenticated {
            Self::cloud_settings_path()
        } else {
            Self::guest_settings_path()
        }
    }

    /// Get the path to the guest/offline layout cache file
    fn guest_layout_path() -> Option<std::path::PathBuf> {
        dirs::config_dir().map(|mut p| {
            p.push("finter");
            p.push("guest_layout.json");
            p
        })
    }

    /// Get the path to the authenticated cloud layout cache file
    fn cloud_layout_path() -> Option<std::path::PathBuf> {
        dirs::config_dir().map(|mut p| {
            p.push("finter");
            p.push("cloud_layout.json");
            p
        })
    }

    /// Active layout file based on authentication state
    fn active_layout_path(&self) -> Option<std::path::PathBuf> {
        if self.is_authenticated {
            Self::cloud_layout_path()
        } else {
            Self::guest_layout_path()
        }
    }

    /// Local tab preferences are kept per guest/cloud profile.
    fn tab_profile(&self) -> crate::data::tab_settings::TabProfile {
        if self.is_authenticated {
            crate::data::tab_settings::TabProfile::Cloud
        } else {
            crate::data::tab_settings::TabProfile::Guest
        }
    }

    /// Reorder dashboards so the tab strip matches the order saved in the
    /// previous session. Dashboards that were not open keep their relative
    /// order after the restored tabs.
    fn reorder_modules_to_match(&mut self, ordered_ids: &[String]) {
        let mut reordered = Vec::with_capacity(self.dashboard_modules.len());
        for id in ordered_ids {
            if let Some(position) = self
                .dashboard_modules
                .iter()
                .position(|module| module.id.as_ref() == Some(id))
            {
                reordered.push(self.dashboard_modules.remove(position));
            }
        }
        reordered.append(&mut self.dashboard_modules);
        self.dashboard_modules = reordered;
    }

    /// Persist the set of open dashboard tabs to the local tab settings file.
    fn persist_open_tabs(&self) {
        let open_ids: Vec<String> = self
            .dashboard_modules
            .iter()
            .filter_map(|module| module.id.clone())
            .filter(|id| !self.closed_dashboard_tabs.contains(id))
            .collect();

        let mut preferences = crate::data::tab_settings::load(self.tab_profile());
        // "Start as empty" should not erase the last restored session; keep it
        // available if the user switches back to "Restore last session".
        if preferences.startup_mode == crate::data::tab_settings::StartupTabMode::Empty
            && open_ids.is_empty()
        {
            return;
        }
        if preferences.open_tab_ids == open_ids {
            return;
        }
        preferences.open_tab_ids = open_ids;
        if let Err(error) = crate::data::tab_settings::save(self.tab_profile(), &preferences) {
            tracing::warn!(?error, "Failed to persist open dashboard tabs");
        }
    }

    /// Apply the local "tabs on startup" preference to the current dashboard
    /// list and return the ids that should be open.
    fn apply_startup_tab_preferences(&mut self) -> Vec<String> {
        let available_ids: Vec<String> = self
            .dashboard_modules
            .iter()
            .filter_map(|module| module.id.clone())
            .collect();
        let preferences = crate::data::tab_settings::load(self.tab_profile());
        let open_ids = preferences.resolve_open_tabs(&available_ids);

        if preferences.startup_mode == crate::data::tab_settings::StartupTabMode::Restore {
            self.reorder_modules_to_match(&open_ids);
        }

        self.closed_dashboard_tabs = available_ids
            .iter()
            .filter(|id| !open_ids.contains(id))
            .cloned()
            .collect();

        self.persist_open_tabs();
        open_ids
    }

    /// Keep the Settings → Tabs dashboard list in sync with the live module
    /// list. The Settings page is created lazily, so this is a no-op until it
    /// exists.
    fn sync_settings_dashboards(&mut self, cx: &mut Context<Self>) {
        let Some(settings_page) = &self.settings_page else {
            return;
        };
        let dashboards = self
            .dashboard_modules
            .iter()
            .filter_map(|module| Some((module.id.clone()?, module.module_name.clone())))
            .collect::<Vec<_>>();
        settings_page.update(cx, |page, cx| page.update_dashboards(dashboards, cx));
    }

    /// Load saved local layout or fallback to a default trading terminal layout for guest users
    fn load_guest_layout(&mut self, cx: &mut Context<Self>) {
        self.guest_banner_dismissed = false;
        // Load guest settings to apply theme/timezone independently of cloud user
        if let Some(path) = Self::guest_settings_path() {
            if let Ok(json) = std::fs::read_to_string(&path) {
                if let Ok(settings) = serde_json::from_str::<crate::data::models::SettingsData>(&json) {
                    let theme_name = settings.theme.clone().unwrap_or_else(|| "gruvbox".to_string());
                    let dark_mode = settings.dark_mode.as_ref().map(|s| s == "dark").unwrap_or(false);
                    apply_theme(&theme_name, dark_mode, cx);
                    self.user_settings = Some(settings);
                    if let Some(home) = &self.home_page {
                        if let Some(s) = &self.user_settings {
                            let s = s.clone();
                            home.update(cx, |page, cx| page.apply_settings(s, cx));
                        }
                    }
                }
            }
        }

        let tree = if let Some(path) = Self::guest_layout_path() {
            if let Ok(content) = std::fs::read_to_string(&path) {
                serde_json::from_str::<PaneNode>(&content).ok()
            } else {
                None
            }
        } else {
            None
        };

        let tree = tree.unwrap_or_else(crate::data::modules::default_workspace);

        if let Ok(tree_value) = serde_json::to_value(&tree) {
            let module = Module {
                id: Some("local-default".to_string()),
                module_name: "Local Workspace".to_string(),
                user_id: None,
                module: tree_value.clone(),
                created_at: None,
                updated_at: None,
            };
            self.dashboard_modules = vec![module.clone()];
            let open_ids = self.apply_startup_tab_preferences();
            let start_empty = open_ids.is_empty();
            if start_empty {
                self.active_module = None;
                self.home_page = None;
                self.pending_module_load = None;
                self.dashboard_views.clear();
            } else {
                self.active_module = Some(module.clone());
            }
            if let Some(dm) = &self.dashboard_manager {
                dm.update(cx, |manager, cx| {
                    manager.update_modules(vec![module.clone()], cx);
                    if start_empty {
                        manager.clear_active_module(cx);
                    } else {
                        manager.set_active_module("local-default".to_string(), cx);
                    }
                });
            }
            self.sync_settings_dashboards(cx);
            if !start_empty {
                if let Some(home) = &self.home_page {
                    home.update(cx, |page, cx| {
                        page.request_module_load(tree_value, "local-default".to_string(), cx);
                    });
                } else {
                    self.pending_module_load = Some(PendingModuleLoad {
                        module_id: "local-default".to_string(),
                        module,
                    });
                }
            }
        }
    }

    /// Kick off a background session restore. Called once, right after creation.
    pub fn start_restore(&mut self, cx: &mut Context<Self>) {
        if self.is_restoring {
            return;
        }
        self.is_restoring = true;

        // Synchronously load settings cache on initial startup
        let has_saved_session = crate::data::session_store::load_session().ok().flatten().is_some();
        let startup_settings_path = if has_saved_session {
            Self::cloud_settings_path().filter(|p| p.exists()).or_else(Self::guest_settings_path)
        } else {
            Self::cloud_settings_path().filter(|p| p.exists()).or_else(Self::guest_settings_path)
        };
        if let Some(path) = startup_settings_path {
            if let Ok(json) = std::fs::read_to_string(&path) {
                if let Ok(settings) =
                    serde_json::from_str::<crate::data::models::SettingsData>(&json)
                {
                    if let Some(server_url) = &settings.server_url {
                        self.api_client.set_base_url(server_url);
                    }
                    let theme_name = settings
                        .theme
                        .clone()
                        .unwrap_or_else(|| "gruvbox".to_string());
                    let dark_mode = settings
                        .dark_mode
                        .as_ref()
                        .map(|s| s == "dark")
                        .unwrap_or(false);
                    tracing::info!(
                        "Loaded initial settings cache from {}. Applying theme: {}",
                        path.display(),
                        theme_name
                    );
                    apply_theme(&theme_name, dark_mode, cx);
                    self.user_settings = Some(settings);
                }
            }
        }

        // Check for crash rollback from a previous failed update
        if let Some(failed_version) = UpdateManager::check_crash_rollback() {
            tracing::warn!(
                "Previous update to v{} may have failed — rollback available",
                failed_version
            );
        }

        let client = self.api_client.clone();
        let rt = crate::data::client::tokio_runtime();
        cx.spawn(async move |this, cx| {
            let restored = rt
                .spawn_blocking(move || client.restore_session())
                .await
                .unwrap_or(None);
            _ = this.update(cx, |this, cx| {
                if restored.is_some() {
                    this.is_authenticated = true;
                    this.current_page = Page::Home;
                    let token = this.api_client.access_token();
                    crate::data::gateway_registry::GatewayRegistry::global()
                        .refresh_for_session(token.as_deref());
                    // Load settings first, then modules with current_dashboard for auto-selection
                    this.load_settings_then_modules(cx);
                    this.load_account_summary(cx);
                } else {
                    this.is_authenticated = false;
                    this.account_summary = None;
                    this.account_summary_loading = false;
                    this.is_restoring = false;
                    this.current_page = Page::Home;
                    this.load_guest_layout(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn switch_page(&mut self, page: Page) {
        self.current_page = page;
    }

    fn start_account_refresh(&mut self, cx: &mut Context<Self>) {
        if self.account_refresh_task.is_some() {
            return;
        }
        self.account_refresh_task = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(std::time::Duration::from_secs(15)).await;
                if this.update(cx, |view, cx| {
                    if view.current_page != Page::Profile || !view.is_authenticated {
                        return false;
                    }
                    view.load_account_summary(cx);
                    true
                }).unwrap_or(false) == false {
                    break;
                }
            }
            let _ = this.update(cx, |view, _| view.account_refresh_task = None);
        }));
    }

    fn load_account_summary(&mut self, cx: &mut Context<Self>) {
        if !self.is_authenticated || self.account_summary_loading {
            return;
        }
        self.account_summary_loading = true;
        let client = self.api_client.clone();
        let rt = crate::data::client::tokio_runtime();
        cx.spawn(async move |this, cx| {
            let result = rt.spawn_blocking(move || client.get_account_summary()).await;
            let _ = this.update(cx, |view, cx| {
                view.account_summary_loading = false;
                match result {
                    Ok(Ok(summary)) => {
                        let balance = summary.pointer("/credits/balance")
                            .and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
                            .unwrap_or(0);
                        let free_remaining = summary.pointer("/credits/monthly_free_remaining")
                            .and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
                            .unwrap_or(0);
                        let base_url = view.api_client.base_url();
                        if balance <= 0 && free_remaining <= 0 {
                            crate::data::credits::CreditStore::record(
                                "Account",
                                "Credit balance exhausted. Add credits or upgrade your plan to resume market data.",
                                balance,
                                1,
                                Some(&base_url),
                            );
                        } else if balance > 0 || free_remaining > 0 {
                            crate::data::credits::CreditStore::clear();
                        }
                        view.account_summary = Some(summary);
                    }
                    Ok(Err(error)) => tracing::warn!("profile summary unavailable: {error}"),
                    Err(error) => tracing::warn!("profile summary task failed: {error}"),
                }
                cx.notify();
            });
        }).detach();
    }

    /// Return to home page from Settings/Profile
    pub fn return_to_home(&mut self) {
        self.current_page = Page::Home;
    }

    /// Toggle a secondary sidebar (close if already open)
    pub fn toggle_secondary_sidebar(&mut self, sidebar: SecondarySidebar, cx: &mut Context<Self>) {
        if self.active_secondary_sidebar == Some(sidebar) {
            self.active_secondary_sidebar = None;
        } else {
            self.active_secondary_sidebar = Some(sidebar);

            // If we are opening the dashboard sidebar and authenticated, refresh modules and presets in the background
            if sidebar == SecondarySidebar::Dashboard && self.is_authenticated {
                let client = self.api_client.clone();
                let rt = crate::data::client::tokio_runtime();
                cx.spawn(async move |this, cx| {
                    let (modules_res, presets_res) = rt
                        .spawn_blocking(move || {
                            (client.list_modules(), client.list_layout_presets())
                        })
                        .await
                        .unwrap_or((
                            Err(crate::data::error::CoinWatchError::Io("Task failed".into())),
                            Err(crate::data::error::CoinWatchError::Io("Task failed".into())),
                        ));

                    let _ = this.update(cx, |view, cx| {
                        match modules_res {
                            Ok(modules) => {
                                view.dashboard_modules = modules.clone();
                                if let Some(dm) = &view.dashboard_manager {
                                    dm.update(cx, |manager, cx| {
                                        manager.update_modules(modules, cx);
                                    });
                                }
                            }
                            Err(e) => tracing::error!("Failed to fetch modules: {:?}", e),
                        }
                        match presets_res {
                            Ok(presets) => {
                                view.layout_presets = presets.clone();
                                if let Some(dm) = &view.dashboard_manager {
                                    dm.update(cx, |manager, cx| {
                                        manager.update_layout_presets(presets, cx);
                                    });
                                }
                            }
                            Err(e) => tracing::error!("Failed to fetch presets: {:?}", e),
                        }
                    });
                })
                .detach();
            }
        }
    }

    fn start_agent_context_refresh(&mut self, cx: &mut Context<Self>) {
        if self.agent_context_task.is_some() { return; }
        self.agent_context_task = Some(cx.spawn(async move |this, cx| {
            let mut last_credit_rev = crate::data::credits::CreditStore::revision();
            loop {
                cx.background_executor().timer(std::time::Duration::from_millis(500)).await;
                let credit_rev = crate::data::credits::CreditStore::revision();
                let credit_changed = credit_rev != last_credit_rev;
                last_credit_rev = credit_rev;

                if this.update(cx, |view, cx| {
                    if credit_changed {
                        cx.notify();
                    }
                    view.refresh_agent_context(cx);
                }).is_err() {
                    break;
                }
            }
        }));
    }

    /// Share a bounded, read-only snapshot of all loaded/open dashboard layouts.
    fn refresh_agent_context(&self, cx: &mut Context<Self>) {
        let mut homes = self.dashboard_views.clone();
        if let (Some(home), Some(active_id)) = (
            &self.home_page,
            self.active_module
                .as_ref()
                .and_then(|module| module.id.clone()),
        ) {
            homes.entry(active_id).or_insert_with(|| home.clone());
        }
        let dashboards: Vec<serde_json::Value> = homes.iter()
            .filter(|(id, _)| !self.closed_dashboard_tabs.contains(*id))
            .map(|(id, home)| {
                let grid = home.read(cx).pane_grid.read(cx);
                let name = self.dashboard_modules.iter().find(|m| m.id.as_ref() == Some(id))
                    .map(|m| m.module_name.as_str()).unwrap_or("Dashboard");
                serde_json::json!({"id": id, "name": name, "layout": grid.tree, "market_data": grid.agent_market_snapshot(cx)})
            }).collect();
        let context = serde_json::json!({"captured_at": chrono::Utc::now().to_rfc3339(), "dashboards": dashboards});
        for home in homes.values() {
            let grid = home.read(cx).pane_grid.clone();
            grid.update(cx, |grid, cx| grid.set_agent_context(context.clone(), cx));
        }
    }

    pub fn api_client(&self) -> &ApiClient {
        &self.api_client
    }

    /// Get the title for the title bar (current layout name or default)
    pub fn title_bar_title(&self) -> String {
        self.active_module
            .as_ref()
            .map(|m| m.module_name.clone())
            .unwrap_or_else(|| "FinTer".to_string())
    }

    /// Build the editor-style dashboard tabs shown in the title bar.
    fn dashboard_tabs(&self, entity: gpui::WeakEntity<Self>, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let list_hover = colors.list_hover;
        let primary = colors.primary;
        let title_bar_border = colors.title_bar_border;
        let bg_active = colors.background;
        let bg_tab = colors.title_bar;
        let fg_active = colors.foreground;
        let fg_muted = colors.muted_foreground;
        let sec_hover = colors.secondary_hover;

        let active_id = self.active_module.as_ref().and_then(|module| module.id.clone());
        let tabs = self.dashboard_modules.iter().filter(|module| {
            module.id.as_ref().is_some_and(|id| !self.closed_dashboard_tabs.contains(id))
        }).cloned().collect::<Vec<_>>();

        let plus_entity = entity.clone();

        div()
            .id("dashboard-tabs")
            .flex()
            .items_end()
            .h_full()
            .gap(px(0.0))
            .overflow_hidden()
            .children(tabs.into_iter().filter_map(|module| {
                let id = module.id?;
                let name = module.module_name;
                let drag_name = name.clone();
                let is_active = active_id.as_ref() == Some(&id);
                let load_id = id.clone();
                let close_id = id.clone();
                let load_entity = entity.clone();
                let middle_close_entity = entity.clone();
                let middle_close_id = id.clone();
                let close_entity = entity.clone();
                let detach_entity = entity.clone();
                let detach_id = id.clone();
                Some(div()
                    .id(gpui::ElementId::Name(gpui::SharedString::from(format!("dashboard-tab-{id}"))))
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .h_full()
                    .w(px(160.0))
                    .flex_shrink_0()
                    .px(px(10.0))
                    .border_b_2()
                    .border_color(if is_active { primary } else { title_bar_border })
                    .bg(if is_active { bg_active } else { bg_tab })
                    .text_color(if is_active { fg_active } else { fg_muted })
                    .cursor_pointer()
                    .hover(move |style| {
                        if !is_active {
                            style.bg(list_hover)
                        } else {
                            style
                        }
                    })
                    .drag_over::<DragDashboardTab>(move |style, _, _, _| style.bg(list_hover))
                    .on_drop({
                        let target_id = id.clone();
                        let entity = entity.clone();
                        move |drag: &DragDashboardTab, _window, app| {
                            if let Some(view) = entity.upgrade() {
                                let dragged_id = drag.0.clone();
                                let target_id = target_id.clone();
                                app.update_entity(&view, |view, cx| {
                                    view.reorder_dashboard_tab(dragged_id, target_id, cx);
                                });
                            }
                        }
                    })
                    .on_drag(
                        DragDashboardTab(id.clone()),
                        {
                            let name = drag_name.clone();
                            move |_drag, _offset, _window, app| {
                                let name = name.clone();
                                app.new(|_| TabDragPreview(name))
                            }
                        },
                    )
                    .on_mouse_down(gpui::MouseButton::Middle, move |_event, _window, app| {
                        if let Some(view) = middle_close_entity.upgrade() {
                            app.update_entity(&view, |view, cx| {
                                view.close_dashboard_tab(middle_close_id.clone(), cx);
                            });
                        }
                    })
                    .on_click(move |_event, _window, app| {
                        if let Some(view) = load_entity.upgrade() {
                            app.update_entity(&view, |view, cx| {
                                view.closed_dashboard_tabs.remove(&load_id);
                                view.load_and_apply_module(load_id.clone(), cx);
                                cx.notify();
                            });
                        }
                    })
                    .child(
                        gpui_component::Icon::default()
                            .path("icons/layout-dashboard.svg")
                            .xsmall()
                            .text_color(if is_active { primary } else { fg_muted }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_xs()
                            .text_ellipsis()
                            .font_weight(if is_active { gpui::FontWeight::MEDIUM } else { gpui::FontWeight::NORMAL })
                            .child(name),
                    )
                    .child(
                        div()
                            .id(gpui::ElementId::Name(gpui::SharedString::from(format!("detach-dashboard-tab-{detach_id}"))))
                            .w(px(18.0))
                            .h(px(18.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .hover(move |style| style.bg(sec_hover).text_color(fg_active))
                            .text_color(fg_muted)
                            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(move |_event, _window, app| {
                                if let Some(view) = detach_entity.upgrade() {
                                    app.update_entity(&view, |view, cx| {
                                        view.detach_dashboard(detach_id.clone(), cx);
                                    });
                                }
                            })
                            .child(gpui_component::Icon::default().path("icons/external-link.svg").xsmall()),
                    )
                    .child(
                        div()
                            .id(gpui::ElementId::Name(gpui::SharedString::from(format!("close-dashboard-tab-{close_id}"))))
                            .w(px(18.0))
                            .h(px(18.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .hover(move |style| style.bg(sec_hover).text_color(fg_active))
                            .text_color(fg_muted)
                            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| {
                                cx.stop_propagation();
                            })
                            .on_click(move |_event, _window, app| {
                                if let Some(view) = close_entity.upgrade() {
                                    app.update_entity(&view, |view, cx| {
                                        view.close_dashboard_tab(close_id.clone(), cx);
                                    });
                                }
                            })
                            .child(gpui_component::Icon::default().path("icons/window-close.svg").xsmall()),
                    ))
            }))
            .child(
                div()
                    .id("add-dashboard-tab-btn")
                    .flex()
                    .items_center()
                    .justify_center()
                    .h_full()
                    .w(px(28.0))
                    .flex_shrink_0()
                    .cursor_pointer()
                    .text_color(fg_muted)
                    .hover(move |style| style.bg(list_hover).text_color(fg_active))
                    .on_click(move |_event, _window, app| {
                        if let Some(view) = plus_entity.upgrade() {
                            app.update_entity(&view, |view, cx| {
                                view.create_new_dashboard("Untitled Dashboard".to_string(), cx);
                            });
                        }
                    })
                    .child(gpui_component::Icon::default().path("icons/plus.svg").xsmall()),
            )
    }

    /// Open a detached dashboard window. The dashboard is loaded from the cached
    /// module payload; it is not removed from the main tab strip.
    fn detach_dashboard(&mut self, module_id: String, cx: &mut Context<Self>) {
        let Some(module) = self.dashboard_modules.iter().find(|m| m.id.as_ref() == Some(&module_id)).cloned() else {
            return;
        };
        let title = module.module_name.clone();
        let entity = cx.weak_entity();
        // Detached windows get their own HomePage entity. Reusing the entity rendered
        // by the main Root makes GPUI route input to the original window.
        let target_module_id = module_id.clone();
        let module_payload = module.module.clone();
        let _ = cx.spawn(async move |this, cx| {
            let window_module_id = target_module_id.clone();
            let result = cx.open_window(
                gpui::WindowOptions {
                    #[cfg(target_os = "linux")]
                    titlebar: None,
                    #[cfg(not(target_os = "linux"))]
                    titlebar: Some(gpui_component::TitleBar::title_bar_options()),
                    window_decorations: Some(gpui::WindowDecorations::Client),
                    app_id: Some("finter-dashboard".to_string()),
                    ..Default::default()
                },
                move |window, cx| {
                    let module_payload = module_payload.clone();
                    let module_id = window_module_id.clone();
                    let home = cx.new(|cx| {
                        let mut home = HomePage::new(cx);
                        home.request_module_load(module_payload, module_id, cx);
                        home
                    });
                    let main_for_save = entity.clone();
                    let module_for_save = window_module_id.clone();
                    cx.subscribe(&home, move |home, event: &HomePageEvent, cx| {
                        if matches!(event, HomePageEvent::LayoutModified) {
                            let tree = home.read(cx).pane_grid.read(cx).tree.clone();
                            if let (Some(main), Some(tree)) = (main_for_save.upgrade(), tree) {
                                let module_id = module_for_save.clone();
                                cx.update_entity(&main, |main, cx| {
                                    main.save_detached_layout(module_id, tree, cx);
                                });
                            }
                        }
                    }).detach();
                    cx.new(|cx| {
                        let detached = cx.new(|_| DetachedDashboard {
                            module_id: window_module_id,
                            title: title.clone(),
                            home,
                            main_view: Some(entity.clone()),
                        });
                        gpui_component::Root::new(detached, window, cx)
                    })
                },
            );
            if result.is_ok() {
                let _ = this.update(cx, |this, cx| {
                    this.close_dashboard_tab(target_module_id, cx);
                });
            }
        }).detach();
    }

    /// Move a dragged dashboard tab immediately before the target tab.
    fn reorder_dashboard_tab(&mut self, dragged_id: String, target_id: String, cx: &mut Context<Self>) {
        if dragged_id == target_id {
            return;
        }
        let Some(from) = self.dashboard_modules.iter().position(|module| module.id.as_ref() == Some(&dragged_id)) else {
            return;
        };
        let module = self.dashboard_modules.remove(from);
        let target = self.dashboard_modules.iter().position(|item| item.id.as_ref() == Some(&target_id))
            .unwrap_or(self.dashboard_modules.len());
        self.dashboard_modules.insert(target, module);
        if let Some(manager) = &self.dashboard_manager {
            let modules = self.dashboard_modules.clone();
            manager.update(cx, |manager, cx| manager.update_modules(modules, cx));
        }
        self.persist_open_tabs();
        cx.notify();
    }

    /// Close a title-bar tab without deleting the saved dashboard.
    fn close_dashboard_tab(&mut self, module_id: String, cx: &mut Context<Self>) {
        self.closed_dashboard_tabs.insert(module_id.clone());
        self.dashboard_views.remove(&module_id);
        if self.active_module.as_ref().and_then(|module| module.id.as_ref()) == Some(&module_id) {
            if let Some(next) = self.dashboard_modules.iter().find(|module| {
                module.id.as_ref().is_some_and(|id| !self.closed_dashboard_tabs.contains(id))
            }).and_then(|module| module.id.clone()) {
                self.load_and_apply_module(next, cx);
            } else {
                self.active_module = None;
            }
        }
        self.persist_open_tabs();
        cx.notify();
    }

    /// Handle successful login: switch to home with trading view
    fn on_logged_in(&mut self, cx: &mut Context<Self>) {
        self.is_authenticated = true;
        self.current_page = Page::Home;
        self.is_restoring = true;
        // Rebuild the settings page with the cloud account's preferences.
        self.settings_page = None;

        let token = self.api_client.access_token();
        crate::data::gateway_registry::GatewayRegistry::global()
            .refresh_for_session(token.as_deref());

        // Load settings first, then modules with current_dashboard for auto-selection
        self.load_settings_then_modules(cx);
        self.load_account_summary(cx);

        cx.notify();
    }

    /// Handle successful signup: create default module and switch to home
    fn on_signed_up(&mut self, cx: &mut Context<Self>) {
        self.is_authenticated = true;
        self.current_page = Page::Home;
        self.is_restoring = true;
        // Rebuild the settings page with the new account's preferences.
        self.settings_page = None;

        let token = self.api_client.access_token();
        crate::data::gateway_registry::GatewayRegistry::global()
            .refresh_for_session(token.as_deref());

        // Create default module for new user, then load modules
        self.create_default_module_then_load(cx);

        cx.notify();
    }

    /// Create default module for new user, then load all modules.
    fn create_default_module_then_load(&mut self, cx: &mut Context<Self>) {
        let client = self.api_client.clone();
        let rt = crate::data::client::tokio_runtime();

        cx.spawn(async move |this, cx| {
            let modules_res = rt
                .spawn_blocking(move || {
                    let _ = client.create_default_module();
                    client.list_modules()
                })
                .await
                .unwrap_or(Err(crate::data::error::CoinWatchError::Io(
                    "Task failed".into(),
                )));

            if let Ok(modules) = modules_res {
                let _ = this.update(cx, |view, cx| {
                    view.dashboard_modules = modules.clone();
                    let open_ids = view.apply_startup_tab_preferences();
                    let start_empty = open_ids.is_empty();

                    if let Some(dm) = &view.dashboard_manager {
                        dm.update(cx, |manager, cx| {
                            manager.update_modules(view.dashboard_modules.clone(), cx);
                            if start_empty {
                                manager.clear_active_module(cx);
                            }
                        });
                    }
                    view.sync_settings_dashboards(cx);

                    if start_empty {
                        view.active_module = None;
                        view.home_page = None;
                        view.dashboard_views.clear();
                    } else {
                        let target = modules
                            .iter()
                            .find(|module| {
                                module.module_name == crate::data::modules::DEFAULT_MODULE_NAME
                                    && module
                                        .id
                                        .as_ref()
                                        .is_some_and(|id| open_ids.contains(id))
                            })
                            .or_else(|| {
                                modules.iter().find(|module| {
                                    module
                                        .id
                                        .as_ref()
                                        .is_some_and(|id| open_ids.contains(id))
                                })
                            });

                        if let Some(module) = target
                            && let Some(module_id) = &module.id
                        {
                            if let Some(dm) = &view.dashboard_manager {
                                dm.update(cx, |manager, cx| {
                                    manager.set_active_module(module_id.clone(), cx);
                                });
                            }
                            view.load_and_apply_module(module_id.clone(), cx);
                        }
                    }

                    view.is_restoring = false;
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// Create a new dashboard with the given name
    fn create_new_dashboard(&mut self, name: String, cx: &mut Context<Self>) {
        if !self.is_authenticated {
            let new_id = uuid::Uuid::new_v4().to_string();
            let new_module = Module {
                id: Some(new_id.clone()),
                module_name: name.clone(),
                user_id: None,
                module: serde_json::json!({
                    "kind": "pane",
                    "id": PaneNode::generate_id(),
                    "data": {
                        "type": "Empty",
                        "config": {}
                    }
                }),
                created_at: None,
                updated_at: None,
            };
            self.dashboard_modules.push(new_module.clone());
            if let Some(dm) = &self.dashboard_manager {
                let modules = self.dashboard_modules.clone();
                dm.update(cx, |manager, cx| {
                    manager.update_modules(modules, cx);
                    manager.set_active_module(new_id.clone(), cx);
                });
            }
            self.load_and_apply_module(new_id, cx);
            self.sync_settings_dashboards(cx);
            cx.notify();
            return;
        }

        let client = self.api_client.clone();

        let rt = crate::data::client::tokio_runtime();
        cx.spawn(async move |this, cx| {
            let name_clone = name.clone();
            let (create_res, modules_res) = rt
                .spawn_blocking(move || {
                    let m = client.create_empty_layout(&name_clone);
                    (m, client.list_modules())
                })
                .await
                .unwrap_or((
                    Err(crate::data::error::CoinWatchError::Io("Task failed".into())),
                    Err(crate::data::error::CoinWatchError::Io("Task failed".into())),
                ));

            if let Ok(module) = create_res {
                tracing::info!("Created new dashboard: {}", name);

                if let Ok(modules) = modules_res {
                    let _ = this.update(cx, |view, cx| {
                        view.dashboard_modules = modules.clone();
                        // Update dashboard manager
                        if let Some(dm) = &view.dashboard_manager {
                            dm.update(cx, |manager, cx| {
                                manager.update_modules(modules, cx);
                            });
                        }

                        // Select the newly created module
                        if let Some(module_id) = &module.id {
                            if let Some(dm) = &view.dashboard_manager {
                                dm.update(cx, |manager, cx| {
                                    manager.set_active_module(module_id.clone(), cx);
                                });
                            }
                            view.load_and_apply_module(module_id.clone(), cx);
                        }

                        view.sync_settings_dashboards(cx);
                        cx.notify();
                    });
                }
            } else {
                tracing::warn!("Failed to create new dashboard");
            }
        })
        .detach();
    }

    /// Delete a layout by module ID
    fn delete_dashboard(&mut self, module_id: String, cx: &mut Context<Self>) {
        self.dashboard_views.remove(&module_id);
        self.closed_dashboard_tabs.remove(&module_id);
        if !self.is_authenticated {
            let is_active = self.active_module.as_ref().and_then(|m| m.id.as_ref()) == Some(&module_id);
            self.dashboard_modules.retain(|m| m.id.as_ref() != Some(&module_id));
            if let Some(dm) = &self.dashboard_manager {
                let modules = self.dashboard_modules.clone();
                dm.update(cx, |manager, cx| {
                    manager.update_modules(modules, cx);
                });
            }
            if is_active {
                self.active_module = None;
                if let Some(first_module) = self.dashboard_modules.first() {
                    if let Some(first_id) = &first_module.id {
                        let id_clone = first_id.clone();
                        if let Some(dm) = &self.dashboard_manager {
                            dm.update(cx, |manager, cx| {
                                manager.set_active_module(id_clone.clone(), cx);
                            });
                        }
                        self.load_and_apply_module(id_clone, cx);
                    }
                } else if let Some(home) = &self.home_page {
                    home.update(cx, |page, cx| {
                        page.request_module_load(serde_json::json!({}), String::new(), cx);
                    });
                }
            }
            self.persist_open_tabs();
            self.sync_settings_dashboards(cx);
            cx.notify();
            return;
        }

        let client = self.api_client.clone();
        let is_active = self.active_module.as_ref().and_then(|m| m.id.as_ref()) == Some(&module_id);
        let rt = crate::data::client::tokio_runtime();

        cx.spawn(async move |this, cx| {
            let modules_res = rt
                .spawn_blocking(move || {
                    let _ = client.delete_module(&module_id);
                    client.list_modules()
                })
                .await
                .unwrap_or(Err(crate::data::error::CoinWatchError::Io(
                    "Task failed".into(),
                )));

            if let Ok(modules) = modules_res {
                let _ = this.update(cx, |view, cx| {
                    view.dashboard_modules = modules.clone();
                    if let Some(dm) = &view.dashboard_manager {
                        dm.update(cx, |manager, cx| {
                            manager.update_modules(modules.clone(), cx);
                        });
                    }

                    if is_active {
                        view.active_module = None;
                        if let Some(first_module) = modules.first() {
                            if let Some(first_id) = &first_module.id {
                                if let Some(dm) = &view.dashboard_manager {
                                    dm.update(cx, |manager, cx| {
                                        manager.set_active_module(first_id.clone(), cx);
                                    });
                                }
                                view.load_and_apply_module(first_id.clone(), cx);
                            }
                        } else {
                            // No dashboards left, clear current_dashboard
                            if let Some(mut current_settings) = view.user_settings.clone() {
                                current_settings.current_dashboard = None;
                                view.user_settings = Some(current_settings.clone());
                                view.save_settings_to_api(current_settings, cx);
                            }
                            
                            if let Some(home) = &view.home_page {
                                home.update(cx, |page, cx| {
                                    page.request_module_load(
                                        serde_json::json!({}),
                                        String::new(),
                                        cx,
                                    );
                                });
                            }
                        }
                    }

                    view.persist_open_tabs();
                    view.sync_settings_dashboards(cx);
                });
            }
        })
        .detach();
    }
    fn load_settings_then_modules(&mut self, cx: &mut Context<Self>) {
        let client = self.api_client.clone();
        let rt = crate::data::client::tokio_runtime();
        // Keep the locally cached selection so it can seed the server value
        // when the API has no current dashboard yet (or is unreachable).
        let local_dashboard = self
            .user_settings
            .as_ref()
            .and_then(|settings| settings.current_dashboard.clone());

        cx.spawn(async move |this, cx| {
            let (settings_res, presets_res, modules_res) = rt
                .spawn_blocking(move || {
                    (
                        client.get_settings(),
                        client.list_layout_presets(),
                        client.list_modules(),
                    )
                })
                .await
                .unwrap_or((
                    Err(crate::data::error::CoinWatchError::Io("Task failed".into())),
                    Err(crate::data::error::CoinWatchError::Io("Task failed".into())),
                    Err(crate::data::error::CoinWatchError::Io("Task failed".into())),
                ));

            // Step 1: Fetch settings from API to get current_dashboard
            tracing::info!("=== LOADING SETTINGS FROM API ===");
            let mut current_dashboard = match settings_res {
                Ok(Some(user_settings)) => {
                    let dashboard = user_settings.settings.current_dashboard.clone();
                    tracing::info!(
                        "✓ Settings loaded successfully. currentDashboard = {:?}",
                        dashboard
                    );

                    // Update cloud cache
                    if let Some(path) = Self::cloud_settings_path() {
                        if let Some(parent) = path.parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        if let Ok(json) = serde_json::to_string(&user_settings.settings) {
                            let _ = std::fs::write(&path, json);
                        }
                    }

                    // Store settings in MainView and apply theme
                    let settings_clone = user_settings.settings.clone();
                    let _ = this.update(cx, |view, cx| {
                        if let Some(server_url) = &settings_clone.server_url {
                            view.api_client.set_base_url(server_url);
                        }
                        // Apply theme from loaded settings
                        let theme_name = settings_clone
                            .theme
                            .clone()
                            .unwrap_or_else(|| "gruvbox".to_string());
                        let dark_mode = settings_clone
                            .dark_mode
                            .as_ref()
                            .map(|s| s == "dark")
                            .unwrap_or(false);
                        tracing::info!(
                            "Applying theme from settings: theme='{}', dark_mode={}",
                            theme_name,
                            dark_mode
                        );
                        crate::data::theme::apply_theme(&theme_name, dark_mode, cx);

                        view.user_settings = Some(settings_clone.clone());

                        if let Some(home) = &view.home_page {
                            let settings = settings_clone.clone();
                            home.update(cx, |page, cx| {
                                page.apply_settings(settings.clone(), cx);
                            });
                        }

                        cx.notify();
                    });
                    dashboard
                }
                Ok(None) => {
                    tracing::warn!("✗ No user settings found in database, using defaults");
                    None
                }
                Err(e) => {
                    tracing::error!("✗ Failed to load settings from API: {:?}", e);
                    None
                }
            };
            
            // If the API failed to give us a current dashboard, fallback to our local cache!
            let _ = this.update(cx, |view, cx| {
                if current_dashboard.is_none() {
                    current_dashboard = local_dashboard.clone();
                }
                // Done loading everything
                view.is_restoring = false;
                cx.notify();
            });

            // Step 2: Fetch modules and layout presets
            tracing::info!("=== LOADING MODULES FROM API ===");
            let presets = match presets_res {
                Ok(presets) => {
                    tracing::info!("✓ Loaded {} layout presets from API", presets.len());
                    presets
                }
                Err(e) => {
                    tracing::warn!("✗ Failed to load layout presets from API: {:?}", e);
                    Vec::new()
                }
            };

            match modules_res {
                Ok(modules) => {
                    tracing::info!("✓ Loaded {} modules from API", modules.len());
                    for module in &modules {
                        tracing::debug!("  - Module: {} (id: {:?})", module.module_name, module.id);
                    }
                    let _ = this.update(cx, |view, cx| {
                        view.dashboard_modules = modules.clone();
                        view.layout_presets = presets.clone();
                        if let Some(dm) = &view.dashboard_manager {
                            dm.update(cx, |manager, cx| {
                                manager.update_modules(modules.clone(), cx);
                                manager.update_layout_presets(presets.clone(), cx);
                            });
                        }
                        view.apply_modules_with_auto_select(
                            modules,
                            presets,
                            current_dashboard,
                            cx,
                        );
                    });
                }
                Err(e) => {
                    tracing::error!("✗ Failed to load modules from API: {:?}", e);
                }
            }
        })
        .detach();
    }

    /// Apply fetched modules to layout manager and auto-select based on current_dashboard.
    fn apply_modules_with_auto_select(
        &mut self,
        modules: Vec<Module>,
        presets: Vec<LayoutPreset>,
        current_dashboard: Option<String>,
        cx: &mut Context<Self>,
    ) {
        // Preload all layouts into cache for instant switching
        self.layout_cache.clear();
        let mut module_exchanges = std::collections::HashSet::new();
        for module in &modules {
            if let Some(module_id) = &module.id {
                self.layout_cache.insert(module_id.clone(), module.clone());
                tracing::debug!("Preloaded layout: {} ({})", module.module_name, module_id);
            }
            if let Ok(tree) = serde_json::from_value::<crate::data::pane_tree::PaneNode>(module.module.clone()) {
                for ex in tree.collect_exchanges() {
                    module_exchanges.insert(ex);
                }
            } else {
                collect_exchanges_from_json(&module.module, &mut module_exchanges);
            }
        }
        tracing::info!("Preloaded {} layouts into cache", self.layout_cache.len());

        // Prefetch exchange and symbol lists via API for all module exchanges
        let module_exchanges_vec: Vec<String> = module_exchanges.into_iter().collect();
        let token = self.api_client.access_token();
        crate::data::gateway_registry::GatewayRegistry::global()
            .prefetch_for_modules(&module_exchanges_vec, token.as_deref());

        // Keep the live module list in sync with the fetched payload before
        // applying tab preferences (which may reorder the list).
        self.dashboard_modules = modules.clone();

        // Apply the local "tabs on startup" preference. This also restores the
        // previous tab order and computes the closed-tab set.
        let open_ids = self.apply_startup_tab_preferences();

        // Update the dashboard manager with the (possibly reordered) modules
        // and layout presets.
        if let Some(dm) = &self.dashboard_manager {
            dm.update(cx, |manager, cx| {
                manager.update_modules(self.dashboard_modules.clone(), cx);
                manager.update_layout_presets(presets, cx);
            });
        }
        self.sync_settings_dashboards(cx);

        // "Start as empty" intentionally opens no tabs and leaves the
        // workspace blank until the user picks a dashboard.
        let startup_mode = crate::data::tab_settings::load(self.tab_profile()).startup_mode;
        if startup_mode == crate::data::tab_settings::StartupTabMode::Empty {
            self.active_module = None;
            self.home_page = None;
            self.pending_module_load = None;
            self.dashboard_views.clear();
            if let Some(dm) = &self.dashboard_manager {
                dm.update(cx, |manager, cx| manager.clear_active_module(cx));
            }
            cx.notify();
            return;
        }

        // Auto-select the last used module based on current_dashboard name.
        // Only consider tabs that are actually open for this session.
        tracing::info!("=== AUTO-SELECTING DASHBOARD ===");
        let target_module = if let Some(dashboard_name) = &current_dashboard {
            modules.iter().find(|module| {
                module.module_name == *dashboard_name
                    && module.id.as_ref().is_some_and(|id| open_ids.contains(id))
            })
        } else {
            None
        };

        let module_to_load = target_module
            .or_else(|| {
                modules
                    .iter()
                    .find(|module| module.id.as_ref().is_some_and(|id| open_ids.contains(id)))
            })
            .or_else(|| modules.first());

        if let Some(module) = module_to_load {
            if let Some(module_id) = &module.id {
                tracing::info!(
                    "✓ Auto-selecting dashboard: '{}' (id: {})",
                    module.module_name,
                    module_id
                );

                // The active dashboard must always be visible, even if the
                // saved selection filtered it out.
                self.closed_dashboard_tabs.remove(module_id);
                if let Some(dm) = &self.dashboard_manager {
                    dm.update(cx, |manager, cx| {
                        manager.set_active_module(module_id.clone(), cx);
                    });
                }
                self.load_and_apply_module(module_id.clone(), cx);
            }
        } else {
            tracing::warn!("No modules available to auto-select.");
        }

        cx.notify();
    }

    /// Persist a layout edited in a detached window without relying on the
    /// currently selected tab in the main window.
    fn save_detached_layout(
        &mut self,
        module_id: String,
        tree: PaneNode,
        cx: &mut Context<Self>,
    ) {
        let Ok(layout_json) = serde_json::to_value(&tree) else { return; };

        if let Some(module) = self.dashboard_modules.iter_mut().find(|m| m.id.as_ref() == Some(&module_id)) {
            module.module = layout_json.clone();
        }
        if let Some(module) = self.layout_cache.get_mut(&module_id) {
            module.module = layout_json.clone();
        }

        if !self.is_authenticated {
            // Keep detached guest edits on disk as well as in the in-memory list.
            if let Some(mut path) = Self::guest_layout_path() {
                path.pop();
                path.push("dashboards");
                if let Err(error) = std::fs::create_dir_all(&path) {
                    tracing::error!(?error, "Failed to create guest dashboard cache");
                } else {
                    path.push(format!("{module_id}.json"));
                    if let Ok(json) = serde_json::to_string_pretty(&tree) {
                        if let Err(error) = std::fs::write(path, json) {
                            tracing::error!(?error, "Failed to save detached guest dashboard");
                        }
                    }
                }
            }
            return;
        }

        let client = self.api_client.clone();
        cx.spawn(async move |_this, _cx| {
            let id = module_id.clone();
            let result = crate::data::client::tokio_runtime()
                .spawn_blocking(move || client.update_module(&id, None, Some(layout_json)))
                .await;
            match result {
                Ok(Ok(_)) => tracing::debug!("Saved detached dashboard {module_id}"),
                Ok(Err(error)) => tracing::error!(?error, "Failed to save detached dashboard {module_id}"),
                Err(error) => tracing::error!(?error, "Detached dashboard save task failed"),
            }
        }).detach();
    }

    /// Save current settings to the API
    fn save_settings_to_api(&self, settings: SettingsData, cx: &mut Context<Self>) {
        let client = self.api_client.clone();

        tracing::info!(
            "Saving settings to API: currentDashboard = {:?}",
            settings.current_dashboard
        );

        // Always keep a local cache, including when authenticated. This lets settings
        // survive offline startup and keeps local preferences available on this machine.
        for path in [Self::guest_settings_path(), Self::cloud_settings_path()].into_iter().flatten() {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Ok(json) = serde_json::to_string(&settings) {
                if let Err(error) = std::fs::write(&path, json) {
                    tracing::error!(?error, ?path, "Failed to save local settings cache");
                }
            }
        }

        if !self.is_authenticated {
            return;
        }

        let rt = crate::data::client::tokio_runtime();
        cx.spawn(async move |_this, _cx| {
            match rt
                .spawn_blocking(move || client.upsert_settings(settings))
                .await
            {
                Ok(Ok(_)) => tracing::debug!("Settings saved to API"),
                Ok(Err(error)) => tracing::error!(?error, "Failed to save settings to API"),
                Err(error) => tracing::error!(?error, "Settings save task failed"),
            }
        })
        .detach();
    }

    /// Save settings from the settings page and update user_settings
    fn save_settings_from_page(&mut self, cx: &mut Context<Self>) {
        if let Some(settings_page) = &self.settings_page {
            // Get the current settings data from the settings page
            let mut settings_data = settings_page.read(cx).get_settings_data(cx);

            // Tab preferences are local-only; keep them out of SettingsData so
            // they never sync to the backend.
            let (startup_mode, selected_tab_ids) = settings_page.read(cx).tab_preferences(cx);
            let mut tab_preferences = crate::data::tab_settings::load(self.tab_profile());
            let mode_changed = tab_preferences.startup_mode != startup_mode;
            let selection_changed = tab_preferences.selected_tab_ids != selected_tab_ids;
            let should_apply_tabs = mode_changed
                || (startup_mode == crate::data::tab_settings::StartupTabMode::Selected
                    && selection_changed);

            tab_preferences.startup_mode = startup_mode;
            tab_preferences.selected_tab_ids = selected_tab_ids;
            if let Err(error) = crate::data::tab_settings::save(self.tab_profile(), &tab_preferences) {
                tracing::error!(?error, "Failed to save local tab settings");
            }

            tracing::info!(
                "Saving settings from settings page: theme={:?}, dark_mode={:?}",
                settings_data.theme,
                settings_data.dark_mode
            );

            // Apply theme visually immediately
            let theme_name = settings_data
                .theme
                .clone()
                .unwrap_or_else(|| "gruvbox".to_string());
            let dark_mode = settings_data
                .dark_mode
                .as_ref()
                .map(|s| s == "dark")
                .unwrap_or(false);
            crate::data::theme::apply_theme(&theme_name, dark_mode, cx);

            // Update ApiClient with configured server_url
            if let Some(server_url) = &settings_data.server_url {
                self.api_client.set_base_url(server_url);
            }

            // Preserve settings the native settings page does not expose while
            // replacing every field that it manages.
            if let Some(existing) = &self.user_settings {
                settings_data.current_dashboard = existing.current_dashboard.clone();
                settings_data.locale = existing.locale.clone();
                settings_data.survey_status = existing.survey_status.clone();
                settings_data.enable_volume_alerts = existing.enable_volume_alerts;
                settings_data.drawing_tools_enabled = existing.drawing_tools_enabled;
                settings_data.chart_background = existing.chart_background.clone();
                settings_data.enable_order_notifications = existing.enable_order_notifications;
                settings_data.enable_trade_notifications = existing.enable_trade_notifications;
                settings_data.enable_system_notifications = existing.enable_system_notifications;
                settings_data.data_retention_days = existing.data_retention_days;
                settings_data.additional = existing.additional.clone();
            }
            self.user_settings = Some(settings_data.clone());

            if let Some(home) = &self.home_page {
                let settings = settings_data.clone();
                home.update(cx, |page, cx| {
                    page.apply_settings(settings.clone(), cx);
                });
            }

            // Save to API / local storage
            if let Some(settings) = self.user_settings.clone() {
                self.save_settings_to_api(settings, cx);
            }

            if should_apply_tabs {
                let mode = crate::data::tab_settings::load(self.tab_profile()).startup_mode;
                let open_ids = self.apply_startup_tab_preferences();

                if mode == crate::data::tab_settings::StartupTabMode::Empty
                    || open_ids.is_empty()
                {
                    self.active_module = None;
                    self.home_page = None;
                    self.pending_module_load = None;
                    self.dashboard_views.clear();
                    if let Some(manager) = &self.dashboard_manager {
                        manager.update(cx, |manager, cx| manager.clear_active_module(cx));
                    }
                } else {
                    let active_is_open = self
                        .active_module
                        .as_ref()
                        .and_then(|module| module.id.as_ref())
                        .is_some_and(|id| open_ids.contains(id));
                    if !active_is_open
                        && let Some(module_id) = open_ids.first()
                    {
                        self.load_and_apply_module(module_id.clone(), cx);
                    }
                }

                if let Some(manager) = &self.dashboard_manager {
                    let modules = self.dashboard_modules.clone();
                    manager.update(cx, |manager, cx| manager.update_modules(modules, cx));
                }
                self.sync_settings_dashboards(cx);
            }

            cx.notify();
        }
    }

    /// Save the current layout immediately (called on LayoutModified events)
    fn save_current_layout_immediately(&mut self, cx: &mut Context<Self>) {
        if let Some(home) = &self.home_page {
            let tree_opt = home.read(cx).pane_grid.read(cx).tree.clone();
            if let Some(tree) = tree_opt {
                // 1. Save to the active local layout file (guest_layout.json or cloud_layout.json)
                if let Some(path) = self.active_layout_path() {
                    if let Some(parent) = path.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    if let Ok(json_str) = serde_json::to_string_pretty(&tree) {
                        let _ = std::fs::write(&path, json_str);
                    }
                }

                if let Some(module_id) = self.active_module.as_ref().and_then(|m| m.id.clone()) {
                    if let Ok(layout_json) = serde_json::to_value(&tree) {
                        let module_id_clone = module_id.clone();
                        let layout_json_clone = layout_json.clone();

                        // Update the cache immediately with the new layout
                        if let Some(cached_module) = self.layout_cache.get_mut(&module_id) {
                            cached_module.module = layout_json.clone();
                            tracing::info!(
                                "✓ Updated layout cache for module {} (immediate save)",
                                module_id
                            );
                        }

                        if self.is_authenticated {
                            let client = self.api_client.clone();
                            cx.spawn(async move |_this, _cx| {
                                tracing::info!(
                                    "→ Starting API call to update module {}",
                                    module_id_clone
                                );
                                let rt = crate::data::client::tokio_runtime();
                                let m_id = module_id_clone.clone();
                                let result = rt
                                    .spawn_blocking(move || {
                                        client.update_module(&m_id, None, Some(layout_json_clone))
                                    })
                                    .await
                                    .unwrap();

                                match result {
                                    Ok(_) => {
                                        tracing::info!(
                                            "✓ Layout successfully saved to database for module {}",
                                            module_id_clone
                                        );
                                    }
                                    Err(e) => {
                                        tracing::error!("✗ Failed to save layout to database: {:?}", e);
                                    }
                                }
                            })
                            .detach();
                        }
                    }
                }
            }
        }
    }

    /// Handle logout: clear cloud session and return to home in guest mode
    pub fn do_logout(&mut self, cx: &mut Context<Self>) {
        let client = self.api_client.clone();
        let rt = crate::data::client::tokio_runtime();
        cx.spawn(async move |this, cx| {
            let _ = rt.spawn_blocking(move || client.logout()).await;
            _ = this.update(cx, |this, cx| {
                this.is_authenticated = false;
                this.current_page = Page::Home;
                this.dashboard_page = None;
                this.user_settings = None;
                this.active_module = None;
                this.settings_page = None;
                this.load_guest_layout(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Load a module from the cache (instant) or API and apply it to the PaneGrid
    fn load_and_apply_module(&mut self, module_id: String, cx: &mut Context<Self>) {
        tracing::debug!("load_and_apply_module called with id: {}", module_id);

        // Selecting the active tab must not rebuild the pane tree or refresh modules.
        if self.active_module.as_ref().and_then(|module| module.id.as_ref()) == Some(&module_id) {
            return;
        }

        let module_to_load = self
            .dashboard_modules
            .iter()
            .find(|m| m.id.as_ref() == Some(&module_id))
            .cloned();

        if let Some(module) = module_to_load {
            if let (Some(previous_id), Some(home)) = (
                self.active_module.as_ref().and_then(|module| module.id.clone()),
                self.home_page.clone(),
            ) {
                self.dashboard_views.insert(previous_id, home);
            }
            let home = if let Some(home) = self.dashboard_views.get(&module_id) {
                home.clone()
            } else {
                let home = cx.new(HomePage::new);
                cx.subscribe(&home, |this, source, _: &HomePageEvent, cx| {
                    if this.home_page.as_ref() == Some(&source) {
                        this.save_current_layout_immediately(cx);
                    }
                }).detach();
                home.update(cx, |page, cx| {
                    page.request_module_load(module.module.clone(), module_id.clone(), cx);
                    if let Some(settings) = self.user_settings.clone() {
                        page.apply_settings(settings, cx);
                    }
                });
                self.dashboard_views.insert(module_id.clone(), home.clone());
                home
            };
            cx.set_global(crate::components::pages::home::GlobalHomeView { view: home.downgrade() });
            self.home_page = Some(home);
            self.closed_dashboard_tabs.remove(&module_id);
            self.persist_open_tabs();
            self.active_module = Some(module.clone());
            self.refresh_agent_context(cx);
            if let Some(dm) = &self.dashboard_manager {
                dm.update(cx, |manager, cx| manager.set_active_module(module_id.clone(), cx));
            }
            self.current_page = Page::Home;

            // Remember the dashboard the user last worked in so the next
            // launch/login restores it instead of falling back to the first
            // module. Keep this here (not only in the dashboard manager) so
            // every entry point — tab clicks, closing a tab, reattaching a
            // detached window — persists the selection.
            self.remember_current_dashboard(&module.module_name, cx);

            cx.notify();
        } else {
            tracing::warn!(
                "Could not find module ID {} in dashboard_modules",
                module_id
            );
        }
    }

    /// Persist the active dashboard name so it can be restored on the next
    /// launch or login. No-op when the saved selection already matches.
    fn remember_current_dashboard(&mut self, module_name: &str, cx: &mut Context<Self>) {
        let already_current = self
            .user_settings
            .as_ref()
            .and_then(|settings| settings.current_dashboard.as_deref())
            == Some(module_name);

        if already_current {
            return;
        }

        // Without a settings record there is nothing safe to update; the
        // selection will be persisted once settings finish loading.
        let Some(mut settings) = self.user_settings.clone() else {
            return;
        };

        settings.current_dashboard = Some(module_name.to_string());
        self.user_settings = Some(settings.clone());
        self.save_settings_to_api(settings, cx);
    }

    /// Build the secondary sidebar content panel
    fn build_secondary_sidebar(
        &self,
        sidebar_type: SecondarySidebar,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let is_macos = cfg!(target_os = "macos");

        let base = div()
            .w(px(sidebar::get_secondary_sidebar_width()))
            .h_full()
            .flex_shrink_0()
            .when(is_macos, |this| {
                this.border_l_1().border_color(theme.colors.sidebar_border)
            })
            .when(!is_macos, |this| {
                this.border_r_1().border_color(theme.colors.sidebar_border)
            })
            .bg(theme.colors.sidebar);

        base.flex()
            .flex_col()
            .child(
                // Header with title and search (draggable)
                div()
                    .id("secondary-sidebar-header")
                    // GPUI's Linux client-decoration hit-test path is not active;
                    // use the explicit compositor move there. Other platforms use
                    // the native drag region to avoid competing drag handlers.
                    .when(!cfg!(target_os = "linux"), |this| {
                        this.window_control_area(WindowControlArea::Drag)
                    })
                    .when(cfg!(target_os = "linux"), |this| {
                        this.on_mouse_down(gpui::MouseButton::Left, |_, window, _| {
                            window.start_window_move();
                        })
                    })
                    .h(crate::components::ui::title_bar::TITLE_BAR_HEIGHT)
                    .flex()
                    .items_center()
                    .px(px(16.0))
                    .border_b_1()
                    .border_color(theme.colors.border)
                    .when(is_macos, |this| {
                        // On macOS, push down so text doesn't overlap the traffic lights
                        this.pt(px(12.0)).h(px(50.0))
                    })
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_size(px(16.0))
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(theme.colors.foreground)
                                    .child(match sidebar_type {
                                        SecondarySidebar::Dashboard => "Layouts",
                                        SecondarySidebar::Feeds => "Feeds",
                                        SecondarySidebar::Superchat => "Superchat",
                                        SecondarySidebar::Notifications => "Alerts",
                                        SecondarySidebar::Settings => "Settings",
                                    }),
                            ),
                    ),
            )
            .child(
                // Content area
                div()
                    .flex_1()
                    .overflow_y_hidden()
                    .child(self.build_sidebar_content(sidebar_type, cx)),
            )
    }

    /// Build content based on sidebar type, returns AnyElement to satisfy match arms
    fn build_sidebar_content(
        &self,
        sidebar_type: SecondarySidebar,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match sidebar_type {
            SecondarySidebar::Dashboard => {
                if let Some(dm) = &self.dashboard_manager {
                    dm.clone().into_any_element()
                } else {
                    div().into_any_element() // Fallback if not initialized
                }
            }
            SecondarySidebar::Feeds => build_feeds_content(cx).into_any_element(),
            SecondarySidebar::Superchat => build_superchat_content(cx).into_any_element(),
            SecondarySidebar::Notifications => build_notifications_content(cx).into_any_element(),
            SecondarySidebar::Settings => build_settings_content(cx).into_any_element(),
        }
    }
}

impl Default for MainView {
    fn default() -> Self {
        Self::new(ApiClient::new())
    }
}

#[allow(dead_code)]
fn build_dashboard_content(cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    div()
        .flex()
        .flex_col()
        .gap(px(12.0))
        .child(
            div()
                .text_sm()
                .text_color(theme.colors.muted_foreground)
                .child("Manage your trading dashboards"),
        )
        .child(build_content_card(
            "Portfolio Value",
            "$125,430.50",
            true,
            cx,
        ))
        .child(build_content_card("24h Change", "+2.45%", true, cx))
        .child(build_content_card("Open Positions", "3", false, cx))
}

fn build_feeds_content(cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    div()
        .flex()
        .flex_col()
        .gap(px(12.0))
        .child(
            div()
                .text_sm()
                .text_color(theme.colors.muted_foreground)
                .child("Real-time market feeds and updates"),
        )
        .child(build_feed_item(
            "BTC/USDT",
            "Large buy order detected",
            "2 min ago",
            true,
            cx,
        ))
        .child(build_feed_item(
            "ETH/USDT",
            "Price alert triggered",
            "5 min ago",
            false,
            cx,
        ))
        .child(build_feed_item(
            "SOL/USDT",
            "Volume spike detected",
            "12 min ago",
            true,
            cx,
        ))
}

fn build_superchat_content(cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    div()
        .flex()
        .flex_col()
        .gap(px(12.0))
        .child(
            div()
                .text_sm()
                .text_color(theme.colors.muted_foreground)
                .child("Trading community chat"),
        )
        .child(build_chat_message(
            "@trader_pro",
            "Just opened a long position on BTC",
            gpui::rgb(0x6366f1),
            cx,
        ))
        .child(build_chat_message(
            "@crypto_whale",
            "Bullish on ETH this week",
            gpui::rgb(0x22c55e),
            cx,
        ))
        .child(build_chat_message(
            "@degen_master",
            "SOL looking strong",
            gpui::rgb(0xf59e0b),
            cx,
        ))
}

fn build_notifications_content(cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    div()
        .w_full()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(12.0))
        .child(
            div()
                .w_full()
                .min_w_0()
                .text_sm()
                .text_color(theme.colors.muted_foreground)
                .child("Your trading alerts"),
        )
        .child(build_notification_item(
            "Order Filled",
            "BTC/USDT limit order filled at $65,000",
            cx,
        ))
        .child(build_notification_item(
            "Price Alert",
            "ETH/USDT reached target of $3,500",
            cx,
        ))
        .child(build_notification_item(
            "Stop Loss",
            "SOL/USDT stop loss triggered at $140",
            cx,
        ))
}

fn build_settings_content(cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    div()
        .flex()
        .flex_col()
        .gap(px(12.0))
        .child(
            div()
                .text_sm()
                .text_color(theme.colors.muted_foreground)
                .child("Configure your trading preferences"),
        )
        .child(build_settings_item(
            "General",
            "Theme, language, and display",
            cx,
        ))
        .child(build_settings_item(
            "Exchange Keys",
            "Manage API keys for exchanges",
            cx,
        ))
        .child(build_settings_item(
            "Trading",
            "Order defaults and risk management",
            cx,
        ))
        .child(build_settings_item(
            "Notifications",
            "Alert preferences and sounds",
            cx,
        ))
}

/// Helper to build a content card
#[allow(dead_code)]
fn build_content_card(
    title: &'static str,
    value: &'static str,
    is_positive: bool,
    cx: &App,
) -> impl IntoElement {
    let theme = cx.theme();
    let positive_color: gpui::Hsla = gpui::rgb(0x22c55e).into();
    div()
        .p(px(12.0))
        .bg(theme.colors.sidebar_accent)
        .child(
            div()
                .text_sm()
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(theme.colors.muted_foreground)
                .mb(px(4.0))
                .child(title),
        )
        .child(
            div()
                .text_size(px(20.0))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(if is_positive {
                    positive_color
                } else {
                    theme.colors.sidebar_foreground
                })
                .child(value),
        )
}

/// Helper to build a feed item
fn build_feed_item(
    symbol: &'static str,
    message: &'static str,
    time: &'static str,
    is_bullish: bool,
    cx: &App,
) -> impl IntoElement {
    let theme = cx.theme();
    div()
        .p(px(12.0))
        .bg(theme.colors.sidebar_accent)
        .border_l_2()
        .border_color(if is_bullish {
            gpui::rgb(0x22c55e)
        } else {
            gpui::rgb(0xef4444)
        })
        .child(
            div()
                .text_xs()
                .text_color(theme.colors.muted_foreground)
                .mb(px(4.0))
                .child(time),
        )
        .child(
            div()
                .text_sm()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(theme.colors.sidebar_foreground)
                .mb(px(2.0))
                .child(symbol),
        )
        .child(
            div()
                .text_sm()
                .text_color(theme.colors.muted_foreground)
                .child(message),
        )
}

/// Helper to build a chat message
fn build_chat_message(
    username: &'static str,
    message: &'static str,
    name_color: gpui::Rgba,
    cx: &App,
) -> impl IntoElement {
    let theme = cx.theme();
    div()
        .p(px(12.0))
        .bg(theme.colors.sidebar_accent)
        .child(
            div()
                .text_sm()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(name_color)
                .mb(px(4.0))
                .child(username),
        )
        .child(
            div()
                .text_sm()
                .text_color(theme.colors.sidebar_foreground)
                .child(message),
        )
}

/// Helper to build a notification item
fn build_notification_item(
    title: &'static str,
    message: &'static str,
    cx: &App,
) -> impl IntoElement {
    let theme = cx.theme();
    div()
        .w_full()
        .min_w_0()
        .p(px(12.0))
        .rounded(theme.radius)
        .bg(theme.colors.sidebar_accent)
        .child(
            div()
                .w_full()
                .min_w_0()
                .text_sm()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(theme.colors.sidebar_foreground)
                .mb(px(4.0))
                .child(title),
        )
        .child(
            div()
                .w_full()
                .min_w_0()
                .text_sm()
                .text_color(theme.colors.muted_foreground)
                .child(message),
        )
}

/// Helper to build a settings item
fn build_settings_item(
    title: &'static str,
    description: &'static str,
    cx: &App,
) -> impl IntoElement {
    let theme = cx.theme();
    div()
        .p(px(12.0))
        .bg(theme.colors.sidebar_accent)
        .cursor_pointer()
        .hover(|style: StyleRefinement| style.bg(gpui::rgb(0x3a3a4e)))
        .child(
            div()
                .text_sm()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(theme.colors.sidebar_foreground)
                .mb(px(4.0))
                .child(title),
        )
        .child(
            div()
                .text_sm()
                .text_color(theme.colors.muted_foreground)
                .child(description),
        )
}

impl Render for MainView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.start_agent_context_refresh(cx);
        let theme = cx.theme().clone();
        let current_page = self.current_page;
        let entity = cx.weak_entity();
        let active_secondary_sidebar = self.active_secondary_sidebar;

        let is_macos = cfg!(target_os = "macos");
        let sidebar_bg = cx.theme().colors.sidebar;
        let sidebar_fg = cx.theme().colors.sidebar_foreground;
        let user_info = self
            .api_client
            .get_session()
            .map(|session| SidebarUserInfo {
                email: Some(session.email),
                name: session.name,
                avatar_url: session.avatar_url,
            })
            .unwrap_or_default();

        // Delete confirmations still use a modal dialog; creation is immediate in the workspace.
        if let Some(action) = self.pending_dialog_action.take() {
            match action {
                PendingDialogAction::OpenDeleteDashboard {
                    module_id,
                    module_name,
                } => {
                    let dialog_entity =
                        open_delete_dashboard_dialog(module_id.clone(), module_name, window, cx);
                    // Subscribe to dialog events
                    cx.subscribe(
                        &dialog_entity,
                        |this: &mut MainView, _dialog, event: &DeleteDashboardDialogEvent, cx| {
                            match event {
                                DeleteDashboardDialogEvent::Confirm(module_id) => {
                                    this.delete_dashboard(module_id.clone(), cx);
                                }
                                DeleteDashboardDialogEvent::Cancel => {
                                    // Dialog will close itself
                                }
                            }
                        },
                    )
                    .detach();
                }
            }
        }

        // Lazily initialize dashboard manager with data retained by MainView.
        // API loading can finish before this entity first exists.
        if self.dashboard_manager.is_none() {
            let modules = self.dashboard_modules.clone();
            let presets = self.layout_presets.clone();
            let dashboard_manager = cx.new(|cx| {
                let mut manager =
                    crate::components::ui::dashboard_manager::DashboardManager::new(cx);
                manager.update_modules(modules, cx);
                manager.update_layout_presets(presets, cx);
                manager
            });

            cx.subscribe(
                &dashboard_manager,
                |this: &mut MainView, _manager: Entity<crate::components::ui::dashboard_manager::DashboardManager>, event: &crate::components::ui::dashboard_manager::DashboardManagerEvent, cx| {
                    match event {
                        crate::components::ui::dashboard_manager::DashboardManagerEvent::LoadModule(module_id) => {
                            _manager.update(cx, |manager, cx| {
                                manager.set_active_module(module_id.clone(), cx);
                            });
                            this.load_and_apply_module(module_id.clone(), cx);
                        }
                        crate::components::ui::dashboard_manager::DashboardManagerEvent::LoadPreset(tree) => {
                            tracing::info!("LoadPreset requested for layout preset: {:?}", tree);
                        }
                        crate::components::ui::dashboard_manager::DashboardManagerEvent::DeletePreset(preset_id) => {
                            let client = this.api_client.clone();
                            let preset_id = preset_id.clone();
                            let rt = crate::data::client::tokio_runtime();
                            cx.spawn(async move |this, cx| {
                                let presets_res = rt.spawn_blocking(move || {
                                    let _ = client.delete_layout_preset(&preset_id);
                                    client.list_layout_presets()
                                }).await.unwrap_or(Err(crate::data::error::CoinWatchError::Io("Task failed".into())));

                                if let Ok(presets) = presets_res {
                                    let _ = this.update(cx, |view, cx| {
                                        view.layout_presets = presets.clone();
                                        if let Some(manager) = &view.dashboard_manager {
                                            manager.update(cx, |m, cx| {
                                                m.update_layout_presets(presets, cx);
                                            });
                                        }
                                    });
                                }
                            })
                            .detach();
                        }
                        crate::components::ui::dashboard_manager::DashboardManagerEvent::ExportDashboard => {
                            tracing::info!("ExportDashboard called (pane grid removed)");
                        }
                        crate::components::ui::dashboard_manager::DashboardManagerEvent::ImportDashboard(_json_opt) => {
                            tracing::info!("Import dashboard requested - requires Window context");
                        }
                        crate::components::ui::dashboard_manager::DashboardManagerEvent::CreateDashboard(name) => {
                            this.create_new_dashboard(name.clone(), cx);
                        }
                        crate::components::ui::dashboard_manager::DashboardManagerEvent::DeleteDashboard(module_id, _module_name) => {
                            this.delete_dashboard(module_id.clone(), cx);
                        }
                        crate::components::ui::dashboard_manager::DashboardManagerEvent::OpenCreateDialog => {
                            this.create_new_dashboard("Untitled Dashboard".to_string(), cx);
                        }
                        crate::components::ui::dashboard_manager::DashboardManagerEvent::OpenDeleteDialog(module_id, module_name) => {
                            this.pending_dialog_action = Some(PendingDialogAction::OpenDeleteDashboard {
                                module_id: module_id.clone(),
                                module_name: module_name.clone(),
                            });
                            cx.notify();
                        }
                        crate::components::ui::dashboard_manager::DashboardManagerEvent::CloseDashboard(module_id) => {
                            this.close_dashboard_tab(module_id.clone(), cx);
                        }
                        crate::components::ui::dashboard_manager::DashboardManagerEvent::DetachDashboard(module_id) => {
                            this.detach_dashboard(module_id.clone(), cx);
                        }
                    }
                },
            )
            .detach();

            self.dashboard_manager = Some(dashboard_manager);
        }

        // While restoring, show a brief loading indicator
        if self.is_restoring {
            if is_macos {
                return div()
                    .flex()
                    .flex_row()
                    .size_full()
                    .bg(theme.colors.background)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .child(FinTerTitleBar::new(self.title_bar_title()))
                            .child(
                                div().flex_1().flex().items_center().justify_center().child(
                                    div()
                                        .text_xl()
                                        .text_color(sidebar_fg)
                                        .child("Restoring session…"),
                                ),
                            ),
                    )
                    .child(sidebar::build_sidebar_element(
                        entity.clone(),
                        self.current_page,
                        self.active_secondary_sidebar,
                        &user_info,
                    ));
            } else {
                return div()
                    .flex()
                    .flex_row()
                    .size_full()
                    .bg(theme.colors.background)
                    .child(sidebar::build_sidebar_element(
                        entity.clone(),
                        self.current_page,
                        self.active_secondary_sidebar,
                        &user_info,
                    ))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .child(FinTerTitleBar::new(self.title_bar_title()))
                            .child(
                                div().flex_1().flex().items_center().justify_center().child(
                                    div()
                                        .text_xl()
                                        .text_color(sidebar_fg)
                                        .child("Restoring session…"),
                                ),
                            ),
                    );
            }
        }

        // Create login page lazily
        if self.login_page.is_none() {
            let client = self.api_client.clone();
            let login = cx.new(|cx| LoginPage::new(client, cx));
            cx.subscribe(
                &login,
                |this: &mut MainView, _login, event: &AuthEvent, cx| match event {
                    AuthEvent::LoggedIn => this.on_logged_in(cx),
                    AuthEvent::SignedUp => this.on_signed_up(cx),
                    AuthEvent::ContinueAsGuest => {
                        this.current_page = Page::Home;
                        cx.notify();
                    }
                },
            )
            .detach();
            self.login_page = Some(login);
        }

        // If explicitly on Login page, show login page only (with title bar and guest option)
        if current_page == Page::Login {
            return div()
                .flex()
                .flex_row()
                .size_full()
                .bg(theme.colors.background)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .child(FinTerTitleBar::new(self.title_bar_title()))
                        .child(
                            div()
                                .flex_1()
                                .size_full()
                                .child(self.login_page.clone().unwrap()),
                        ),
                );
        }

        // If on Settings or Profile page, show full-screen with sidebar
        if current_page == Page::Settings || current_page == Page::Profile {
            if current_page == Page::Profile && self.is_authenticated {
                if self.account_refresh_task.is_none() {
                    self.load_account_summary(cx);
                    self.start_account_refresh(cx);
                }
            } else {
                self.account_refresh_task = None;
            }
            if self.settings_page.is_none() {
                let settings_data = self.user_settings.clone().unwrap_or_default();
                let settings_entity = entity.clone();
                let tab_profile = self.tab_profile();
                let dashboards = self
                    .dashboard_modules
                    .iter()
                    .filter_map(|module| Some((module.id.clone()?, module.module_name.clone())))
                    .collect::<Vec<_>>();

                let on_setting_changed = Arc::new(move |cx: &mut App| {
                    if let Some(e) = settings_entity.upgrade() {
                        cx.update_entity(&e, |view: &mut MainView, cx: &mut Context<MainView>| {
                            view.save_settings_from_page(cx);
                        });
                    }
                });

                self.settings_page = Some(cx.new(move |cx| {
                    SettingsPage::new_with_data(&settings_data, tab_profile, cx)
                        .with_dashboards(dashboards.clone())
                        .with_on_change(on_setting_changed.clone())
                }));
            }

            let back_button = gpui_component::button::Button::new("back-button")
                .icon(gpui_component::Icon::default().path("icons/arrow-left.svg"))
                .ghost()
                .on_click({
                    let entity = entity.clone();
                    move |_event: &gpui::ClickEvent,
                          _window: &mut gpui::Window,
                          app: &mut gpui::App| {
                        if let Some(e) = entity.upgrade() {
                            app.update_entity(
                                &e,
                                |view: &mut MainView, cx: &mut gpui::Context<MainView>| {
                                    view.return_to_home();
                                    cx.notify();
                                },
                            );
                        }
                    }
                });

            let page_title = if current_page == Page::Settings {
                "Settings"
            } else {
                "Profile"
            };

            let settings_content = div()
                .flex_1()
                .overflow_hidden()
                .when(current_page == Page::Settings, |this| {
                    if let Some(settings_page) = self.settings_page.as_ref() {
                        this.child(settings_page.clone())
                    } else {
                        this
                    }
                })
                .when(current_page == Page::Profile, |this| {
                    this.child(ProfilePage::build_element(
                        self.api_client.get_session(),
                        self.user_settings.clone(),
                        self.account_summary.clone(),
                        self.account_summary_loading,
                        self.is_authenticated,
                        cx,
                    ))
                });

            if is_macos {
                return div()
                    .flex()
                    .flex_row()
                    .size_full()
                    .bg(theme.colors.background)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .bg(sidebar_bg)
                            .child(
                                FinTerTitleBar::new(page_title).child_before_title(back_button),
                            )
                            .child(settings_content),
                    )
                    .child(sidebar::build_sidebar_element(
                        entity.clone(),
                        self.current_page,
                        self.active_secondary_sidebar,
                        &user_info,
                    ));
            } else {
                return div()
                    .flex()
                    .flex_row()
                    .size_full()
                    .bg(theme.colors.background)
                    .child(sidebar::build_sidebar_element(
                        entity.clone(),
                        self.current_page,
                        self.active_secondary_sidebar,
                        &user_info,
                    ))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .bg(sidebar_bg)
                            .child(
                                FinTerTitleBar::new(page_title).child_before_title(back_button),
                            )
                            .child(settings_content),
                    );
            }
        }

        // Create home page lazily (main trading view)
        if self.home_page.is_none() {
            let active_id = self
                .active_module
                .as_ref()
                .and_then(|m| m.id.clone());
            let home_page = cx.new(HomePage::new);

            // Subscribe to HomePage events to handle immediate layout saves
            cx.subscribe(
                &home_page,
                |this: &mut MainView, _home, event: &HomePageEvent, cx| match event {
                    HomePageEvent::LayoutModified => {
                        tracing::debug!("HomePage layout modified, triggering immediate save");
                        this.save_current_layout_immediately(cx);
                    }
                },
            )
            .detach();

            if let Some(module) = &self.active_module {
                if let Some(module_id) = &module.id {
                    let module_clone = module.module.clone();
                    let module_id_clone = module_id.clone();
                    home_page.update(cx, |page, cx| {
                        page.request_module_load(module_clone, module_id_clone, cx);
                    });
                }
            }

            // Only real dashboards belong in the view cache. When starting
            // empty, `home_page` is a temporary blank workspace.
            if let Some(active_id) = active_id {
                self.dashboard_views.insert(active_id, home_page.clone());
            }
            self.home_page = Some(home_page);
        }

        if let (Some(home), Some(settings)) = (&self.home_page, self.user_settings.clone()) {
            home.update(cx, |page, cx| {
                page.apply_settings(settings.clone(), cx);
            });
        }

        // Process pending module load now that HomePage exists
        if let Some(pending) = self.pending_module_load.take() {
            tracing::info!(
                "Processing pending module load: {} ({})",
                pending.module.module_name,
                pending.module_id
            );
            if let Some(home) = &self.home_page {
                home.update(cx, |page, cx| {
                    page.request_module_load(
                        pending.module.module.clone(),
                        pending.module_id.clone(),
                        cx,
                    );
                    tracing::info!("Pending module {} loaded into HomePage", pending.module_id);
                });
            }
        }

        // Create dashboard lazily (if needed for backward compatibility)
        if self.dashboard_page.is_none() {
            self.dashboard_page = Some(cx.new(|_cx| DashboardPage::default()));
        }

        let sidebar_element = sidebar::build_sidebar_element(
            entity.clone(),
            self.current_page,
            self.active_secondary_sidebar,
            &user_info,
        );

        if !self.update_check_started {
            self.update_check_started = true;
            let mgr = self.update_manager.clone();
            cx.spawn(async move |this, cx| {
                let result = crate::data::client::tokio_runtime()
                    .spawn_blocking(move || mgr.check_for_update())
                    .await;
                if let Err(error) = result { tracing::warn!("Update check task failed: {error}"); }
                let _ = this.update(cx, |_this, cx| cx.notify());
            }).detach();
        }

        // Lazily create update banner entity
        if self.update_banner.is_none() {
            let mgr = self.update_manager.clone();
            let banner = cx.new(|_cx| UpdateBanner::new(mgr));

            cx.subscribe(
                &banner,
                |this: &mut MainView, _banner, event: &UpdateBannerEvent, cx| match event {
                    UpdateBannerEvent::DownloadRequested => {
                        let state = this.update_manager.state();
                        if let UpdateState::Available(info) = state {
                            let mgr = this.update_manager.clone();
                            cx.spawn(async move |this, cx| {
                                let mgr_worker = mgr.clone();
                                let info_worker = info.clone();
                                let download_task = crate::data::client::tokio_runtime().spawn(async move {
                                    tokio::task::spawn_blocking(move || mgr_worker.download_update(&info_worker)).await
                                });

                                loop {
                                    cx.background_executor().timer(std::time::Duration::from_millis(100)).await;
                                    let is_downloading = matches!(mgr.state(), UpdateState::Downloading { .. });
                                    if this.update(cx, |_this, cx| cx.notify()).is_err() || !is_downloading {
                                        break;
                                    }
                                }

                                if let Ok(res) = download_task.await {
                                    if let Err(error) = res {
                                        tracing::error!("Update download task failed: {error:?}");
                                    }
                                }
                                let _ = this.update(cx, |_this, cx| cx.notify());
                            })
                            .detach();
                        }
                    }
                    UpdateBannerEvent::InstallRequested(path) => {
                        let path = std::path::PathBuf::from(path);
                        let mgr = this.update_manager.clone();
                        let version = match mgr.state() {
                            UpdateState::ReadyToInstall { ref version, .. } => version.clone(),
                            _ => crate::data::version::APP_VERSION.to_string(),
                        };
                        cx.spawn(async move |this, cx| {
                            let result = crate::data::client::tokio_runtime()
                                .spawn_blocking(move || UpdateManager::install_update(&path))
                                .await;
                            match result {
                                Ok(Ok(())) => mgr.mark_installed(&version),
                                Ok(Err(error)) => mgr.fail(error.to_string()),
                                Err(error) => mgr.fail(format!("Update install task failed: {error}")),
                            }
                            let _ = this.update(cx, |_this, cx| cx.notify());
                        }).detach();
                    }
                    UpdateBannerEvent::RestartRequested => {
                        if let Err(error) = UpdateManager::restart_app() {
                            tracing::error!("Failed to restart application: {error}");
                            this.update_manager.fail(format!("Restart failed: {error}"));
                            cx.notify();
                        } else {
                            cx.quit();
                        }
                    }
                    UpdateBannerEvent::Dismissed => {
                        this.update_manager.dismiss();
                        cx.notify();
                    }
                    UpdateBannerEvent::CheckRequested => {
                        let mgr = this.update_manager.clone();
                        cx.spawn(async move |this, cx| {
                            let result = crate::data::client::tokio_runtime()
                                .spawn_blocking(move || mgr.force_check())
                                .await;
                            if let Err(error) = result { tracing::warn!("Update check task failed: {error}"); }
                            let _ = this.update(cx, |_this, cx| cx.notify());
                        })
                        .detach();
                    }
                },
            )
            .detach();

            self.update_banner = Some(banner);
        }

        // Determine if update banner should be visible
        let show_update_banner = !matches!(
            self.update_manager.state(),
            UpdateState::Idle | UpdateState::Checking
        );

        // Inform user if guest session has no data fetched
        let show_guest_banner = !self.is_authenticated
            && !self.guest_banner_dismissed
            && (current_page == Page::Home || current_page == Page::Dashboard)
            && (!crate::data::client::is_api_url_configured()
                || crate::data::gateway_registry::GatewayRegistry::global().fetch_failed());

        let guest_banner = if show_guest_banner {
            let no_api = !crate::data::client::is_api_url_configured();
            let banner_msg = if no_api {
                "Guest session: No API service URL configured. Remote market data streaming is disabled until an API endpoint is connected."
            } else {
                "Guest session: No data could be fetched from the configured API service. Check your connection or API URL in Settings."
            };

            Some(
                div()
                    .id("guest-no-data-banner")
                    .w_full()
                    .min_w_0()
                    .px(px(16.0))
                    .py(px(7.0))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap(px(12.0))
                    .bg(rgb(0x1a2638))
                    .border_b_1()
                    .border_color(rgb(0x2a3e5c))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(8.0))
                            .min_w_0()
                            .flex_1()
                            .child(
                                Icon::new(IconName::Info)
                                    .size(px(14.0))
                                    .text_color(rgb(0x60a5fa))
                                    .flex_shrink_0(),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(rgb(0x93c5fd))
                                    .child("Guest Session:"),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(0xbfdbfe))
                                    .truncate()
                                    .child(banner_msg),
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
                                Button::new("guest-configure-api-btn")
                                    .label("Configure in Settings")
                                    .xsmall()
                                    .primary()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.switch_page(Page::Settings);
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("guest-dismiss-banner-btn")
                                    .icon(IconName::Close)
                                    .ghost()
                                    .xsmall()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.guest_banner_dismissed = true;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .into_any_element(),
            )
        } else {
            None
        };

        let content_with_title_bar = div()
            .flex()
            .flex_col()
            .flex_1()
            .overflow_hidden()
            .child(FinTerTitleBar::new("").tabs(self.dashboard_tabs(entity.clone(), cx)))
            .children(crate::components::ui::credit_alert::DashboardCreditBanner::render_opt())
            .children(guest_banner)
            .when(show_update_banner, |this| {
                if let Some(banner) = self.update_banner.as_ref() {
                    this.child(banner.clone())
                } else {
                    this
                }
            })
            .child(
                div().flex_1().flex().flex_col().overflow_hidden().child(
                    div()
                        .flex_1()
                        .overflow_hidden()
                        .when(
                            current_page == Page::Home || current_page == Page::Dashboard,
                            |this| {
                                if let Some(home_page) = self.home_page.as_ref() {
                                    let active_home = home_page.downgrade();
                                    this.child(
                                        div()
                                            .id("main-dashboard-workspace")
                                            .size_full()
                                            .on_mouse_down(gpui::MouseButton::Left, move |_event, _window, cx| {
                                                cx.set_global(crate::components::pages::home::GlobalHomeView {
                                                    view: active_home.clone(),
                                                });
                                            })
                                            .child(home_page.clone()),
                                    )
                                } else {
                                    this
                                }
                            },
                        )
                        .when(current_page == Page::Login, |this| {
                            this.child(
                                div()
                                    .size_full()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_xl()
                                    .text_color(sidebar_fg)
                                    .child("Already logged in"),
                            )
                        })
                        .when(current_page == Page::Feeds, |this| {
                            this.child(
                                div()
                                    .size_full()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_xl()
                                    .text_color(sidebar_fg)
                                    .child("Feeds Page"),
                            )
                        })
                        .when(current_page == Page::Superchat, |this| {
                            this.child(
                                div()
                                    .size_full()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_xl()
                                    .text_color(sidebar_fg)
                                    .child("Superchat Page"),
                            )
                        })
                        .when(current_page == Page::Notification, |this| {
                            this.child(
                                div()
                                    .size_full()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_xl()
                                    .text_color(sidebar_fg)
                                    .child("Notification Page"),
                            )
                        }),
                ),
            );

        let secondary_sidebar_element =
            active_secondary_sidebar.map(|st| self.build_secondary_sidebar(st, cx));

        if is_macos {
            div()
                .flex()
                .flex_row()
                .size_full()
                .bg(theme.colors.background)
                .child(content_with_title_bar)
                .when(secondary_sidebar_element.is_some(), |this| {
                    this.child(secondary_sidebar_element.unwrap())
                })
                .child(sidebar_element)
        } else {
            div()
                .flex()
                .flex_row()
                .size_full()
                .bg(theme.colors.background)
                .child(sidebar_element)
                .when(secondary_sidebar_element.is_some(), |this| {
                    this.child(secondary_sidebar_element.unwrap())
                })
                .child(content_with_title_bar)
        }
    }
}

fn collect_exchanges_from_json(
    val: &serde_json::Value,
    set: &mut std::collections::HashSet<String>,
) {
    match val {
        serde_json::Value::Object(map) => {
            if let Some(ex) = map.get("exchange").and_then(|v| v.as_str()) {
                let trimmed = ex.trim().to_lowercase();
                if !trimmed.is_empty() {
                    set.insert(trimmed);
                }
            }
            for v in map.values() {
                collect_exchanges_from_json(v, set);
            }
        }
        serde_json::Value::Array(arr) => {
            for v in arr {
                collect_exchanges_from_json(v, set);
            }
        }
        _ => {}
    }
}
