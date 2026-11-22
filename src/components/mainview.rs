use crate::components::pages::login::AuthEvent;
use crate::components::pages::{
    dashboard::DashboardPage,
    home::{HomePage, HomePageEvent},
    login::LoginPage,
    profile::ProfilePage,
    settings::SettingsPage,
};
use crate::components::ui::alerts_panel::{AlertsPanel, AlertsPanelEvent};
use crate::components::ui::delete_dashboard_dialog::{
    DeleteDashboardDialogEvent, open_delete_dashboard_dialog,
};
use crate::components::ui::sidebar::{self, SidebarUserInfo};
use crate::components::ui::title_bar::FinTerTitleBar;
use crate::components::ui::update_banner::{UpdateBanner, UpdateBannerEvent};
use crate::data::client::ApiClient;
use crate::data::models::{LayoutEntry, LayoutEntryKind, Module, SettingsData};
use crate::data::pane_tree::PaneNode;
use crate::data::theme::apply_theme;
use crate::data::update_manager::UpdateManager;
use crate::data::version::{UpdateChannel, UpdateState, VersionInfo};
use gpui::BorrowAppContext as _;
use gpui::prelude::FluentBuilder;
use gpui::{Animation, AnimationExt as _, ease_out_quint};
use gpui::{
    AnyElement, App, AppContext, Context, Entity, InteractiveElement, IntoElement, ParentElement,
    Render, StatefulInteractiveElement, StyleRefinement, Styled, Task, Window, WindowControlArea,
    div, px, relative, rgb,
};
use gpui_component::Sizable;
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::theme::ActiveTheme;
use gpui_component::{Icon, IconName};
use std::sync::Arc;
use std::time::Duration;

/// Process-wide handle to the main workspace, used by Agent `terminal`
/// directives from any window.
pub struct GlobalMainView {
    pub view: gpui::WeakEntity<MainView>,
}

impl gpui::Global for GlobalMainView {}

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Page {
    Login,
    Home,
    Dashboard,
    SimpleDashboard, // New PaneGrid implementation
    Feeds,
    Notification,
    Settings,
    Profile,
    File,
}

/// Type of secondary sidebar to show
#[derive(PartialEq, Clone, Copy)]
pub enum SecondarySidebar {
    Dashboard,
    Feeds,
    Notifications,
    Settings,
}

/// Pending dialog actions (since subscribe doesn't have Window access)
enum PendingDialogAction {
    OpenDeleteDashboard {
        module_id: String,
        module_name: String,
        is_folder: bool,
        child_count: usize,
    },
}

/// Pending module load (for startup before HomePage is created)
#[derive(Clone)]
struct PendingModuleLoad {
    module_id: String,
    module: Module,
}

/// One entry in the title-bar tab strip: a saved layout/dashboard or a local
/// file opened from the explorer.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum WorkspaceTab {
    Dashboard(String),
    File(String),
}

/// Payload used to move a tab around the strip or between editor groups.
#[derive(Clone, Debug)]
struct DragWorkspaceTab {
    tab: WorkspaceTab,
}

/// Invisible drag payload that lets the split divider resize itself.
#[derive(Clone, Debug)]
struct SplitDrag;

struct SplitDragPreview;

impl Render for SplitDragPreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

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
        div()
            .w(px(160.0))
            .p(px(10.0))
            .bg(cx.theme().colors.background)
            .text_xs()
            .child(self.0.clone())
    }
}

/// Cached active-file content shared with the Agent. The agent snapshot is
/// republished several times per second, so the text is only re-read when the
/// file's length or modification time changes.
struct AgentFileSnapshot {
    path: String,
    len: u64,
    modified: Option<std::time::SystemTime>,
    text: Option<String>,
    truncated: bool,
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
    /// Generic local file opened from the explorer, plus every open file tab.
    file_editor: Option<Entity<crate::components::ui::file_editor::FileEditor>>,
    file_editor_title: Option<String>,
    active_file: Option<String>,
    file_tabs: Vec<String>,
    /// Title-bar tabs in display order, interleaving dashboards and files.
    workspace_tabs: Vec<WorkspaceTab>,
    /// Index into `workspace_tabs` where the second editor group begins.
    /// `None` means the workspace is a single tab group.
    split_at: Option<usize>,
    /// Active tab for each editor group.
    group_active: [Option<WorkspaceTab>; 2],
    /// Focused editor group (0 or 1).
    focused_group: usize,
    /// Horizontal split ratio (share given to the left group).
    split_ratio: f32,
    /// Which group edge is currently previewed while dragging a tab.
    edge_hover: Option<(usize, bool)>,
    file_editors:
        std::collections::HashMap<String, Entity<crate::components::ui::file_editor::FileEditor>>,
    /// Live CoinWatch alert desk shown in the secondary sidebar.
    alerts_panel: Option<Entity<AlertsPanel>>,
    api_client: ApiClient,
    is_authenticated: bool,
    /// Redacted account snapshot for the native Profile page.
    account_summary: Option<serde_json::Value>,
    account_summary_loading: bool,
    account_refresh_task: Option<Task<()>>,
    /// Periodically republishes live panel data to every Agent pane.
    agent_context_task: Option<Task<()>>,
    /// Workspace-level Agent surface: floating composer and transcript.
    agent_overlay_panel: Option<Entity<crate::components::ui::agent_panel::AgentPanel>>,
    /// Last snapshot published to the workspace overlay Agent.
    agent_overlay_context: Option<serde_json::Value>,
    /// Bounded cache of the active file's text for the Agent.
    agent_file_snapshot: Option<AgentFileSnapshot>,
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
    /// Local workspace tree retained until the lazy secondary sidebar exists.
    layout_entries: Vec<LayoutEntry>,
    /// Cached layout data (layout path -> Module) for instant switching
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
            file_editor: None,
            file_editor_title: None,
            active_file: None,
            file_tabs: Vec::new(),
            workspace_tabs: Vec::new(),
            split_at: None,
            group_active: [None, None],
            focused_group: 0,
            split_ratio: 0.5,
            edge_hover: None,
            file_editors: std::collections::HashMap::new(),
            alerts_panel: None,
            api_client,
            is_authenticated: false,
            account_summary: None,
            account_summary_loading: false,
            account_refresh_task: None,
            agent_context_task: None,
            agent_overlay_panel: None,
            agent_overlay_context: None,
            agent_file_snapshot: None,
            is_restoring: false,
            user_settings: None,
            active_module: None,
            pending_dialog_action: None,
            pending_module_load: None,
            dashboard_modules: Vec::new(),
            layout_entries: Vec::new(),
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

    /// Legacy single-layout file, read once when seeding the dashboard store.
    fn guest_layout_path() -> Option<std::path::PathBuf> {
        dirs::config_dir().map(|mut p| {
            p.push("finter");
            p.push("guest_layout.json");
            p
        })
    }

    /// Local tab preferences are kept per guest/cloud profile.
    fn tab_profile(&self) -> crate::data::tab_settings::TabProfile {
        if self.is_authenticated {
            crate::data::tab_settings::TabProfile::Cloud
        } else {
            crate::data::tab_settings::TabProfile::Guest
        }
    }

    /// Keep `file_tabs` in the same order as the file entries in the strip.
    fn sync_file_tabs_order(&mut self) {
        self.file_tabs = self
            .workspace_tabs
            .iter()
            .filter_map(|tab| match tab {
                WorkspaceTab::File(path) => Some(path.clone()),
                WorkspaceTab::Dashboard(_) => None,
            })
            .collect();
    }

    /// True when a second editor group is showing.
    fn split_visible(&self) -> bool {
        self.split_at
            .is_some_and(|at| at > 0 && at < self.workspace_tabs.len())
    }

    fn group_count(&self) -> usize {
        if self.split_visible() { 2 } else { 1 }
    }

    fn group_tabs(&self, group: usize) -> &[WorkspaceTab] {
        match (self.split_at, group) {
            (Some(at), 0) if at <= self.workspace_tabs.len() => &self.workspace_tabs[..at],
            (Some(at), 1) if at <= self.workspace_tabs.len() => &self.workspace_tabs[at..],
            (_, 0) => &self.workspace_tabs,
            _ => &[],
        }
    }

    fn group_of_tab(&self, tab: &WorkspaceTab) -> Option<usize> {
        let position = self
            .workspace_tabs
            .iter()
            .position(|candidate| candidate == tab)?;
        match self.split_at {
            Some(at) => Some(if position < at { 0 } else { 1 }),
            None => Some(0),
        }
    }

    fn split_group_vecs(&self) -> (Vec<WorkspaceTab>, Option<Vec<WorkspaceTab>>) {
        match self.split_at {
            Some(at) if at > 0 && at < self.workspace_tabs.len() => (
                self.workspace_tabs[..at].to_vec(),
                Some(self.workspace_tabs[at..].to_vec()),
            ),
            _ => (self.workspace_tabs.clone(), None),
        }
    }

    /// Replace the group contents. `None` (or an empty side) collapses back to
    /// a single group.
    fn set_group_vecs(&mut self, left: Vec<WorkspaceTab>, right: Option<Vec<WorkspaceTab>>) {
        match right {
            Some(right) if !left.is_empty() && !right.is_empty() => {
                self.workspace_tabs = left;
                self.split_at = Some(self.workspace_tabs.len());
                self.workspace_tabs.extend(right);
            }
            _ => {
                let mut single = left;
                if let Some(right) = right {
                    single.extend(right);
                }
                self.workspace_tabs = single;
                self.split_at = None;
                if self.focused_group == 1 {
                    self.group_active[0] = self.group_active[1].take();
                }
                self.focused_group = 0;
                self.group_active[1] = None;
            }
        }
        self.ensure_group_actives();
    }

    /// Make sure each group's active tab belongs to that group.
    fn ensure_group_actives(&mut self) {
        if self.group_count() == 1 {
            self.focused_group = 0;
            self.group_active[1] = None;
        }
        for group in 0..2 {
            let valid = self.group_active[group]
                .as_ref()
                .is_some_and(|active| self.group_tabs(group).contains(active));
            if !valid {
                self.group_active[group] = self.group_tabs(group).first().cloned();
            }
        }
    }

    /// Track the pointer side while a tab is dragged over a group's content.
    fn track_edge_hover(
        &mut self,
        group: usize,
        event: &gpui::DragMoveEvent<DragWorkspaceTab>,
        cx: &mut Context<Self>,
    ) {
        let width = f32::from(event.bounds.size.width);
        if width <= 1.0 {
            return;
        }
        let x = f32::from(event.event.position.x - event.bounds.left());
        let ratio = x / width;
        let next = if ratio < 0.15 {
            Some((group, true))
        } else if ratio > 0.85 {
            Some((group, false))
        } else {
            None
        };
        if self.edge_hover != next {
            self.edge_hover = next;
            cx.notify();
        }
    }

    /// Focus a group and show its active tab.
    fn focus_group(&mut self, group: usize, cx: &mut Context<Self>) {
        let group = group.min(self.group_count().saturating_sub(1));
        if self.focused_group == group {
            return;
        }
        self.focused_group = group;
        let active = self.group_active[group].clone();
        self.show_active_tab(active, cx);
        cx.notify();
    }

    /// Show an already-open tab as the focused group's content.
    fn show_active_tab(&mut self, tab: Option<WorkspaceTab>, cx: &mut Context<Self>) {
        match tab {
            Some(WorkspaceTab::Dashboard(id)) => self.load_and_apply_module(id, cx),
            Some(WorkspaceTab::File(path)) => self.activate_file_tab(path, cx),
            None => {
                self.active_module = None;
                self.home_page = None;
                self.file_editor = None;
                self.file_editor_title = None;
                self.active_file = None;
                self.current_page = Page::Home;
                self.refresh_agent_context(cx);
                cx.notify();
            }
        }
    }

    /// Insert a tab at `index` inside `group`.
    fn insert_tab_in_group(&mut self, group: usize, index: usize, tab: WorkspaceTab) {
        let (mut left, right) = self.split_group_vecs();
        if group == 1 {
            if let Some(mut right) = right {
                let index = index.min(right.len());
                right.insert(index, tab);
                self.set_group_vecs(left, Some(right));
            } else {
                left.push(tab);
                self.set_group_vecs(left, None);
            }
        } else {
            let index = index.min(left.len());
            left.insert(index, tab);
            self.set_group_vecs(left, right);
        }
    }

    /// Move a tab into `target_group` before `target_index` (or at the end when
    /// `None`). Returns the source group when a move happened.
    fn move_tab_to_group_at(
        &mut self,
        dragged: &WorkspaceTab,
        target_group: usize,
        target_index: Option<usize>,
    ) -> Option<usize> {
        let (mut left, right) = self.split_group_vecs();
        let mut right = right.unwrap_or_default();
        let source = if left.contains(dragged) {
            0
        } else if right.contains(dragged) {
            1
        } else {
            return None;
        };
        if source == 0 {
            left.retain(|tab| tab != dragged);
        } else {
            right.retain(|tab| tab != dragged);
        }
        let target = if target_group == 1 {
            &mut right
        } else {
            &mut left
        };
        let index = target_index.unwrap_or(target.len()).min(target.len());
        target.insert(index, dragged.clone());
        self.set_group_vecs(left, Some(right).filter(|right| !right.is_empty()));
        Some(source)
    }

    /// Remove a tab from whichever group holds it.
    fn remove_tab_everywhere(&mut self, tab: &WorkspaceTab) -> Option<usize> {
        let group = self.group_of_tab(tab)?;
        let (mut left, right) = self.split_group_vecs();
        let mut right = right;
        if group == 0 {
            left.retain(|candidate| candidate != tab);
        } else if let Some(right) = right.as_mut() {
            right.retain(|candidate| candidate != tab);
        }
        self.set_group_vecs(left, right);
        Some(group)
    }

    fn reorder_dashboards_from_tabs(&mut self) {
        let order: Vec<String> = self
            .workspace_tabs
            .iter()
            .filter_map(|tab| match tab {
                WorkspaceTab::Dashboard(id) => Some(id.clone()),
                WorkspaceTab::File(_) => None,
            })
            .collect();
        self.dashboard_modules.sort_by_key(|module| {
            module
                .id
                .as_ref()
                .and_then(|id| order.iter().position(|ordered| ordered == id))
                .unwrap_or(usize::MAX)
        });
    }

    /// Make the editor groups agree with the live dashboard and file sets while
    /// preserving the user's chosen order. Newly discovered dashboards are
    /// appended to the focused group.
    fn sync_workspace_tabs(&mut self) {
        let dashboard_order: Vec<String> = self
            .dashboard_modules
            .iter()
            .filter_map(|module| module.id.clone())
            .collect();
        let dashboard_ids: std::collections::HashSet<String> = dashboard_order
            .iter()
            .filter(|id| !self.closed_dashboard_tabs.contains(*id))
            .cloned()
            .collect();
        let file_tabs = self.file_tabs.clone();
        let (mut left, right) = self.split_group_vecs();
        let mut right = right;

        let mut seen = std::collections::HashSet::new();
        let mut retain_tab = |tabs: &mut Vec<WorkspaceTab>| {
            tabs.retain(|tab| match tab {
                WorkspaceTab::Dashboard(id) => {
                    dashboard_ids.contains(id) && seen.insert(tab.clone())
                }
                WorkspaceTab::File(path) => file_tabs.contains(path) && seen.insert(tab.clone()),
            });
        };
        retain_tab(&mut left);
        if let Some(right) = right.as_mut() {
            retain_tab(right);
        }

        let target_group = self.focused_group.min(if right.is_some() { 1 } else { 0 });
        for id in dashboard_order
            .iter()
            .filter(|id| dashboard_ids.contains(*id))
        {
            let entry = WorkspaceTab::Dashboard(id.clone());
            if left.contains(&entry) || right.as_ref().is_some_and(|right| right.contains(&entry)) {
                continue;
            }
            if target_group == 1 {
                if let Some(right) = right.as_mut() {
                    right.push(entry);
                } else {
                    left.push(entry);
                }
            } else {
                left.push(entry);
            }
        }

        self.set_group_vecs(left, right);
        self.sync_file_tabs_order();
    }

    /// Persist the open tabs and their order to the local tab settings file.
    fn persist_open_tabs(&mut self) {
        self.sync_workspace_tabs();

        let open_tabs: Vec<crate::data::tab_settings::OpenTab> = self
            .workspace_tabs
            .iter()
            .map(|tab| match tab {
                WorkspaceTab::Dashboard(id) => {
                    crate::data::tab_settings::OpenTab::dashboard(id.clone())
                }
                WorkspaceTab::File(path) => crate::data::tab_settings::OpenTab::file(path.clone()),
            })
            .collect();
        let open_ids: Vec<String> = open_tabs
            .iter()
            .filter_map(|tab| match tab {
                crate::data::tab_settings::OpenTab::Dashboard { id } => Some(id.clone()),
                crate::data::tab_settings::OpenTab::File { .. } => None,
            })
            .collect();

        let mut preferences = crate::data::tab_settings::load(self.tab_profile());
        // "Start as empty" should not erase the last restored session; keep it
        // available if the user switches back to "Restore last session".
        if preferences.startup_mode == crate::data::tab_settings::StartupTabMode::Empty
            && open_tabs.is_empty()
        {
            return;
        }
        if preferences.open_tab_ids == open_ids && preferences.open_tabs == open_tabs {
            return;
        }
        preferences.open_tab_ids = open_ids;
        preferences.open_tabs = open_tabs;
        if let Err(error) = crate::data::tab_settings::save(self.tab_profile(), &preferences) {
            tracing::warn!(?error, "Failed to persist open tabs");
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

        // "Restore" is the only mode that brings files back; the other modes
        // still keep files that are open right now (runtime re-apply).
        let restore_saved_files = preferences.startup_mode
            == crate::data::tab_settings::StartupTabMode::Restore
            && !preferences.open_tabs.is_empty();

        let mut order: Vec<WorkspaceTab> = Vec::new();
        let mut seen_dashboards = std::collections::HashSet::new();
        let mut seen_files = std::collections::HashSet::new();

        if restore_saved_files {
            for tab in &preferences.open_tabs {
                match tab {
                    crate::data::tab_settings::OpenTab::Dashboard { id } => {
                        if open_ids.contains(id) && seen_dashboards.insert(id.clone()) {
                            order.push(WorkspaceTab::Dashboard(id.clone()));
                        }
                    }
                    crate::data::tab_settings::OpenTab::File { path } => {
                        if !seen_files.contains(path)
                            && Self::workspace_file_exists(path)
                            && seen_files.insert(path.clone())
                        {
                            order.push(WorkspaceTab::File(path.clone()));
                        }
                    }
                }
            }
        }

        // Append resolved dashboards (and legacy settings without `open_tabs`)
        // in the order the startup mode chose.
        for id in &open_ids {
            if seen_dashboards.insert(id.clone()) {
                order.push(WorkspaceTab::Dashboard(id.clone()));
            }
        }
        let open_files = self.file_tabs.clone();
        for path in open_files {
            if seen_files.insert(path.clone()) {
                order.push(WorkspaceTab::File(path));
            }
        }

        self.workspace_tabs = order;
        self.sync_file_tabs_order();
        self.reorder_dashboards_from_tabs();

        self.closed_dashboard_tabs = available_ids
            .iter()
            .filter(|id| !open_ids.contains(id))
            .cloned()
            .collect();

        self.persist_open_tabs();
        open_ids
    }

    /// Whether a persisted file tab can still be opened.
    fn workspace_file_exists(path: &str) -> bool {
        crate::data::layout_workspace::absolute_path(path).is_ok_and(|path| path.is_file())
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

    /// Keep the explorer's open-tab dots in sync with the title-bar tabs.
    fn sync_manager_open_tabs(&self, cx: &mut Context<Self>) {
        let Some(manager) = &self.dashboard_manager else {
            return;
        };
        let open_ids: Vec<String> = self
            .dashboard_modules
            .iter()
            .filter_map(|module| module.id.clone())
            .filter(|id| !self.closed_dashboard_tabs.contains(id))
            .collect();
        manager.update(cx, |manager, cx| manager.set_open_modules(open_ids, cx));
    }

    /// Load the local workspace and restore the last active layout.
    fn load_guest_layout(&mut self, cx: &mut Context<Self>) {
        self.guest_banner_dismissed = false;
        // Load guest settings to apply theme/timezone independently of cloud user
        if let Some(path) = Self::guest_settings_path() {
            if let Ok(json) = std::fs::read_to_string(&path) {
                if let Ok(mut settings) =
                    serde_json::from_str::<crate::data::models::SettingsData>(&json)
                {
                    settings.server_url = settings
                        .server_url
                        .as_deref()
                        .and_then(crate::data::client::normalize_api_origin);
                    let update_channel = settings.update_channel.clone();
                    let theme_name = settings
                        .theme
                        .clone()
                        .unwrap_or_else(|| "gruvbox".to_string());
                    let dark_mode = settings
                        .dark_mode
                        .as_ref()
                        .map(|s| s == "dark")
                        .unwrap_or(false);
                    apply_theme(&theme_name, dark_mode, cx);
                    self.user_settings = Some(settings);
                    self.apply_update_channel(update_channel.as_deref(), cx);
                    if let Some(home) = &self.home_page {
                        if let Some(s) = &self.user_settings {
                            let s = s.clone();
                            home.update(cx, |page, cx| page.apply_settings(s, cx));
                        }
                    }
                }
            }
        }

        self.restore_local_workspace(cx);
    }

    /// Seed the app-managed workspace once so a fresh install is not blank.
    /// A user-chosen folder is never touched.
    fn seed_workspace_if_empty(&self) {
        let Ok(snapshot) = crate::data::layout_workspace::snapshot() else {
            return;
        };
        if !snapshot.documents.is_empty() {
            return;
        }
        let (Ok(root), Ok(default)) = (
            crate::data::layout_workspace::root(),
            crate::data::layout_workspace::default_root(),
        ) else {
            return;
        };
        if root != default {
            return;
        }

        let legacy_tree = Self::guest_layout_path()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|content| serde_json::from_str::<PaneNode>(&content).ok());
        let tree = legacy_tree.unwrap_or_else(crate::data::modules::default_workspace);
        let tree_value = serde_json::to_value(&tree).unwrap_or_default();

        match crate::data::layout_workspace::create_layout("", "Local Workspace") {
            Ok(document) => {
                if let Err(error) =
                    crate::data::layout_workspace::save_layout(&document.path, tree_value)
                {
                    tracing::error!(?error, "Failed to seed local workspace");
                }
            }
            Err(error) => tracing::error!(?error, "Failed to create local workspace layout"),
        }
    }

    /// Reload the workspace tree from disk and apply startup preferences.
    fn restore_local_workspace(&mut self, cx: &mut Context<Self>) {
        self.seed_workspace_if_empty();
        let snapshot = match crate::data::layout_workspace::snapshot() {
            Ok(snapshot) => snapshot,
            Err(error) => {
                tracing::error!(?error, "Failed to load local layout workspace");
                self.show_manager_error(error, cx);
                return;
            }
        };
        self.apply_workspace_snapshot(snapshot, cx);
    }

    fn resolve_legacy_tab_preferences(&mut self) {
        let mut preferences = crate::data::tab_settings::load(self.tab_profile());
        let mut changed = false;
        for id in &mut preferences.open_tab_ids {
            if self
                .dashboard_modules
                .iter()
                .any(|module| module.id.as_deref() == Some(id.as_str()))
            {
                continue;
            }
            if let Some(resolved) =
                crate::data::layout_workspace::resolve_legacy_id(&self.layout_entries, id)
            {
                *id = resolved;
                changed = true;
            }
        }
        for tab in &mut preferences.open_tabs {
            let crate::data::tab_settings::OpenTab::Dashboard { id } = tab else {
                continue;
            };
            if self
                .dashboard_modules
                .iter()
                .any(|module| module.id.as_deref() == Some(id.as_str()))
            {
                continue;
            }
            if let Some(resolved) =
                crate::data::layout_workspace::resolve_legacy_id(&self.layout_entries, id)
            {
                *id = resolved;
                changed = true;
            }
        }
        if changed {
            if let Err(error) = crate::data::tab_settings::save(self.tab_profile(), &preferences) {
                tracing::warn!(?error, "Failed to migrate legacy tab paths");
            }
        }
    }

    fn apply_workspace_snapshot(
        &mut self,
        snapshot: crate::data::layout_workspace::WorkspaceSnapshot,
        cx: &mut Context<Self>,
    ) {
        // Force the selected layout to be re-applied against the freshly read
        // tree instead of short-circuiting on a stale active module.
        self.active_module = None;
        self.home_page = None;
        self.pending_module_load = None;
        self.layout_entries = snapshot.entries;
        self.dashboard_modules = snapshot.documents;
        self.split_at = None;
        self.group_active = [None, None];
        self.focused_group = 0;
        self.layout_cache = self
            .dashboard_modules
            .iter()
            .filter_map(|module| Some((module.id.clone()?, module.clone())))
            .collect();
        self.resolve_legacy_tab_preferences();

        if let Some(manager) = &self.dashboard_manager {
            let entries = self.layout_entries.clone();
            let root_name = snapshot.root_name.clone();
            let open_ids: Vec<String> = self
                .dashboard_modules
                .iter()
                .filter_map(|module| module.id.clone())
                .filter(|id| !self.closed_dashboard_tabs.contains(id))
                .collect();
            manager.update(cx, |manager, cx| {
                manager.update_tree(entries, root_name, cx);
                manager.set_open_modules(open_ids, cx);
            });
        }
        self.sync_settings_dashboards(cx);

        if self.dashboard_modules.is_empty() {
            self.active_module = None;
            self.home_page = None;
            self.pending_module_load = None;
            self.dashboard_views.clear();
            if let Some(manager) = &self.dashboard_manager {
                manager.update(cx, |manager, cx| manager.clear_active_module(cx));
            }
            // A workspace can legitimately contain only files; restore those
            // tabs even when there are no dashboards.
            self.apply_startup_tab_preferences();
            return;
        }

        let open_ids = self.apply_startup_tab_preferences();
        if let Some(manager) = &self.dashboard_manager {
            let open_ids_for_manager = open_ids.clone();
            manager.update(cx, |manager, cx| {
                manager.set_open_modules(open_ids_for_manager, cx)
            });
        }
        if crate::data::tab_settings::load(self.tab_profile()).startup_mode
            == crate::data::tab_settings::StartupTabMode::Empty
        {
            self.active_module = None;
            self.home_page = None;
            self.pending_module_load = None;
            self.dashboard_views.clear();
            if let Some(manager) = &self.dashboard_manager {
                manager.update(cx, |manager, cx| manager.clear_active_module(cx));
            }
            return;
        }

        let active_path = crate::data::layout_workspace::active_layout().and_then(|path| {
            if self
                .dashboard_modules
                .iter()
                .any(|module| module.id.as_deref() == Some(path.as_str()))
            {
                Some(path)
            } else {
                crate::data::layout_workspace::resolve_legacy_id(&self.layout_entries, &path)
            }
        });
        let target = active_path
            .as_deref()
            .and_then(|path| {
                self.dashboard_modules
                    .iter()
                    .find(|module| module.id.as_deref() == Some(path))
            })
            .or_else(|| {
                self.dashboard_modules
                    .iter()
                    .find(|module| module.id.as_ref().is_some_and(|id| open_ids.contains(id)))
            })
            .or_else(|| self.dashboard_modules.first())
            .cloned();

        if let Some(module) = target {
            if let Some(id) = module.id.clone() {
                self.load_and_apply_module(id, cx);
            }
        } else {
            self.active_module = None;
            self.home_page = None;
            self.pending_module_load = None;
            self.dashboard_views.clear();
        }
    }

    /// Kick off a background session restore. Called once, right after creation.
    pub fn start_restore(&mut self, cx: &mut Context<Self>) {
        if self.is_restoring {
            return;
        }
        self.is_restoring = true;

        // Synchronously load settings cache on initial startup
        let has_saved_session = crate::data::session_store::load_session()
            .ok()
            .flatten()
            .is_some();
        let startup_settings_path = if has_saved_session {
            Self::cloud_settings_path()
                .filter(|p| p.exists())
                .or_else(Self::guest_settings_path)
        } else {
            Self::cloud_settings_path()
                .filter(|p| p.exists())
                .or_else(Self::guest_settings_path)
        };
        if let Some(path) = startup_settings_path {
            if let Ok(json) = std::fs::read_to_string(&path) {
                if let Ok(mut settings) =
                    serde_json::from_str::<crate::data::models::SettingsData>(&json)
                {
                    // Drop legacy/invalid origins so they cannot be re-applied
                    // or written back to the server settings record.
                    settings.server_url = settings
                        .server_url
                        .as_deref()
                        .and_then(crate::data::client::normalize_api_origin);
                    if let Some(channel) = settings
                        .update_channel
                        .as_deref()
                        .and_then(UpdateChannel::parse)
                    {
                        crate::data::version::set_selected_channel(channel);
                    }
                    if let Some(server_url) = settings.server_url.as_deref() {
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

        // Layouts are local: load them before any network work so Guest Mode
        // starts immediately and never waits on a backend.
        self.guest_banner_dismissed = false;
        self.restore_local_workspace(cx);
        if !has_saved_session {
            self.is_restoring = false;
            cx.notify();
            return;
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
                    // Load account settings, then restore the local layout workspace
                    this.load_settings_then_modules(cx);
                    this.load_account_summary(cx);
                    this.ensure_alerts_panel(cx);
                    if let Some(panel) = &this.alerts_panel {
                        panel.update(cx, |panel, cx| panel.refresh(cx));
                    }
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
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(15))
                    .await;
                if this
                    .update(cx, |view, cx| {
                        if view.current_page != Page::Profile || !view.is_authenticated {
                            return false;
                        }
                        view.load_account_summary(cx);
                        true
                    })
                    .unwrap_or(false)
                    == false
                {
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

    /// Create the alerts desk once and keep it alive so its poll task, inputs,
    /// and scroll state survive closing and reopening the sidebar.
    fn ensure_alerts_panel(&mut self, cx: &mut Context<Self>) {
        if self.alerts_panel.is_some() {
            return;
        }

        let client = self.api_client.clone();
        let panel = cx.new(|cx| AlertsPanel::new(client, cx));
        cx.subscribe(
            &panel,
            |this, _panel, event: &AlertsPanelEvent, cx| match event {
                AlertsPanelEvent::RequestLogin => {
                    this.active_secondary_sidebar = None;
                    this.switch_page(Page::Login);
                    cx.notify();
                }
            },
        )
        .detach();
        self.alerts_panel = Some(panel);
    }

    /// Toggle a secondary sidebar (close if already open)
    pub fn toggle_secondary_sidebar(&mut self, sidebar: SecondarySidebar, cx: &mut Context<Self>) {
        if self.active_secondary_sidebar == Some(sidebar) {
            self.active_secondary_sidebar = None;
        } else {
            self.active_secondary_sidebar = Some(sidebar);

            if sidebar == SecondarySidebar::Notifications {
                self.ensure_alerts_panel(cx);
                if let Some(panel) = &self.alerts_panel {
                    panel.update(cx, |panel, cx| panel.refresh(cx));
                }
            }

            // Opening the layout explorer rescans the local workspace. Layouts
            // never touch the network, signed in or not.
            if sidebar == SecondarySidebar::Dashboard {
                self.reload_dashboards_from_store(cx);
            }
        }
    }

    /// Create (once) the workspace-level Agent used by the floating composer.
    /// It is not tied to a dashboard or pane, so `ui` directives fall back to the
    /// dashboard currently under the cursor.
    fn ensure_agent_overlay_panel(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Entity<crate::components::ui::agent_panel::AgentPanel> {
        if let Some(panel) = &self.agent_overlay_panel {
            return panel.clone();
        }
        let panel = cx.new(|cx| crate::components::ui::agent_panel::AgentPanel::new(cx));
        if let Some(context) = self.agent_overlay_context.clone() {
            panel.update(cx, |panel, _| panel.set_context(context));
        }
        self.agent_overlay_panel = Some(panel.clone());
        panel
    }

    /// Build the floating workspace Agent surfaces (transcript and composer).
    ///
    /// Shown on dashboards and text-file tabs. Both elements are absolutely
    /// positioned, so the caller must place them in a `relative()` container.
    fn agent_overlay_elements(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (Option<AnyElement>, Option<AnyElement>) {
        if !matches!(self.current_page, Page::Home | Page::Dashboard | Page::File) {
            return (None, None);
        }

        let overlay_panel = self.ensure_agent_overlay_panel(cx);
        let overlay_bubbles = overlay_panel.read(cx).overlay_bubbles_visible().then(|| {
            overlay_panel.update(cx, |panel, cx| panel.render_overlay_bubbles(window, cx))
        });
        let overlay_composer = if overlay_panel.read(cx).overlay_composer_visible() {
            Some(overlay_panel.update(cx, |panel, cx| panel.render_overlay_composer(window, cx)))
        } else {
            let panel_for_open = overlay_panel.clone();
            Some(
                div()
                    .absolute()
                    .bottom(px(20.0))
                    .left(px(0.0))
                    .right(px(0.0))
                    .flex()
                    .justify_center()
                    .child(
                        Button::new("agent-overlay-open")
                            .label("Ask FinTer…")
                            .on_click(move |_, _, app| {
                                panel_for_open.update(app, |panel, cx| {
                                    panel.set_overlay_composer_visible(true, cx);
                                });
                            }),
                    )
                    .into_any_element(),
            )
        };
        (overlay_bubbles, overlay_composer)
    }

    /// Applies one Agent-issued workspace command and reports the outcome.
    ///
    /// `caller_dashboard` identifies the dashboard hosting the conversation, if
    /// any. Destructive commands refuse to target it so the Agent cannot close
    /// or delete the screen it is talking from.
    pub fn apply_terminal_action(
        &mut self,
        action: &crate::data::terminal_control::TerminalAction,
        caller_dashboard: Option<&str>,
        _caller_pane: Option<&str>,
        cx: &mut Context<Self>,
    ) -> crate::data::pane_control::UiActionResult {
        use crate::data::pane_control::UiActionResult;
        use crate::data::terminal_control::TerminalAction;

        let name = action.action_name();

        match action {
            TerminalAction::OpenDashboard { dashboard } => {
                let path = match self.resolve_dashboard(dashboard.as_deref(), cx) {
                    Ok(path) => path,
                    Err(error) => return UiActionResult::failure(name, error),
                };
                // A detached dashboard already lives on its own screen; focus
                // that window instead of opening a second copy in a tab. If the
                // handle is stale, fall through and open the tab.
                if let Some(window) = Self::detached_window(&path, cx) {
                    if window
                        .update(cx, |_, window, _| window.activate_window())
                        .is_ok()
                    {
                        return UiActionResult::success(
                            name,
                            format!("Focused {path} on its detached window."),
                        )
                        .with_dashboard(path);
                    }
                }
                self.closed_dashboard_tabs.remove(&path);
                self.load_and_apply_module(path.clone(), cx);
                UiActionResult::success(name, format!("Opened {path}.")).with_dashboard(path)
            }
            TerminalAction::CloseTab { dashboard } => {
                let path = match self.resolve_dashboard(dashboard.as_deref(), cx) {
                    Ok(path) => path,
                    Err(error) => return UiActionResult::failure(name, error),
                };
                if caller_dashboard.is_some_and(|caller| path_is_or_under(caller, &path)) {
                    return UiActionResult::failure(
                        name,
                        "The dashboard hosting this conversation cannot close its own view.",
                    )
                    .with_dashboard(path);
                }
                if let Some(window) = Self::detached_window(&path, cx) {
                    let _ = window.update(cx, |_, window, _| window.remove_window());
                    if cx.has_global::<crate::components::ui::pane_control::DashboardRegistry>() {
                        cx.update_global::<crate::components::ui::pane_control::DashboardRegistry, _>(
                            |registry, _| registry.remove(&path),
                        );
                    }
                    return UiActionResult::success(
                        name,
                        format!("Closed the detached window for {path}."),
                    )
                    .with_dashboard(path);
                }
                if self.closed_dashboard_tabs.contains(&path) {
                    return UiActionResult::success(name, format!("{path} is already closed."))
                        .with_dashboard(path);
                }
                self.close_dashboard_tab(path.clone(), cx);
                UiActionResult::success(
                    name,
                    format!("Closed the tab for {path}; the layout is saved."),
                )
                .with_dashboard(path)
            }
            TerminalAction::CreateDashboard {
                name: layout_name,
                parent,
                open,
            } => {
                let layout_name = layout_name.trim().to_string();
                if layout_name.is_empty() {
                    return UiActionResult::failure(name, "A dashboard name is required.");
                }
                if self
                    .dashboard_modules
                    .iter()
                    .any(|module| module.module_name.eq_ignore_ascii_case(&layout_name))
                {
                    return UiActionResult::failure(
                        name,
                        format!("A dashboard named '{layout_name}' already exists."),
                    );
                }
                self.create_new_dashboard(
                    parent.clone().unwrap_or_default(),
                    layout_name.clone(),
                    cx,
                );
                // The Agent already supplied a name; don't leave the inline
                // rename editor open in the sidebar.
                if let Some(manager) = &self.dashboard_manager {
                    manager.update(cx, |manager, cx| manager.cancel_rename(cx));
                }
                let created = self
                    .dashboard_modules
                    .iter()
                    .find(|module| module.module_name.eq_ignore_ascii_case(&layout_name))
                    .and_then(|module| module.id.clone());
                match created {
                    Some(path) => {
                        if open == &Some(false) {
                            self.close_dashboard_tab(path.clone(), cx);
                        }
                        UiActionResult::success(name, format!("Created '{layout_name}'."))
                            .with_dashboard(path)
                    }
                    None => UiActionResult::failure(
                        name,
                        format!(
                            "Could not create '{layout_name}'. Check the workspace folder permissions."
                        ),
                    ),
                }
            }
            TerminalAction::RenameDashboard {
                dashboard,
                name: new_name,
            } => {
                let old_path = match self.resolve_dashboard(Some(dashboard), cx) {
                    Ok(path) => path,
                    Err(error) => return UiActionResult::failure(name, error),
                };
                if self.is_detached_dashboard(&old_path, cx) {
                    return UiActionResult::failure(
                        name,
                        "That dashboard is open in its own window; rename it after attaching it.",
                    )
                    .with_dashboard(old_path);
                }
                if caller_dashboard.is_some_and(|caller| path_is_or_under(caller, &old_path)) {
                    return UiActionResult::failure(
                        name,
                        "The dashboard hosting this conversation cannot be renamed from here.",
                    )
                    .with_dashboard(old_path);
                }
                let new_name = new_name.trim().to_string();
                if new_name.is_empty() {
                    return UiActionResult::failure(name, "A new name is required.");
                }
                let old_active = self
                    .active_module
                    .as_ref()
                    .and_then(|module| module.id.clone());
                let was_active = old_active
                    .as_deref()
                    .is_some_and(|active| path_is_or_under(active, &old_path));
                match crate::data::layout_workspace::rename(&old_path, &new_name) {
                    Ok(new_path) => {
                        self.reload_after_path_change(
                            was_active, old_active, &old_path, &new_path, cx,
                        );
                        UiActionResult::success(name, format!("Renamed to '{new_name}'."))
                            .with_dashboard(new_path)
                    }
                    Err(error) => UiActionResult::failure(name, error.to_string()),
                }
            }
            TerminalAction::DuplicateDashboard { dashboard } => {
                let path = match self.resolve_dashboard(Some(dashboard), cx) {
                    Ok(path) => path,
                    Err(error) => return UiActionResult::failure(name, error),
                };
                let before: std::collections::HashSet<String> = self
                    .dashboard_modules
                    .iter()
                    .filter_map(|module| module.id.clone())
                    .collect();
                match crate::data::layout_workspace::duplicate(&path) {
                    Ok(new_path) => {
                        self.reload_dashboards_from_store(cx);
                        self.closed_dashboard_tabs.remove(&new_path);
                        self.load_and_apply_module(new_path.clone(), cx);
                        self.persist_open_tabs();
                        self.sync_manager_open_tabs(cx);
                        self.sync_settings_dashboards(cx);
                        let created = self
                            .dashboard_modules
                            .iter()
                            .filter_map(|module| module.id.clone())
                            .find(|id| !before.contains(id))
                            .unwrap_or(new_path);
                        UiActionResult::success(name, "Duplicated the layout.")
                            .with_dashboard(created)
                    }
                    Err(error) => UiActionResult::failure(name, error.to_string()),
                }
            }
            TerminalAction::DeleteDashboard { dashboard, confirm } => {
                let path = match self.resolve_dashboard(Some(dashboard), cx) {
                    Ok(path) => path,
                    Err(error) => return UiActionResult::failure(name, error),
                };
                if self.is_detached_dashboard(&path, cx) {
                    return UiActionResult::failure(
                        name,
                        "That dashboard is open in its own window; close the window first.",
                    )
                    .with_dashboard(path);
                }
                if !confirm {
                    return UiActionResult::failure(
                        name,
                        "Deleting a dashboard is destructive; re-issue the directive with \"confirm\":true.",
                    )
                    .with_dashboard(path);
                }
                if caller_dashboard.is_some_and(|caller| path_is_or_under(caller, &path)) {
                    return UiActionResult::failure(
                        name,
                        "Refusing to delete the dashboard hosting this conversation.",
                    )
                    .with_dashboard(path);
                }
                if self.dashboard_modules.len() <= 1 {
                    return UiActionResult::failure(
                        name,
                        "Refusing to delete the last dashboard in the workspace.",
                    )
                    .with_dashboard(path);
                }
                self.delete_dashboard(path.clone(), cx);
                let still_exists = self
                    .dashboard_modules
                    .iter()
                    .any(|module| module.id.as_deref() == Some(path.as_str()));
                if still_exists {
                    UiActionResult::failure(name, format!("Could not delete {path}."))
                        .with_dashboard(path)
                } else {
                    UiActionResult::success(name, format!("Deleted {path}.")).with_dashboard(path)
                }
            }
            TerminalAction::SetMarket {
                exchange,
                symbol,
                timeframe,
                scope,
                dashboard,
            } => {
                let exchange = exchange
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(crate::data::agent::normalize_exchange);
                let symbol = symbol
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(crate::data::agent::normalize_symbol);
                let timeframe = timeframe
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(crate::data::agent::normalize_timeframe);
                if exchange.is_none() && symbol.is_none() && timeframe.is_none() {
                    return UiActionResult::failure(
                        name,
                        "set_market needs at least one of exchange, symbol, or timeframe.",
                    );
                }
                let scope = scope.as_deref().unwrap_or("all");
                // `scope:"dashboard"` from a pane conversation means the
                // dashboard hosting that conversation, not whichever tab the
                // main window happens to show.
                let selector = dashboard.as_deref().or_else(|| {
                    scope
                        .eq_ignore_ascii_case("dashboard")
                        .then_some(caller_dashboard)
                        .flatten()
                });
                let targets = if let Some(selector) = selector {
                    let path = match self.resolve_dashboard(Some(selector), cx) {
                        Ok(path) => path,
                        Err(error) => return UiActionResult::failure(name, error),
                    };
                    let home = self.home_for_path(&path, cx);
                    match home {
                        Some(home) => vec![(path, home)],
                        None => {
                            return UiActionResult::failure(
                                name,
                                format!(
                                    "{path} is not open. Open it first, then change its market."
                                ),
                            )
                            .with_dashboard(path);
                        }
                    }
                } else if scope.eq_ignore_ascii_case("dashboard") {
                    match crate::components::ui::pane_control::active_dashboard_id(cx)
                        .and_then(|id| self.home_for_path(&id, cx).map(|home| (id, home)))
                    {
                        Some(target) => vec![target],
                        None => {
                            return UiActionResult::failure(
                                name,
                                "No dashboard is open to target.",
                            );
                        }
                    }
                } else {
                    self.live_home_targets(cx)
                };

                if targets.is_empty() {
                    return UiActionResult::failure(name, "No open dashboards to update.");
                }

                let mut total = 0usize;
                let mut dashboards = 0usize;
                for (path, home) in targets {
                    let updated = home.update(cx, |page, cx| {
                        page.apply_market_broadcast(
                            exchange.as_deref(),
                            symbol.as_deref(),
                            timeframe.as_deref(),
                            scope,
                            cx,
                        )
                    });
                    if updated > 0 {
                        dashboards += 1;
                        total += updated;
                        // Non-active dashboards are not saved by the HomePage
                        // subscription (it only tracks the active tab), so
                        // persist each edited tree explicitly.
                        if let Some(tree) = home.read(cx).pane_grid.read(cx).tree.clone() {
                            if let Ok(json) = serde_json::to_value(&tree) {
                                if let Err(error) =
                                    crate::data::layout_workspace::save_layout(&path, json.clone())
                                {
                                    tracing::error!(
                                        ?error,
                                        "Failed to save broadcast layout {path}"
                                    );
                                }
                                // Keep the in-memory copies in sync so closing and
                                // reopening the tab does not restore the old tree.
                                if let Some(module) = self
                                    .dashboard_modules
                                    .iter_mut()
                                    .find(|module| module.id.as_deref() == Some(path.as_str()))
                                {
                                    module.module = json.clone();
                                }
                                if let Some(cached) = self.layout_cache.get_mut(&path) {
                                    cached.module = json;
                                }
                            }
                        }
                    }
                }
                UiActionResult::success(
                    name,
                    format!(
                        "Updated {total} pane{} across {dashboards} dashboard{}.",
                        if total == 1 { "" } else { "s" },
                        if dashboards == 1 { "" } else { "s" }
                    ),
                )
            }
            TerminalAction::SetTimezone { timezone } => {
                let timezone = timezone.trim().to_string();
                if timezone.is_empty() {
                    return UiActionResult::failure(name, "A timezone is required.");
                }
                self.patch_settings(|settings| settings.timezone = Some(timezone.clone()), cx);
                UiActionResult::success(name, format!("Timezone set to {timezone}."))
            }
            TerminalAction::SetTheme { theme, dark } => {
                let available = crate::data::theme::get_theme_names();
                let Some(theme_name) = available
                    .iter()
                    .find(|candidate| candidate.eq_ignore_ascii_case(theme))
                    .map(|candidate| (*candidate).to_string())
                else {
                    return UiActionResult::failure(
                        name,
                        format!(
                            "Unknown theme '{theme}'. Available: {}.",
                            available.join(", ")
                        ),
                    );
                };
                let dark_mode = dark.unwrap_or_else(|| {
                    self.user_settings
                        .as_ref()
                        .and_then(|settings| settings.dark_mode.as_deref())
                        .map(|mode| mode == "dark")
                        .unwrap_or(true)
                });
                crate::data::theme::apply_theme(&theme_name, dark_mode, cx);
                self.patch_settings(
                    |settings| {
                        settings.theme = Some(theme_name.clone());
                        settings.dark_mode =
                            Some(if dark_mode { "dark" } else { "light" }.to_string());
                    },
                    cx,
                );
                UiActionResult::success(name, format!("Theme set to {theme_name}."))
            }
            TerminalAction::SetAlertSound { enabled } => {
                self.patch_settings(|settings| settings.alert_sound_enabled = Some(*enabled), cx);
                UiActionResult::success(
                    name,
                    if *enabled {
                        "Alert sounds enabled."
                    } else {
                        "Alert sounds muted."
                    },
                )
            }
        }
    }

    /// True when a layout is currently owned by a detached window. Those must be
    /// managed from their own window; mutating or removing the file underneath
    /// them would desynchronize the open view.
    fn is_detached_dashboard(&self, path: &str, cx: &App) -> bool {
        cx.try_global::<crate::components::ui::pane_control::DashboardRegistry>()
            .map(|registry| {
                registry
                    .live()
                    .iter()
                    .any(|target| target.detached && target.id == path)
            })
            .unwrap_or(false)
    }

    /// The top-level window showing a detached dashboard, when one exists.
    fn detached_window(path: &str, cx: &App) -> Option<gpui::AnyWindowHandle> {
        cx.try_global::<crate::components::ui::pane_control::DashboardRegistry>()
            .and_then(|registry| registry.detached_window(path))
    }

    /// Resolve an Agent dashboard selector (`id`, name, path, or `active`) to a
    /// workspace-relative layout path.
    fn resolve_dashboard(
        &mut self,
        selector: Option<&str>,
        cx: &mut Context<Self>,
    ) -> std::result::Result<String, String> {
        let selector = selector.map(str::trim).filter(|value| !value.is_empty());
        let active = self
            .active_module
            .as_ref()
            .and_then(|module| module.id.clone());
        match selector {
            None | Some("active") | Some("current") => {
                active.ok_or_else(|| "No dashboard is open.".to_string())
            }
            Some("all") | Some("*") => Err(
                "This action targets a single dashboard; use a name or path instead of 'all'."
                    .to_string(),
            ),
            Some(wanted) => {
                if let Some(path) = self.find_dashboard_path(wanted) {
                    return Ok(path);
                }
                // A layout may have been created outside FinTer.
                self.reload_dashboards_from_store(cx);
                if let Some(path) = self.find_dashboard_path(wanted) {
                    return Ok(path);
                }
                let available = self
                    .dashboard_modules
                    .iter()
                    .filter_map(|module| module.id.clone())
                    .take(20)
                    .collect::<Vec<_>>()
                    .join(", ");
                Err(format!(
                    "No dashboard matches '{wanted}'. Known dashboards: {available}"
                ))
            }
        }
    }

    /// Find a layout path by exact id, exact name, or unique path suffix.
    fn find_dashboard_path(&self, wanted: &str) -> Option<String> {
        let wanted_lower = wanted.to_ascii_lowercase();
        if let Some(module) = self
            .dashboard_modules
            .iter()
            .find(|module| module.id.as_deref() == Some(wanted))
        {
            return module.id.clone();
        }

        let name_matches: Vec<&Module> = self
            .dashboard_modules
            .iter()
            .filter(|module| module.module_name.eq_ignore_ascii_case(wanted))
            .collect();
        if name_matches.len() == 1 {
            return name_matches[0].id.clone();
        }

        let suffix = format!("/{}", wanted_lower);
        let suffix_with_ext = format!("{suffix}.finter.json");
        let mut matches = Vec::new();
        for module in &self.dashboard_modules {
            let Some(id) = module.id.as_deref() else {
                continue;
            };
            let id_lower = id.to_ascii_lowercase();
            if id_lower == wanted_lower
                || id_lower.ends_with(&suffix)
                || id_lower.ends_with(&suffix_with_ext)
            {
                matches.push(id.to_string());
            }
        }
        if matches.len() == 1 {
            matches.pop()
        } else {
            None
        }
    }

    /// Every open dashboard, including detached windows, deduplicated by path.
    fn live_home_targets(&self, cx: &App) -> Vec<(String, Entity<HomePage>)> {
        let mut targets: Vec<(String, Entity<HomePage>)> = self
            .dashboard_views
            .iter()
            .map(|(id, home)| (id.clone(), home.clone()))
            .collect();
        if let (Some(home), Some(id)) = (
            &self.home_page,
            self.active_module
                .as_ref()
                .and_then(|module| module.id.clone()),
        ) {
            if !targets.iter().any(|(existing, _)| existing == &id) {
                targets.push((id, home.clone()));
            }
        }
        // Detached dashboards are not in `dashboard_views`; they register
        // themselves with the cross-window registry.
        if let Some(registry) =
            cx.try_global::<crate::components::ui::pane_control::DashboardRegistry>()
        {
            for target in registry.live() {
                if !target.detached {
                    continue;
                }
                if targets.iter().any(|(id, _)| id == &target.id) {
                    continue;
                }
                if let Some(home) = target.home.upgrade() {
                    targets.push((target.id, home));
                }
            }
        }
        targets
    }

    /// The HomePage for a layout path, whether it lives in a tab or a detached
    /// window.
    fn home_for_path(&self, path: &str, cx: &App) -> Option<Entity<HomePage>> {
        if let Some(home) = self.dashboard_views.get(path) {
            return Some(home.clone());
        }
        let active = self
            .active_module
            .as_ref()
            .and_then(|module| module.id.clone());
        if active.as_deref() == Some(path) {
            if let Some(home) = &self.home_page {
                return Some(home.clone());
            }
        }
        cx.try_global::<crate::components::ui::pane_control::DashboardRegistry>()?
            .live()
            .into_iter()
            .find(|target| target.id == path)
            .and_then(|target| target.home.upgrade())
    }

    /// Apply a settings patch to every live dashboard and persist it.
    fn patch_settings(&mut self, mutate: impl FnOnce(&mut SettingsData), cx: &mut Context<Self>) {
        let mut settings = self.user_settings.clone().unwrap_or_default();
        mutate(&mut settings);
        self.user_settings = Some(settings.clone());

        let homes = self.live_home_targets(cx);
        for (_, home) in homes {
            home.update(cx, |page, cx| {
                page.apply_settings(settings.clone(), cx);
            });
        }
        self.save_settings_to_api(settings, cx);
        cx.notify();
    }

    fn start_agent_context_refresh(&mut self, cx: &mut Context<Self>) {
        if self.agent_context_task.is_some() {
            return;
        }
        self.agent_context_task = Some(cx.spawn(async move |this, cx| {
            // Publish once immediately so a freshly created overlay composer is
            // not left with an empty snapshot for the first half second.
            let _ = this.update(cx, |view, cx| view.refresh_agent_context(cx));
            let mut last_credit_rev = crate::data::credits::CreditStore::revision();
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(500))
                    .await;
                let credit_rev = crate::data::credits::CreditStore::revision();
                let credit_changed = credit_rev != last_credit_rev;
                last_credit_rev = credit_rev;

                if this
                    .update(cx, |view, cx| {
                        if credit_changed {
                            cx.notify();
                        }
                        view.refresh_agent_context(cx);
                    })
                    .is_err()
                {
                    break;
                }
            }
        }));
    }

    /// Publish every open tabbed dashboard to the Agent's cross-window registry.
    ///
    /// Detached dashboards register themselves from their own window, so this
    /// only replaces the tabbed entries and leaves detached ones untouched.
    fn sync_dashboard_registry(&self, cx: &mut Context<Self>) {
        let mut entries: Vec<crate::components::ui::pane_control::DashboardTarget> = Vec::new();
        for (id, home) in &self.dashboard_views {
            if self.closed_dashboard_tabs.contains(id) {
                continue;
            }
            let name = self
                .dashboard_modules
                .iter()
                .find(|module| module.id.as_ref() == Some(id))
                .map(|module| module.module_name.clone())
                .unwrap_or_else(|| "Dashboard".to_string());
            entries.push(crate::components::ui::pane_control::DashboardTarget {
                id: id.clone(),
                name,
                detached: false,
                window: None,
                home: home.downgrade(),
            });
        }
        if let (Some(home), Some(active_id)) = (
            &self.home_page,
            self.active_module
                .as_ref()
                .and_then(|module| module.id.clone()),
        ) {
            if !entries.iter().any(|entry| entry.id == active_id) {
                let name = self
                    .active_module
                    .as_ref()
                    .map(|module| module.module_name.clone())
                    .unwrap_or_else(|| "Dashboard".to_string());
                entries.push(crate::components::ui::pane_control::DashboardTarget {
                    id: active_id,
                    name,
                    detached: false,
                    window: None,
                    home: home.downgrade(),
                });
            }
        }
        if !cx.has_global::<crate::components::ui::pane_control::DashboardRegistry>() {
            cx.set_global(crate::components::ui::pane_control::DashboardRegistry::default());
        }
        cx.update_global::<crate::components::ui::pane_control::DashboardRegistry, _>(
            |registry, _| registry.sync_main(entries),
        );
    }

    /// Share a bounded, read-only snapshot of all loaded/open dashboard layouts.
    fn refresh_agent_context(&mut self, cx: &mut Context<Self>) {
        self.sync_dashboard_registry(cx);
        let mut homes = self.dashboard_views.clone();
        if let (Some(home), Some(active_id)) = (
            &self.home_page,
            self.active_module
                .as_ref()
                .and_then(|module| module.id.clone()),
        ) {
            homes.entry(active_id).or_insert_with(|| home.clone());
        }
        // Detached dashboards live in their own windows but still belong in the
        // analysis snapshot: the Agent must see their layouts and market data to
        // reason about them, not just the pane catalog.
        if let Some(registry) =
            cx.try_global::<crate::components::ui::pane_control::DashboardRegistry>()
        {
            for target in registry.live() {
                if !target.detached {
                    continue;
                }
                if let Some(home) = target.home.upgrade() {
                    homes.entry(target.id).or_insert(home);
                }
            }
        }
        let dashboards: Vec<serde_json::Value> = homes.iter()
            .filter(|(id, _)| {
                !self.closed_dashboard_tabs.contains(*id)
                    || Self::detached_window(id, cx).is_some()
            })
            .map(|(id, home)| {
                let grid = home.read(cx).pane_grid.read(cx);
                let name = self.dashboard_modules.iter().find(|m| m.id.as_ref() == Some(id))
                    .map(|m| m.module_name.as_str()).unwrap_or("Dashboard");
                serde_json::json!({"id": id, "name": name, "layout": grid.tree, "market_data": grid.agent_market_snapshot(cx)})
            }).collect();
        // `control` is what the Agent addresses when it emits `ui` directives:
        // every open dashboard (tabs and detached windows) with per-pane refs.
        // It is deliberately tiny and kept under its own key so the context
        // truncator can preserve it while trimming market data.
        let catalog = crate::components::ui::pane_control::dashboard_catalog(cx);
        let active_path = self
            .active_module
            .as_ref()
            .and_then(|module| module.id.clone());
        let settings = self.user_settings.clone().unwrap_or_default();
        let saved_dashboards: Vec<serde_json::Value> = self
            .dashboard_modules
            .iter()
            .take(200)
            .map(|module| {
                let detached = module
                    .id
                    .as_deref()
                    .is_some_and(|id| Self::detached_window(id, cx).is_some());
                let open = module.id.as_deref().is_some_and(|id| {
                    self.dashboard_views.contains_key(id)
                        || active_path.as_deref() == Some(id)
                        || detached
                });
                serde_json::json!({
                    "id": module.id,
                    "name": module.module_name,
                    "open": open,
                    "detached": detached,
                })
            })
            .collect();
        let active_file = (self.current_page == Page::File)
            .then(|| self.active_file.clone())
            .flatten();
        let (active_file_content, active_file_truncated) =
            self.agent_file_snapshot(active_file.as_deref());
        let workspace_files = self.agent_workspace_files();
        let terminal = serde_json::json!({
            "app": {
                "authenticated": self.is_authenticated,
                "page": format!("{:?}", self.current_page).to_ascii_lowercase(),
                "active_dashboard": active_path,
                "active_file": active_file,
                "active_file_content": active_file_content,
                "active_file_truncated": active_file_truncated,
            },
            "settings": {
                "theme": settings.theme,
                "dark": settings.dark_mode.as_deref().map(|mode| mode == "dark"),
                "timezone": settings.timezone,
                "alert_sound": settings.alert_sound_enabled,
            },
            "saved_dashboards": saved_dashboards,
            "saved_dashboard_count": self.dashboard_modules.len(),
            "workspace_files": workspace_files,
            "limits": {
                "max_panes": crate::data::pane_layout::MAX_PANES,
            },
        });
        let context = serde_json::json!({
            "captured_at": chrono::Utc::now().to_rfc3339(),
            "control": catalog,
            "terminal": terminal,
            "dashboards": dashboards,
        });
        for home in homes.values() {
            let grid = home.read(cx).pane_grid.clone();
            grid.update(cx, |grid, cx| grid.set_agent_context(context.clone(), cx));
        }
        // Detached dashboards live in their own windows and are not in
        // `dashboard_views`, but their Agent panes need the same snapshot so a
        // conversation there can address every other dashboard too.
        let detached: Vec<_> = cx
            .try_global::<crate::components::ui::pane_control::DashboardRegistry>()
            .map(|registry| {
                registry
                    .live()
                    .into_iter()
                    .filter(|target| target.detached)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for target in detached {
            if let Some(home) = target.home.upgrade() {
                let grid = home.read(cx).pane_grid.clone();
                grid.update(cx, |grid, cx| grid.set_agent_context(context.clone(), cx));
            }
        }
        // The workspace-level Agent lives outside every dashboard, so it needs
        // the same snapshot pushed directly.
        self.agent_overlay_context = Some(context.clone());
        if let Some(panel) = self.agent_overlay_panel.clone() {
            panel.update(cx, |panel, _| panel.set_context(context));
        }
    }

    /// Bounded, cached text of the active file for the Agent snapshot.
    ///
    /// The snapshot is republished several times per second, so the file is only
    /// re-read when its length or modification time changes.
    fn agent_file_snapshot(&mut self, path: Option<&str>) -> (Option<String>, bool) {
        let Some(path) = path else {
            return (None, false);
        };
        if !crate::data::file_control::is_text_file_path(path) {
            return (None, false);
        }
        let Ok(absolute) = crate::data::layout_workspace::absolute_path(path) else {
            return (None, false);
        };
        let Ok(metadata) = std::fs::metadata(&absolute) else {
            return (None, false);
        };
        if !metadata.is_file() {
            return (None, false);
        }
        let len = metadata.len();
        let modified = metadata.modified().ok();

        let cache_hit = self.agent_file_snapshot.as_ref().is_some_and(|snapshot| {
            snapshot.path == path && snapshot.len == len && snapshot.modified == modified
        });
        if !cache_hit {
            let limit = crate::data::file_control::MAX_READ_BYTES;
            let (text, truncated) = read_agent_file(&absolute, limit);
            self.agent_file_snapshot = Some(AgentFileSnapshot {
                path: path.to_string(),
                len,
                modified,
                text,
                truncated,
            });
        }

        let snapshot = self.agent_file_snapshot.as_ref();
        (
            snapshot.and_then(|snapshot| snapshot.text.clone()),
            snapshot.is_some_and(|snapshot| snapshot.truncated),
        )
    }

    /// Text-file index advertised to the Agent so it can choose read targets.
    fn agent_workspace_files(&self) -> Vec<serde_json::Value> {
        fn walk(entries: &[LayoutEntry], out: &mut Vec<serde_json::Value>) {
            for entry in entries {
                if out.len() >= 300 {
                    return;
                }
                match entry.kind {
                    LayoutEntryKind::Folder => walk(&entry.children, out),
                    LayoutEntryKind::Layout => out.push(serde_json::json!({
                        "path": entry.path,
                        "kind": "layout",
                    })),
                    LayoutEntryKind::File => {
                        if crate::data::file_control::is_text_file_path(&entry.path) {
                            out.push(serde_json::json!({
                                "path": entry.path,
                                "kind": "text",
                            }));
                        }
                    }
                }
            }
        }

        let mut files = Vec::new();
        walk(&self.layout_entries, &mut files);
        files
    }

    pub fn api_client(&self) -> &ApiClient {
        &self.api_client
    }

    /// Get the title for the title bar (current layout name or default)
    pub fn title_bar_title(&self) -> String {
        if self.current_page == Page::File {
            if let Some(title) = &self.file_editor_title {
                return title.clone();
            }
        }
        self.active_module
            .as_ref()
            .map(|m| m.module_name.clone())
            .unwrap_or_else(|| "FinTer".to_string())
    }

    /// Build the title-bar tab strip for a single-group workspace.
    fn dashboard_tabs(
        &self,
        entity: gpui::WeakEntity<Self>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.render_group_tabs(0, entity, cx, false)
    }

    /// Split-mode title row: the window chrome and both pane tab strips share
    /// one row. Each strip aligns with its pane below.
    fn render_split_title_row(
        &self,
        entity: gpui::WeakEntity<Self>,
        workspace_width: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = cx.theme().colors;
        let ratio = clamp_split_ratio(self.split_ratio);
        let left_width = ((workspace_width - 12.0).max(1.0) * ratio).max(1.0);
        let mac_pad =
            (f32::from(crate::components::ui::title_bar::tab_strip_left_padding()) - 6.0).max(0.0);

        let left_strip = div()
            .flex_shrink()
            .h_full()
            .w(px(left_width))
            .min_w(px(0.0))
            .overflow_hidden()
            .when(mac_pad > 0.0, |this| this.pl(px(mac_pad)))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _, _, cx| this.focus_group(0, cx)),
            )
            .child(self.render_group_tabs(0, entity.clone(), cx, true));

        let right_strip = div()
            .h_full()
            .flex_1()
            .min_w(px(0.0))
            .overflow_hidden()
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _, _, cx| this.focus_group(1, cx)),
            )
            .child(self.render_group_tabs(1, entity, cx, true));

        div()
            .id("workspace-split-title-row")
            .h(crate::components::ui::title_bar::TITLE_BAR_HEIGHT)
            .flex_shrink_0()
            .w_full()
            .flex()
            .pl(px(6.0))
            .bg(colors.title_bar)
            .border_b_1()
            .border_color(colors.border)
            .child(left_strip)
            .child(div().flex_shrink_0().w(px(8.0)).h_full())
            .child(right_strip)
            .child(crate::components::ui::title_bar::render_window_controls())
            .into_any_element()
    }

    /// Draggable empty area in the split title row.
    fn title_row_drag_spacer(group: usize) -> AnyElement {
        div()
            .id(gpui::ElementId::Name(
                format!("split-title-row-drag-{group}").into(),
            ))
            .flex_1()
            .h_full()
            .min_w(px(12.0))
            .when(!cfg!(target_os = "linux"), |this| {
                this.window_control_area(WindowControlArea::Drag)
            })
            .when(cfg!(target_os = "linux"), |this| {
                this.on_mouse_down(gpui::MouseButton::Left, |_, window, _| {
                    window.start_window_move();
                })
            })
            .on_click(|event: &gpui::ClickEvent, window, _| {
                if event.click_count() == 2 {
                    window.zoom_window();
                }
            })
            .into_any_element()
    }

    /// Build the tab strip for one editor group.
    fn render_group_tabs(
        &self,
        group: usize,
        entity: gpui::WeakEntity<Self>,
        cx: &mut Context<Self>,
        extend_drag: bool,
    ) -> impl IntoElement {
        let active = self.group_active[group].clone().or_else(|| {
            if group != 0 {
                return None;
            }
            if self.current_page == Page::File {
                self.active_file.clone().map(WorkspaceTab::File)
            } else {
                self.active_module
                    .as_ref()
                    .and_then(|module| module.id.clone())
                    .map(WorkspaceTab::Dashboard)
            }
        });
        let active_id = match &active {
            Some(WorkspaceTab::Dashboard(id)) => Some(id.clone()),
            _ => None,
        };
        let active_file = match &active {
            Some(WorkspaceTab::File(path)) => Some(path.clone()),
            _ => None,
        };

        let children: Vec<AnyElement> = self
            .group_tabs(group)
            .to_vec()
            .into_iter()
            .filter_map(|tab| match tab {
                WorkspaceTab::Dashboard(id) => {
                    let module = self
                        .dashboard_modules
                        .iter()
                        .find(|module| module.id.as_deref() == Some(id.as_str()))?
                        .clone();
                    Some(self.render_dashboard_tab(
                        &module,
                        active_id.as_deref() == Some(id.as_str()),
                        group,
                        entity.clone(),
                        cx,
                    ))
                }
                WorkspaceTab::File(path) => {
                    if !self.file_tabs.contains(&path) {
                        return None;
                    }
                    let dirty = self
                        .file_editors
                        .get(&path)
                        .is_some_and(|editor| editor.read(cx).is_dirty());
                    let is_active = active_file.as_deref() == Some(path.as_str());
                    Some(self.render_file_tab(path, is_active, dirty, group, entity.clone(), cx))
                }
            })
            .collect();

        let colors = cx.theme().colors;
        let list_hover = colors.list_hover;
        let plus_entity = entity.clone();
        let end_drop_entity = entity.clone();
        let split = self.split_visible();
        let mono_font = cx.theme().mono_font_family.clone();
        let focused_group = self.focused_group == group;

        div()
            .id(gpui::ElementId::Name(
                format!("dashboard-tabs-{group}").into(),
            ))
            .flex()
            .items_end()
            .h_full()
            .gap(px(0.0))
            .overflow_hidden()
            .on_drag_move::<DragWorkspaceTab>(cx.listener(
                |this, _event: &gpui::DragMoveEvent<DragWorkspaceTab>, _, cx| {
                    if this.edge_hover.is_some() {
                        this.edge_hover = None;
                        cx.notify();
                    }
                },
            ))
            .when(split, |this| {
                this.child(
                    div()
                        .flex_shrink_0()
                        .h_full()
                        .px(px(8.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .border_r_1()
                        .border_color(colors.border)
                        .text_size(px(9.0))
                        .font_family(mono_font)
                        .text_color(if focused_group {
                            colors.primary
                        } else {
                            colors.muted_foreground
                        })
                        .child(format!("{:02}", group + 1)),
                )
            })
            .children(children)
            .child(
                div()
                    .id(gpui::ElementId::Name(
                        format!("dashboard-tabs-end-drop-{group}").into(),
                    ))
                    .flex_shrink_0()
                    .w(px(18.0))
                    .h_full()
                    .drag_over::<DragWorkspaceTab>(move |style, _, _, _| style.bg(list_hover))
                    .on_drop(move |drag: &DragWorkspaceTab, _window, app| {
                        if let Some(view) = end_drop_entity.upgrade() {
                            let dragged = drag.tab.clone();
                            app.update_entity(&view, |view, cx| {
                                view.move_workspace_tab_to_end(dragged, group, cx);
                            });
                        }
                    }),
            )
            .when(extend_drag, |this| {
                this.child(Self::title_row_drag_spacer(group))
            })
            .child(
                div()
                    .id(gpui::ElementId::Name(
                        format!("add-dashboard-tab-btn-{group}").into(),
                    ))
                    .flex()
                    .items_center()
                    .justify_center()
                    .h_full()
                    .w(px(28.0))
                    .flex_shrink_0()
                    .cursor_pointer()
                    .text_color(colors.muted_foreground)
                    .hover(move |style| style.bg(list_hover).text_color(colors.foreground))
                    .on_click(move |_event, _window, app| {
                        if let Some(view) = plus_entity.upgrade() {
                            app.update_entity(&view, |view, cx| {
                                view.focus_group(group, cx);
                                view.create_new_dashboard(
                                    String::new(),
                                    "Untitled Layout".to_string(),
                                    cx,
                                );
                            });
                        }
                    })
                    .child(
                        gpui_component::Icon::default()
                            .path("icons/plus.svg")
                            .xsmall(),
                    ),
            )
    }

    fn render_dashboard_tab(
        &self,
        module: &Module,
        is_active: bool,
        group: usize,
        entity: gpui::WeakEntity<Self>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = cx.theme().colors;
        let list_hover = colors.list_hover;
        let primary = colors.primary;
        let accent = if self.focused_group == group {
            primary
        } else {
            primary.opacity(0.5)
        };
        let bg_active = colors.background;
        let fg_active = colors.foreground;
        let fg_muted = colors.muted_foreground;
        let sec_hover = colors.secondary_hover;

        let Some(id) = module.id.clone() else {
            return div().into_any_element();
        };
        let name = module.module_name.clone();
        let drag_name = name.clone();
        let load_id = id.clone();
        let close_id = id.clone();
        let load_entity = entity.clone();
        let middle_close_entity = entity.clone();
        let middle_close_id = id.clone();
        let close_entity = entity.clone();
        let detach_entity = entity.clone();
        let detach_id = id.clone();
        let drop_entity = entity.clone();
        let drop_target = WorkspaceTab::Dashboard(id.clone());

        div()
            .id(gpui::ElementId::Name(gpui::SharedString::from(format!(
                "dashboard-tab-{id}"
            ))))
            .flex()
            .items_center()
            .gap(px(6.0))
            .h_full()
            .w(px(172.0))
            .flex_shrink_0()
            .px(px(10.0))
            .border_b_2()
            .border_color(if is_active {
                accent
            } else {
                gpui::transparent_black()
            })
            .bg(if is_active {
                bg_active
            } else {
                gpui::transparent_black()
            })
            .text_color(if is_active { fg_active } else { fg_muted })
            .cursor_pointer()
            .hover(move |style| {
                if !is_active {
                    style.bg(list_hover)
                } else {
                    style
                }
            })
            .drag_over::<DragWorkspaceTab>(move |style, _, _, _| style.bg(list_hover))
            .on_drop(move |drag: &DragWorkspaceTab, _window, app| {
                if let Some(view) = drop_entity.upgrade() {
                    let dragged = drag.tab.clone();
                    let target = drop_target.clone();
                    app.update_entity(&view, |view, cx| {
                        view.reorder_workspace_tab(dragged, target, group, cx);
                    });
                }
            })
            .on_drag(
                DragWorkspaceTab {
                    tab: WorkspaceTab::Dashboard(id.clone()),
                },
                move |_drag, _offset, _window, app| {
                    let name = drag_name.clone();
                    app.new(|_| TabDragPreview(name))
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
                    .text_color(if is_active { accent } else { fg_muted }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_xs()
                    .text_ellipsis()
                    .font_weight(if is_active {
                        gpui::FontWeight::MEDIUM
                    } else {
                        gpui::FontWeight::NORMAL
                    })
                    .child(name),
            )
            .child(
                div()
                    .id(gpui::ElementId::Name(gpui::SharedString::from(format!(
                        "detach-dashboard-tab-{detach_id}"
                    ))))
                    .w(px(18.0))
                    .h(px(18.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .opacity(0.55)
                    .hover(move |style| style.bg(sec_hover).text_color(fg_active).opacity(1.0))
                    .text_color(fg_muted)
                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(move |_event, _window, app| {
                        if let Some(view) = detach_entity.upgrade() {
                            app.update_entity(&view, |view, cx| {
                                view.detach_dashboard(detach_id.clone(), cx);
                            });
                        }
                    })
                    .child(
                        gpui_component::Icon::default()
                            .path("icons/external-link.svg")
                            .xsmall(),
                    ),
            )
            .child(
                div()
                    .id(gpui::ElementId::Name(gpui::SharedString::from(format!(
                        "close-dashboard-tab-{close_id}"
                    ))))
                    .w(px(18.0))
                    .h(px(18.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .opacity(0.55)
                    .hover(move |style| style.bg(sec_hover).text_color(fg_active).opacity(1.0))
                    .text_color(fg_muted)
                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(move |_event, _window, app| {
                        if let Some(view) = close_entity.upgrade() {
                            app.update_entity(&view, |view, cx| {
                                view.close_dashboard_tab(close_id.clone(), cx);
                            });
                        }
                    })
                    .child(
                        gpui_component::Icon::default()
                            .path("icons/window-close.svg")
                            .xsmall(),
                    ),
            )
            .into_any_element()
    }

    fn render_file_tab(
        &self,
        path: String,
        is_active: bool,
        dirty: bool,
        group: usize,
        entity: gpui::WeakEntity<Self>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = cx.theme().colors;
        let list_hover = colors.list_hover;
        let primary = colors.primary;
        let accent = if self.focused_group == group {
            primary
        } else {
            primary.opacity(0.5)
        };
        let bg_active = colors.background;
        let fg_active = colors.foreground;
        let fg_muted = colors.muted_foreground;
        let sec_hover = colors.secondary_hover;

        let name = std::path::Path::new(&path)
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| path.clone());
        let drag_name = name.clone();
        let activate_path = path.clone();
        let close_path = path.clone();
        let close_click_path = path.clone();
        let activate_entity = entity.clone();
        let close_entity = entity.clone();
        let close_click_entity = entity.clone();
        let drop_entity = entity.clone();
        let drop_target = WorkspaceTab::File(path.clone());

        div()
            .id(gpui::ElementId::Name(gpui::SharedString::from(format!(
                "file-tab-{path}"
            ))))
            .flex()
            .items_center()
            .gap(px(6.0))
            .h_full()
            .w(px(172.0))
            .flex_shrink_0()
            .px(px(10.0))
            .border_b_2()
            .border_color(if is_active {
                accent
            } else {
                gpui::transparent_black()
            })
            .bg(if is_active {
                bg_active
            } else {
                gpui::transparent_black()
            })
            .text_color(if is_active { fg_active } else { fg_muted })
            .cursor_pointer()
            .hover(move |style| {
                if !is_active {
                    style.bg(list_hover)
                } else {
                    style
                }
            })
            .drag_over::<DragWorkspaceTab>(move |style, _, _, _| style.bg(list_hover))
            .on_drop(move |drag: &DragWorkspaceTab, _window, app| {
                if let Some(view) = drop_entity.upgrade() {
                    let dragged = drag.tab.clone();
                    let target = drop_target.clone();
                    app.update_entity(&view, |view, cx| {
                        view.reorder_workspace_tab(dragged, target, group, cx);
                    });
                }
            })
            .on_drag(
                DragWorkspaceTab {
                    tab: WorkspaceTab::File(path.clone()),
                },
                move |_drag, _offset, _window, app| {
                    let name = drag_name.clone();
                    app.new(|_| TabDragPreview(name))
                },
            )
            .on_mouse_down(gpui::MouseButton::Middle, move |_event, _window, app| {
                if let Some(view) = close_entity.upgrade() {
                    app.update_entity(&view, |view, cx| {
                        view.close_file_tab(close_path.clone(), cx);
                    });
                }
            })
            .on_click(move |_event, _window, app| {
                if let Some(view) = activate_entity.upgrade() {
                    app.update_entity(&view, |view, cx| {
                        view.activate_file_tab(activate_path.clone(), cx);
                    });
                }
            })
            .child(
                gpui_component::Icon::default()
                    .path("icons/file.svg")
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
                    .font_weight(if is_active {
                        gpui::FontWeight::MEDIUM
                    } else {
                        gpui::FontWeight::NORMAL
                    })
                    .child(name),
            )
            .when(dirty, |this| {
                this.child(
                    div()
                        .flex_shrink_0()
                        .text_size(px(9.0))
                        .text_color(primary)
                        .child("●"),
                )
            })
            .child(
                div()
                    .id(gpui::ElementId::Name(gpui::SharedString::from(format!(
                        "close-file-tab-{path}"
                    ))))
                    .w(px(18.0))
                    .h(px(18.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .opacity(0.55)
                    .hover(move |style| style.bg(sec_hover).text_color(fg_active).opacity(1.0))
                    .text_color(fg_muted)
                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(move |_event, _window, app| {
                        if let Some(view) = close_click_entity.upgrade() {
                            app.update_entity(&view, |view, cx| {
                                view.close_file_tab(close_click_path.clone(), cx);
                            });
                        }
                    })
                    .child(
                        gpui_component::Icon::default()
                            .path("icons/window-close.svg")
                            .xsmall(),
                    ),
            )
            .into_any_element()
    }

    /// Two editor groups side by side on a workbench, with a styled divider.
    fn render_split_workspace(
        &mut self,
        entity: gpui::WeakEntity<Self>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = cx.theme().colors;
        let ratio = clamp_split_ratio(self.split_ratio);
        let grip = colors.primary;
        let left = self.render_editor_group(0, entity.clone(), cx);
        let right = self.render_editor_group(1, entity.clone(), cx);

        let divider = div()
            .id("workspace-split-divider")
            .flex_shrink_0()
            .w(px(8.0))
            .h_full()
            .cursor(gpui::CursorStyle::ResizeLeftRight)
            .flex()
            .items_center()
            .justify_center()
            .on_click(cx.listener(|this, event: &gpui::ClickEvent, _, cx| {
                if event.click_count() == 2 {
                    this.unsplit_workspace(cx);
                }
            }))
            .on_drag(SplitDrag, |_, _, _, app| app.new(|_| SplitDragPreview))
            .child(
                div()
                    .w(px(2.0))
                    .h(px(48.0))
                    .bg(colors.border)
                    .hover(move |style| style.bg(grip).w(px(3.0))),
            );

        div()
            .id("workspace-groups")
            .size_full()
            .flex()
            .flex_row()
            .p(px(6.0))
            .bg(colors.title_bar)
            .on_drag_move::<SplitDrag>(cx.listener(
                |this, event: &gpui::DragMoveEvent<SplitDrag>, _, cx| {
                    let width = f32::from(event.bounds.size.width);
                    if width <= 1.0 {
                        return;
                    }
                    let x = f32::from(event.event.position.x - event.bounds.left());
                    let ratio = clamp_split_ratio(x / width);
                    if (this.split_ratio - ratio).abs() > 0.001 {
                        this.split_ratio = ratio;
                        cx.notify();
                    }
                },
            ))
            .child(div().h_full().w(relative(ratio)).child(left))
            .child(divider)
            .child(div().h_full().min_w(px(0.0)).flex_1().child(right))
            .with_animation(
                "workspace-groups-in",
                Animation::new(Duration::from_millis(160)).with_easing(ease_out_quint()),
                |this, delta| this.opacity(delta),
            )
            .into_any_element()
    }

    /// One editor group: a machined panel with its active dashboard/file and
    /// edge drop zones for splitting.
    fn render_editor_group(
        &mut self,
        group: usize,
        entity: gpui::WeakEntity<Self>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = cx.theme().colors;
        let mono = cx.theme().mono_font_family.clone();
        let focused = self.focused_group == group;
        let active = self.group_active[group].clone();

        let content: AnyElement = match active {
            Some(WorkspaceTab::Dashboard(id)) => {
                if let Some(home) = self.ensure_dashboard_home(&id, cx) {
                    let active_home = home.downgrade();
                    div()
                        .size_full()
                        .on_mouse_down(gpui::MouseButton::Left, move |_event, _window, cx| {
                            cx.set_global(crate::components::pages::home::GlobalHomeView {
                                view: active_home.clone(),
                            });
                        })
                        .child(home)
                        .into_any_element()
                } else {
                    Self::editor_group_placeholder("Dashboard unavailable", &colors)
                }
            }
            Some(WorkspaceTab::File(path)) => {
                if let Some(editor) = self.ensure_file_editor(path, cx) {
                    div()
                        .flex()
                        .flex_col()
                        .size_full()
                        .min_h(px(0.0))
                        .child(editor)
                        .into_any_element()
                } else {
                    Self::editor_group_placeholder("File unavailable", &colors)
                }
            }
            None => Self::editor_group_placeholder("Drag a tab here", &colors),
        };

        let drag_active = cx.has_active_drag();
        let hover_left = drag_active && self.edge_hover == Some((group, true));
        let hover_right = drag_active && self.edge_hover == Some((group, false));

        let mut content_wrapper = div()
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .min_w(px(0.0))
            .overflow_hidden()
            .on_drag_move::<DragWorkspaceTab>(cx.listener(
                move |this, event: &gpui::DragMoveEvent<DragWorkspaceTab>, _, cx| {
                    this.track_edge_hover(group, event, cx);
                },
            ))
            .child(content);
        if hover_left {
            content_wrapper = content_wrapper.child(Self::split_ghost(
                group,
                true,
                entity.clone(),
                &colors,
                &mono,
            ));
        }
        if hover_right {
            content_wrapper = content_wrapper.child(Self::split_ghost(
                group,
                false,
                entity.clone(),
                &colors,
                &mono,
            ));
        }

        let focus_entity = entity.clone();
        div()
            .id(gpui::ElementId::Name(
                format!("editor-group-{group}").into(),
            ))
            .flex()
            .flex_col()
            .size_full()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .bg(colors.background)
            .border_1()
            .border_color(if focused {
                colors.primary.opacity(0.55)
            } else {
                colors.border
            })
            .overflow_hidden()
            .on_mouse_down(gpui::MouseButton::Left, move |_event, _window, app| {
                if let Some(view) = focus_entity.upgrade() {
                    app.update_entity(&view, |view, cx| view.focus_group(group, cx));
                }
            })
            .child(content_wrapper)
            .child(Self::render_group_edge_zone_el(
                group,
                true,
                entity.clone(),
                &colors,
            ))
            .child(Self::render_group_edge_zone_el(
                group, false, entity, &colors,
            ))
            .into_any_element()
    }

    fn editor_group_placeholder(
        message: &str,
        colors: &gpui_component::theme::ThemeColor,
    ) -> AnyElement {
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .text_sm()
            .text_color(colors.muted_foreground)
            .child(message.to_string())
            .into_any_element()
    }

    /// Ghost panel shown while a tab is dragged near a group edge.
    fn split_ghost(
        group: usize,
        left: bool,
        entity: gpui::WeakEntity<Self>,
        colors: &gpui_component::theme::ThemeColor,
        mono: &gpui::SharedString,
    ) -> AnyElement {
        let accent = colors.primary;
        div()
            .id(gpui::ElementId::Name(
                format!("split-ghost-{group}-{left}").into(),
            ))
            .absolute()
            .top(px(0.0))
            .bottom(px(0.0))
            .when(left, |this| this.left(px(0.0)))
            .when(!left, |this| this.right(px(0.0)))
            .w(relative(0.45))
            .bg(accent.opacity(0.10))
            .border_1()
            .border_color(accent.opacity(0.55))
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(9.0))
            .font_family(mono.clone())
            .text_color(accent)
            .child("DROP TO SPLIT")
            .on_drop(move |drag: &DragWorkspaceTab, _window, app| {
                if let Some(view) = entity.upgrade() {
                    let dragged = drag.tab.clone();
                    app.update_entity(&view, |view, cx| {
                        view.edge_hover = None;
                        view.split_or_move_to_edge(dragged, group, left, cx);
                    });
                }
            })
            .into_any_element()
    }

    fn render_group_edge_zone_el(
        group: usize,
        left: bool,
        entity: gpui::WeakEntity<Self>,
        colors: &gpui_component::theme::ThemeColor,
    ) -> AnyElement {
        let highlight = colors.primary.opacity(0.16);
        div()
            .id(gpui::ElementId::Name(
                format!("editor-group-edge-{group}-{left}").into(),
            ))
            .absolute()
            .top(px(0.0))
            .bottom(px(0.0))
            .when(left, |this| this.left(px(0.0)))
            .when(!left, |this| this.right(px(0.0)))
            .w(px(8.0))
            .drag_over::<DragWorkspaceTab>(move |style, _, _, _| style.bg(highlight))
            .on_drop(move |drag: &DragWorkspaceTab, _window, app| {
                if let Some(view) = entity.upgrade() {
                    let dragged = drag.tab.clone();
                    app.update_entity(&view, |view, cx| {
                        view.edge_hover = None;
                        view.split_or_move_to_edge(dragged, group, left, cx);
                    });
                }
            })
            .into_any_element()
    }

    /// Edge drop zone for the single-group workspace. Dropping here creates the
    /// first split.
    fn render_single_edge_zone(
        left: bool,
        entity: gpui::WeakEntity<Self>,
        colors: &gpui_component::theme::ThemeColor,
    ) -> AnyElement {
        let highlight = colors.primary.opacity(0.16);
        div()
            .id(gpui::ElementId::Name(
                format!("workspace-split-edge-{left}").into(),
            ))
            .absolute()
            .top(px(0.0))
            .bottom(px(0.0))
            .when(left, |this| this.left(px(0.0)))
            .when(!left, |this| this.right(px(0.0)))
            .w(px(10.0))
            .drag_over::<DragWorkspaceTab>(move |style, _, _, _| style.bg(highlight))
            .on_drop(move |drag: &DragWorkspaceTab, _window, app| {
                if let Some(view) = entity.upgrade() {
                    let dragged = drag.tab.clone();
                    app.update_entity(&view, |view, cx| {
                        view.split_or_move_to_edge(dragged, 0, left, cx);
                    });
                }
            })
            .into_any_element()
    }

    /// Open a detached dashboard window. The dashboard is loaded from the cached
    /// module payload; it is not removed from the main tab strip.
    fn detach_dashboard(&mut self, module_id: String, cx: &mut Context<Self>) {
        let Some(module) = self
            .dashboard_modules
            .iter()
            .find(|m| m.id.as_ref() == Some(&module_id))
            .cloned()
        else {
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
                        app_id: Some("ch.coinwat.finter".to_string()),
                        icon: crate::data::app_icon::png().map(|bytes| bytes.to_vec()),
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
                    // Register this detached dashboard so an Agent pane in any
                    // window can address it by id or name.
                    let registry_name = title.clone();
                    let registry_id = window_module_id.clone();
                    if !cx.has_global::<crate::components::ui::pane_control::DashboardRegistry>() {
                        cx.set_global(crate::components::ui::pane_control::DashboardRegistry::default());
                    }
                    cx.update_global::<crate::components::ui::pane_control::DashboardRegistry, _>(
                        |registry, _| {
                            registry.register(crate::components::ui::pane_control::DashboardTarget {
                                id: registry_id.clone(),
                                name: registry_name.clone(),
                                detached: true,
                                window: Some(window.window_handle()),
                                home: home.downgrade(),
                            });
                        },
                    );
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

    /// Merge a split back into one group; the focused group's tabs stay in
    /// order and the title-bar strip returns.
    fn unsplit_workspace(&mut self, cx: &mut Context<Self>) {
        if self.split_at.is_none() {
            return;
        }
        self.edge_hover = None;
        let previous_active = self.group_active[self.focused_group].clone();
        self.split_at = None;
        self.focused_group = 0;
        self.group_active[1] = None;
        self.group_active[0] = previous_active
            .filter(|tab| self.workspace_tabs.contains(tab))
            .or_else(|| self.workspace_tabs.first().cloned());
        let active = self.group_active[0].clone();
        self.show_active_tab(active, cx);
        self.persist_open_tabs();
    }

    /// Move a dragged tab immediately before the target tab in `target_group`.
    fn reorder_workspace_tab(
        &mut self,
        dragged: WorkspaceTab,
        target: WorkspaceTab,
        target_group: usize,
        cx: &mut Context<Self>,
    ) {
        if dragged == target {
            return;
        }
        self.edge_hover = None;
        let Some(source_group) = self.group_of_tab(&dragged) else {
            return;
        };
        let Some(target_index) = self
            .group_tabs(target_group)
            .iter()
            .position(|tab| *tab == target)
        else {
            return;
        };
        let insert_index = if source_group == target_group {
            let source_index = self
                .group_tabs(source_group)
                .iter()
                .position(|tab| *tab == dragged)
                .unwrap_or(0);
            if source_index < target_index {
                target_index - 1
            } else {
                target_index
            }
        } else {
            target_index
        };
        if self
            .move_tab_to_group_at(&dragged, target_group, Some(insert_index))
            .is_none()
        {
            return;
        }
        self.sync_file_tabs_order();
        self.reorder_dashboards_from_tabs();
        if source_group != target_group {
            let target = target_group.min(self.group_count().saturating_sub(1));
            self.focused_group = target;
            self.group_active[target] = Some(dragged.clone());
            self.show_active_tab(Some(dragged), cx);
        } else {
            cx.notify();
        }
        self.persist_open_tabs();
    }

    /// Move a dragged tab to the end of its target group's strip.
    fn move_workspace_tab_to_end(
        &mut self,
        dragged: WorkspaceTab,
        target_group: usize,
        cx: &mut Context<Self>,
    ) {
        self.edge_hover = None;
        let Some(source_group) = self.group_of_tab(&dragged) else {
            return;
        };
        if self
            .move_tab_to_group_at(&dragged, target_group, None)
            .is_none()
        {
            return;
        }
        self.sync_file_tabs_order();
        self.reorder_dashboards_from_tabs();
        if source_group != target_group {
            let target = target_group.min(self.group_count().saturating_sub(1));
            self.focused_group = target;
            self.group_active[target] = Some(dragged.clone());
            self.show_active_tab(Some(dragged), cx);
        } else {
            cx.notify();
        }
        self.persist_open_tabs();
    }

    /// Drop a tab on the left or right edge of a group. With one group this
    /// creates the horizontal split; with two groups it moves the tab into the
    /// target group.
    fn split_or_move_to_edge(
        &mut self,
        dragged: WorkspaceTab,
        target_group: usize,
        side_left: bool,
        cx: &mut Context<Self>,
    ) {
        self.edge_hover = None;
        let Some(source_group) = self.group_of_tab(&dragged) else {
            return;
        };

        if self.split_visible() {
            if source_group == target_group {
                return;
            }
            let index = if side_left { Some(0) } else { None };
            if self
                .move_tab_to_group_at(&dragged, target_group, index)
                .is_none()
            {
                return;
            }
            self.sync_file_tabs_order();
            self.reorder_dashboards_from_tabs();
            let target = target_group.min(self.group_count().saturating_sub(1));
            self.focused_group = target;
            self.group_active[target] = Some(dragged.clone());
            self.show_active_tab(Some(dragged), cx);
            self.persist_open_tabs();
            return;
        }

        // Single group: the dragged tab becomes a new group on that side.
        let (mut tabs, _) = self.split_group_vecs();
        let Some(index) = tabs.iter().position(|tab| *tab == dragged) else {
            return;
        };
        let tab = tabs.remove(index);
        if tabs.is_empty() {
            return;
        }
        let previous_active = self.group_active[0].clone();
        let (left, right, focused) = if side_left {
            (vec![tab.clone()], tabs, 0)
        } else {
            (tabs, vec![tab.clone()], 1)
        };
        self.workspace_tabs = left;
        self.split_at = Some(self.workspace_tabs.len());
        self.workspace_tabs.extend(right);
        self.split_ratio = 0.5;
        self.group_active[0] = None;
        self.group_active[1] = None;
        if side_left {
            self.group_active[0] = Some(tab.clone());
            self.group_active[1] = previous_active
                .filter(|active| self.group_tabs(1).contains(active))
                .or_else(|| self.group_tabs(1).first().cloned());
        } else {
            self.group_active[1] = Some(tab.clone());
            self.group_active[0] = previous_active
                .filter(|active| self.group_tabs(0).contains(active))
                .or_else(|| self.group_tabs(0).first().cloned());
        }
        self.focused_group = focused;
        self.sync_file_tabs_order();
        self.reorder_dashboards_from_tabs();
        self.show_active_tab(Some(tab), cx);
        self.persist_open_tabs();
    }

    /// Close a title-bar tab without deleting the saved dashboard.
    fn close_dashboard_tab(&mut self, module_id: String, cx: &mut Context<Self>) {
        self.closed_dashboard_tabs.insert(module_id.clone());
        self.dashboard_views.remove(&module_id);
        if cx.has_global::<crate::components::ui::pane_control::DashboardRegistry>() {
            cx.update_global::<crate::components::ui::pane_control::DashboardRegistry, _>(
                |registry, _| registry.remove_tabbed(&module_id),
            );
        }
        let tab = WorkspaceTab::Dashboard(module_id.clone());
        let previous_focused = self.focused_group;
        let previous_active = self.group_active[previous_focused].clone();
        self.remove_tab_everywhere(&tab);
        self.ensure_group_actives();
        self.reorder_dashboards_from_tabs();
        if self.focused_group != previous_focused
            || self.group_active[self.focused_group] != previous_active
        {
            let active = self.group_active[self.focused_group].clone();
            self.show_active_tab(active, cx);
        }
        self.persist_open_tabs();
        self.sync_manager_open_tabs(cx);
        cx.notify();
    }

    /// Drop stale view state before an account switch. The layout workspace
    /// itself is local and is reloaded from disk afterwards.
    fn reset_workspace_for_account_switch(&mut self, cx: &mut Context<Self>) {
        self.active_module = None;
        self.home_page = None;
        self.pending_module_load = None;
        self.dashboard_views.clear();
        self.closed_dashboard_tabs.clear();
        self.layout_cache.clear();
        self.split_at = None;
        self.group_active = [None, None];
        self.focused_group = 0;
        // Layouts are local workspace state, so account changes do not discard
        // them. The API origin is a client-local network setting, so keep it
        // across account switches.
        let local_server_url = self
            .user_settings
            .as_ref()
            .and_then(|settings| settings.server_url.as_deref())
            .and_then(crate::data::client::normalize_api_origin);
        self.user_settings = local_server_url.map(|server_url| SettingsData {
            server_url: Some(server_url),
            ..Default::default()
        });
        if let Some(panel) = &self.alerts_panel {
            panel.update(cx, |panel, cx| panel.clear(cx));
        }
    }

    /// Handle successful login: switch to home with trading view
    fn on_logged_in(&mut self, cx: &mut Context<Self>) {
        self.is_authenticated = true;
        self.current_page = Page::Home;
        self.is_restoring = true;
        // Rebuild the settings page with the cloud account's preferences.
        self.settings_page = None;
        self.reset_workspace_for_account_switch(cx);

        let token = self.api_client.access_token();
        crate::data::gateway_registry::GatewayRegistry::global()
            .refresh_for_session(token.as_deref());

        // Load account settings, then restore the local layout workspace
        self.load_settings_then_modules(cx);
        self.load_account_summary(cx);
        self.ensure_alerts_panel(cx);
        if let Some(panel) = &self.alerts_panel {
            panel.update(cx, |panel, cx| panel.refresh(cx));
        }

        cx.notify();
    }

    /// Handle successful signup: create default module and switch to home
    fn on_signed_up(&mut self, cx: &mut Context<Self>) {
        self.is_authenticated = true;
        self.current_page = Page::Home;
        self.is_restoring = true;
        // Rebuild the settings page with the new account's preferences.
        self.settings_page = None;
        self.reset_workspace_for_account_switch(cx);

        let token = self.api_client.access_token();
        crate::data::gateway_registry::GatewayRegistry::global()
            .refresh_for_session(token.as_deref());

        // Create default module for new user, then load modules
        self.create_default_module_then_load(cx);
        self.ensure_alerts_panel(cx);
        if let Some(panel) = &self.alerts_panel {
            panel.update(cx, |panel, cx| panel.refresh(cx));
        }

        cx.notify();
    }

    /// Rebuild the workspace after signup. Layouts are local, so there is no
    /// server default to create.
    fn create_default_module_then_load(&mut self, cx: &mut Context<Self>) {
        self.restore_local_workspace(cx);
        self.is_restoring = false;
        cx.notify();
    }

    /// Create a new layout inside `parent` (empty means the workspace root).
    fn create_new_dashboard(&mut self, parent: String, name: String, cx: &mut Context<Self>) {
        let document = match crate::data::layout_workspace::create_layout(&parent, &name) {
            Ok(document) => document,
            Err(error) => {
                tracing::error!(?error, "Failed to create layout {name}");
                if let Some(manager) = &self.dashboard_manager {
                    manager.update(cx, |manager, cx| {
                        manager.update_error(error.to_string(), cx);
                    });
                }
                return;
            }
        };
        let new_id = document.path.clone();
        self.reload_dashboards_from_store(cx);
        if let Some(manager) = &self.dashboard_manager {
            let open_ids: Vec<String> = self
                .dashboard_modules
                .iter()
                .filter_map(|module| module.id.clone())
                .filter(|id| !self.closed_dashboard_tabs.contains(id))
                .collect();
            manager.update(cx, |manager, cx| {
                manager.set_open_modules(open_ids, cx);
                manager.set_active_module(new_id.clone(), cx);
                manager.begin_rename_path(new_id.clone(), cx);
            });
        }
        self.load_and_apply_module(new_id, cx);
        cx.notify();
    }

    /// Reload the workspace tree and layout documents from disk.
    fn reload_dashboards_from_store(&mut self, cx: &mut Context<Self>) {
        match crate::data::layout_workspace::snapshot() {
            Ok(snapshot) => {
                self.layout_entries = snapshot.entries;
                self.dashboard_modules = snapshot.documents;
                self.layout_cache = self
                    .dashboard_modules
                    .iter()
                    .filter_map(|module| Some((module.id.clone()?, module.clone())))
                    .collect();
                self.resolve_legacy_tab_preferences();
                if let Some(manager) = &self.dashboard_manager {
                    let entries = self.layout_entries.clone();
                    let root_name = snapshot.root_name;
                    let open_ids: Vec<String> = self
                        .dashboard_modules
                        .iter()
                        .filter_map(|module| module.id.clone())
                        .filter(|id| !self.closed_dashboard_tabs.contains(id))
                        .collect();
                    manager.update(cx, |manager, cx| {
                        manager.update_tree(entries, root_name, cx);
                        manager.set_open_modules(open_ids, cx);
                    });
                }
                self.sync_settings_dashboards(cx);
                self.sync_workspace_tabs();
            }
            Err(error) => {
                tracing::error!(?error, "Failed to reload layout workspace");
                if let Some(manager) = &self.dashboard_manager {
                    manager.update(cx, |manager, cx| {
                        manager.update_error(error.to_string(), cx);
                    });
                }
            }
        }
    }

    /// Delete a layout by module ID
    fn delete_dashboard(&mut self, module_id: String, cx: &mut Context<Self>) {
        let active_path = self
            .active_module
            .as_ref()
            .and_then(|module| module.id.clone());
        let active_deleted = active_path
            .as_deref()
            .is_some_and(|path| path == module_id || path.starts_with(&format!("{module_id}/")));

        if let Err(error) = crate::data::layout_workspace::delete(&module_id) {
            tracing::error!(?error, "Failed to delete layout entry {module_id}");
            if let Some(manager) = &self.dashboard_manager {
                manager.update(cx, |manager, cx| {
                    manager.update_error(error.to_string(), cx);
                });
            }
            return;
        }

        self.dashboard_views
            .retain(|key, _| !path_is_or_under(key, &module_id));
        self.closed_dashboard_tabs
            .retain(|key| !path_is_or_under(key, &module_id));
        self.layout_cache
            .retain(|key, _| !path_is_or_under(key, &module_id));
        self.reload_dashboards_from_store(cx);

        if active_deleted {
            self.active_module = None;
            if let Some(first_module) = self.dashboard_modules.first().cloned() {
                if let Some(first_id) = first_module.id.clone() {
                    if let Some(manager) = &self.dashboard_manager {
                        manager.update(cx, |manager, cx| {
                            manager.set_active_module(first_id.clone(), cx);
                        });
                    }
                    self.load_and_apply_module(first_id, cx);
                }
            } else {
                if let Some(manager) = &self.dashboard_manager {
                    manager.update(cx, |manager, cx| manager.clear_active_module(cx));
                }
                if let Some(home) = &self.home_page {
                    home.update(cx, |page, cx| {
                        page.request_module_load(serde_json::json!({}), String::new(), cx);
                    });
                }
            }
        }

        self.persist_open_tabs();
        self.sync_manager_open_tabs(cx);
        self.sync_settings_dashboards(cx);
        cx.notify();
    }

    fn show_manager_error(&self, error: impl ToString, cx: &mut Context<Self>) {
        if let Some(manager) = &self.dashboard_manager {
            let message = error.to_string();
            manager.update(cx, |manager, cx| manager.update_error(message, cx));
        }
    }

    fn create_new_file(&mut self, parent: String, name: String, cx: &mut Context<Self>) {
        match crate::data::layout_workspace::create_file(&parent, &name) {
            Ok(path) => {
                self.reload_dashboards_from_store(cx);
                if let Some(manager) = &self.dashboard_manager {
                    manager.update(cx, |manager, cx| {
                        manager.set_selected_path(path.clone(), cx);
                        manager.begin_rename_path(path.clone(), cx);
                    });
                }
                self.open_workspace_file(path, cx);
                cx.notify();
            }
            Err(error) => self.show_manager_error(error, cx),
        }
    }

    fn create_new_folder(&mut self, parent: String, name: String, cx: &mut Context<Self>) {
        match crate::data::layout_workspace::create_folder(&parent, &name) {
            Ok(path) => {
                self.reload_dashboards_from_store(cx);
                if let Some(manager) = &self.dashboard_manager {
                    manager.update(cx, |manager, cx| {
                        manager.set_selected_path(path.clone(), cx);
                        manager.begin_rename_path(path.clone(), cx);
                    });
                }
                cx.notify();
            }
            Err(error) => self.show_manager_error(error, cx),
        }
    }

    fn remap_path_maps(&mut self, old: &str, new: &str) {
        fn remap_map<T>(map: &mut std::collections::HashMap<String, T>, old: &str, new: &str) {
            let keys: Vec<String> = map.keys().cloned().collect();
            for key in keys {
                if let Some(new_key) = remap_relative_path(&key, old, new) {
                    if let Some(value) = map.remove(&key) {
                        map.insert(new_key, value);
                    }
                }
            }
        }

        fn remap_set(set: &mut std::collections::HashSet<String>, old: &str, new: &str) {
            let keys: Vec<String> = set.iter().cloned().collect();
            for key in keys {
                if let Some(new_key) = remap_relative_path(&key, old, new) {
                    set.remove(&key);
                    set.insert(new_key);
                }
            }
        }

        remap_map(&mut self.dashboard_views, old, new);
        remap_map(&mut self.layout_cache, old, new);
        remap_set(&mut self.closed_dashboard_tabs, old, new);
        if let Some(active) = &mut self.active_module
            && let Some(id) = active.id.as_deref()
            && let Some(new_id) = remap_relative_path(id, old, new)
        {
            active.id = Some(new_id);
        }
        if let Some(pending) = &mut self.pending_module_load
            && let Some(new_id) = remap_relative_path(&pending.module_id, old, new)
        {
            pending.module_id = new_id;
        }
        for tab in &mut self.workspace_tabs {
            if let WorkspaceTab::Dashboard(id) = tab
                && let Some(new_id) = remap_relative_path(id, old, new)
            {
                *id = new_id;
            }
        }
        for slot in &mut self.group_active {
            if let Some(WorkspaceTab::Dashboard(id)) = slot
                && let Some(new_id) = remap_relative_path(id, old, new)
            {
                *id = new_id;
            }
        }
    }

    fn reload_after_path_change(
        &mut self,
        was_active: bool,
        old_active: Option<String>,
        old_path: &str,
        new_path: &str,
        cx: &mut Context<Self>,
    ) {
        if let Some(manager) = &self.dashboard_manager {
            manager.update(cx, |manager, cx| {
                manager.remap_paths(old_path, new_path, cx);
            });
        }
        self.remap_path_maps(old_path, new_path);
        self.reload_dashboards_from_store(cx);
        if was_active {
            let target = old_active
                .as_deref()
                .and_then(|path| remap_relative_path(path, old_path, new_path))
                .unwrap_or_else(|| new_path.to_string());
            if self
                .dashboard_modules
                .iter()
                .any(|module| module.id.as_deref() == Some(target.as_str()))
            {
                self.active_module = None;
                self.closed_dashboard_tabs.remove(&target);
                self.load_and_apply_module(target, cx);
            }
        }
        if let Some(manager) = &self.dashboard_manager {
            manager.update(cx, |manager, cx| {
                manager.set_selected_path(new_path.to_string(), cx);
            });
        }
        self.persist_open_tabs();
        self.sync_manager_open_tabs(cx);
        self.sync_settings_dashboards(cx);
        cx.notify();
    }

    fn rename_layout_entry(&mut self, path: String, name: String, cx: &mut Context<Self>) {
        let old_active = self
            .active_module
            .as_ref()
            .and_then(|module| module.id.clone());
        let was_active = old_active
            .as_deref()
            .is_some_and(|active| path_is_or_under(active, &path));
        let open_file_was_renamed = self
            .file_editor
            .as_ref()
            .is_some_and(|editor| editor.read(cx).path() == path);
        match crate::data::layout_workspace::rename(&path, &name) {
            Ok(new_path) => {
                self.reload_after_path_change(was_active, old_active, &path, &new_path, cx);
                self.close_file_tabs_under(&path, cx);
                if open_file_was_renamed {
                    if crate::data::layout_workspace::is_layout_file(&new_path) {
                        self.load_and_apply_module(new_path, cx);
                    } else {
                        self.open_workspace_file(new_path, cx);
                    }
                }
            }
            Err(error) => self.show_manager_error(error, cx),
        }
    }

    fn duplicate_layout_entry(&mut self, path: String, cx: &mut Context<Self>) {
        match crate::data::layout_workspace::duplicate(&path) {
            Ok(new_path) => {
                self.reload_dashboards_from_store(cx);
                if let Some(manager) = &self.dashboard_manager {
                    manager.update(cx, |manager, cx| {
                        manager.set_selected_path(new_path.clone(), cx);
                    });
                }
                self.closed_dashboard_tabs.remove(&new_path);
                if crate::data::layout_workspace::is_layout_file(&new_path) {
                    self.load_and_apply_module(new_path, cx);
                } else {
                    self.open_workspace_file(new_path, cx);
                }
                self.persist_open_tabs();
                self.sync_manager_open_tabs(cx);
                self.sync_settings_dashboards(cx);
                cx.notify();
            }
            Err(error) => self.show_manager_error(error, cx),
        }
    }

    fn move_layout_entry(&mut self, path: String, dest: String, cx: &mut Context<Self>) {
        let old_active = self
            .active_module
            .as_ref()
            .and_then(|module| module.id.clone());
        let was_active = old_active
            .as_deref()
            .is_some_and(|active| path_is_or_under(active, &path));
        let open_file_path = self
            .file_editor
            .as_ref()
            .map(|editor| editor.read(cx).path().to_string());
        match crate::data::layout_workspace::move_entry(&path, &dest) {
            Ok(new_path) => {
                self.reload_after_path_change(was_active, old_active, &path, &new_path, cx);
                let new_open = open_file_path
                    .as_deref()
                    .and_then(|open| remap_relative_path(open, &path, &new_path));
                self.close_file_tabs_under(&path, cx);
                if let Some(new_open) = new_open {
                    if crate::data::layout_workspace::is_layout_file(&new_open) {
                        self.load_and_apply_module(new_open, cx);
                    } else {
                        self.open_workspace_file(new_open, cx);
                    }
                }
            }
            Err(error) => self.show_manager_error(error, cx),
        }
    }

    /// Read a workspace text file for the Agent. Layouts and binary files are
    /// rejected; oversized files are truncated to the read budget.
    pub fn read_workspace_file(&mut self, path: &str) -> Result<String, String> {
        let path = path.trim();
        if path.is_empty() {
            return Err("A file path is required.".into());
        }
        if crate::data::layout_workspace::is_layout_file(path)
            || crate::data::file_control::is_layout_path(path)
        {
            return Err(
                "Layout files are managed by ui/terminal directives, not file edits.".into(),
            );
        }
        if !crate::data::file_control::is_text_file_path(path) {
            return Err("Only text files can be read by the Agent.".into());
        }
        let bytes =
            crate::data::layout_workspace::read_bytes(path).map_err(|error| error.to_string())?;
        if bytes.contains(&0) {
            return Err("The file contains binary data.".into());
        }
        let mut text = String::from_utf8_lossy(&bytes).into_owned();
        if text.len() > crate::data::file_control::MAX_READ_BYTES {
            let mut end = crate::data::file_control::MAX_READ_BYTES;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            text.truncate(end);
            text.push_str("\n…[truncated]");
        }
        Ok(text)
    }

    /// Apply one Agent `write`/`append` directive to the workspace.
    pub fn apply_file_write_action(
        &mut self,
        action: &crate::data::file_control::FileAction,
        cx: &mut Context<Self>,
    ) -> crate::data::pane_control::UiActionResult {
        use crate::data::file_control::FileAction;
        use crate::data::pane_control::UiActionResult;

        let name = action.action_name();
        let path = action
            .path()
            .map(|path| path.trim().to_string())
            .or_else(|| {
                (self.current_page == Page::File)
                    .then(|| self.active_file.clone())
                    .flatten()
            })
            .filter(|path| !path.trim().is_empty());
        let Some(path) = path else {
            return UiActionResult::failure(
                name,
                "Specify a workspace-relative path; no file tab is active.",
            );
        };
        if crate::data::layout_workspace::is_layout_file(&path)
            || crate::data::file_control::is_layout_path(&path)
        {
            return UiActionResult::failure(
                name,
                "Layout files must be edited with ui/terminal directives.",
            );
        }
        if !crate::data::file_control::is_text_file_path(&path) {
            return UiActionResult::failure(name, format!("{path} is not a text file."));
        }
        let Some(content) = action.content() else {
            return UiActionResult::failure(name, "No content was supplied.");
        };
        if content.len() > crate::data::file_control::MAX_FILE_WRITE_BYTES {
            return UiActionResult::failure(
                name,
                format!(
                    "Content is too large for one write ({} bytes).",
                    content.len()
                ),
            );
        }
        if let Some(editor) = self.file_editors.get(&path)
            && editor.read(cx).is_dirty()
            && !action.force()
        {
            return UiActionResult::failure(
                name,
                format!(
                    "{path} has unsaved changes; save it or pass force:true to overwrite the buffer."
                ),
            );
        }

        let saved = match action {
            FileAction::Write { .. } => {
                crate::data::layout_workspace::write_text_creating(&path, content)
            }
            FileAction::Append { .. } => crate::data::layout_workspace::append_text(&path, content),
            FileAction::Read { .. } => {
                return UiActionResult::failure(name, "read is not a write action.");
            }
        };
        if let Err(error) = saved {
            return UiActionResult::failure(name, format!("Could not save {path}: {error}"));
        }

        // Refresh an open editor so the new contents are visible and the dirty
        // flag clears; then rescan so a newly created file appears in the tree.
        if let Some(editor) = self.file_editors.get(&path).cloned() {
            editor.update(cx, |editor, cx| editor.reload(cx));
        }
        self.agent_file_snapshot = None;
        self.reload_dashboards_from_store(cx);
        self.refresh_agent_context(cx);

        let verb = if name == "append" {
            "Appended to"
        } else {
            "Saved"
        };
        UiActionResult::success(name, format!("{verb} {path} ({} bytes).", content.len()))
    }

    /// Open any non-layout file in the generic local file editor.
    fn open_workspace_file(&mut self, path: String, cx: &mut Context<Self>) {
        let tab = WorkspaceTab::File(path.clone());
        if self.group_of_tab(&tab).is_some() {
            self.activate_file_tab(path, cx);
            return;
        }

        let Some(editor) = self.ensure_file_editor(path.clone(), cx) else {
            return;
        };
        let title = editor.read(cx).title();
        let group = self.focused_group.min(self.group_count().saturating_sub(1));
        let index = self.group_tabs(group).len();
        self.insert_tab_in_group(group, index, tab.clone());
        self.sync_file_tabs_order();
        self.focused_group = group;
        self.group_active[group] = Some(tab);
        self.file_editor = Some(editor);
        self.file_editor_title = Some(title);
        self.active_file = Some(path);
        self.current_page = Page::File;
        self.refresh_agent_context(cx);
        self.persist_open_tabs();
        cx.notify();
    }

    /// Return the editor entity for `path`, creating it on demand. Restored
    /// tabs start without an entity so a session with many files does not read
    /// every file at launch.
    fn ensure_file_editor(
        &mut self,
        path: String,
        cx: &mut Context<Self>,
    ) -> Option<Entity<crate::components::ui::file_editor::FileEditor>> {
        if let Some(editor) = self.file_editors.get(&path) {
            return Some(editor.clone());
        }

        let editor_path = path.clone();
        let editor =
            cx.new(|cx| crate::components::ui::file_editor::FileEditor::new(editor_path, cx));
        let close_path = path.clone();
        cx.subscribe(
            &editor,
            move |this: &mut MainView,
                  _editor: Entity<crate::components::ui::file_editor::FileEditor>,
                  event: &crate::components::ui::file_editor::FileEditorEvent,
                  cx| {
                if matches!(
                    event,
                    crate::components::ui::file_editor::FileEditorEvent::Close
                ) {
                    this.close_file_tab(close_path.clone(), cx);
                }
            },
        )
        .detach();
        self.file_editors.insert(path, editor.clone());
        Some(editor)
    }

    fn activate_file_tab(&mut self, path: String, cx: &mut Context<Self>) {
        let tab = WorkspaceTab::File(path.clone());
        let group = match self.group_of_tab(&tab) {
            Some(group) => group,
            None => {
                let group = self.focused_group.min(self.group_count().saturating_sub(1));
                let index = self.group_tabs(group).len();
                self.insert_tab_in_group(group, index, tab.clone());
                self.sync_file_tabs_order();
                group
            }
        };
        let Some(editor) = self.ensure_file_editor(path.clone(), cx) else {
            return;
        };
        let title = editor.read(cx).title();
        self.focused_group = group;
        self.group_active[group] = Some(tab);
        self.file_editor = Some(editor);
        self.file_editor_title = Some(title);
        self.active_file = Some(path);
        self.current_page = Page::File;
        self.refresh_agent_context(cx);
        cx.notify();
    }

    fn close_file_tabs_under(&mut self, prefix: &str, cx: &mut Context<Self>) {
        let affected: Vec<String> = self
            .file_tabs
            .iter()
            .filter(|path| path_is_or_under(path, prefix))
            .cloned()
            .collect();
        for path in affected {
            self.close_file_tab(path, cx);
        }
    }

    fn close_file_tab(&mut self, path: String, cx: &mut Context<Self>) {
        self.file_editors.remove(&path);
        let tab = WorkspaceTab::File(path);
        let previous_focused = self.focused_group;
        let previous_active = self.group_active[previous_focused].clone();
        self.remove_tab_everywhere(&tab);
        self.sync_file_tabs_order();
        self.ensure_group_actives();
        if self.focused_group != previous_focused
            || self.group_active[self.focused_group] != previous_active
        {
            let active = self.group_active[self.focused_group].clone();
            self.show_active_tab(active, cx);
        } else {
            cx.notify();
        }
        self.persist_open_tabs();
    }

    fn open_workspace_folder(&mut self, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open Layouts Folder".into()),
        });
        cx.spawn(async move |this, cx| {
            let result = receiver.await;
            let _ = this.update(cx, |view, cx| {
                let path = match result {
                    Ok(Ok(Some(paths))) => paths.into_iter().next(),
                    Ok(Ok(None)) => None,
                    Ok(Err(error)) => {
                        view.show_manager_error(error, cx);
                        None
                    }
                    Err(error) => {
                        view.show_manager_error(error, cx);
                        None
                    }
                };
                let Some(path) = path else {
                    return;
                };
                match crate::data::layout_workspace::set_root(&path) {
                    Ok(_) => {
                        view.active_module = None;
                        view.home_page = None;
                        view.pending_module_load = None;
                        view.dashboard_views.clear();
                        view.layout_cache.clear();
                        view.closed_dashboard_tabs.clear();
                        view.file_tabs.clear();
                        view.workspace_tabs.clear();
                        view.split_at = None;
                        view.group_active = [None, None];
                        view.focused_group = 0;
                        view.file_editors.clear();
                        view.file_editor = None;
                        view.file_editor_title = None;
                        view.active_file = None;
                        view.restore_local_workspace(cx);
                        cx.notify();
                    }
                    Err(error) => view.show_manager_error(error, cx),
                }
            });
        })
        .detach();
    }

    fn load_settings_then_modules(&mut self, cx: &mut Context<Self>) {
        let client = self.api_client.clone();
        let rt = crate::data::client::tokio_runtime();
        // The backend origin is configured per machine and must survive a
        // settings round-trip. The shared server record commonly has
        // `serverUrl: null`, which previously wiped the local endpoint and left
        // the next launch with no API base URL at all.
        let local_server_url = self
            .user_settings
            .as_ref()
            .and_then(|settings| settings.server_url.as_deref())
            .and_then(crate::data::client::normalize_api_origin);

        cx.spawn(async move |this, cx| {
            let settings_res = rt
                .spawn_blocking(move || client.get_settings())
                .await
                .unwrap_or(Err(crate::data::error::CoinWatchError::Io(
                    "Task failed".into(),
                )));

            let _ = this.update(cx, |view, cx| {
                match settings_res {
                    Ok(Some(mut user_settings)) => {
                        user_settings.settings.server_url = user_settings
                            .settings
                            .server_url
                            .as_deref()
                            .and_then(crate::data::client::normalize_api_origin)
                            .or_else(|| local_server_url.clone());

                        if let Some(path) = Self::cloud_settings_path() {
                            if let Some(parent) = path.parent() {
                                let _ = std::fs::create_dir_all(parent);
                            }
                            if let Ok(json) = serde_json::to_string(&user_settings.settings) {
                                let _ = std::fs::write(&path, json);
                            }
                        }

                        let settings_clone = user_settings.settings.clone();
                        if let Some(server_url) = settings_clone
                            .server_url
                            .as_deref()
                            .and_then(crate::data::client::normalize_api_origin)
                        {
                            view.api_client.set_base_url(&server_url);
                        }
                        let theme_name = settings_clone
                            .theme
                            .clone()
                            .unwrap_or_else(|| "gruvbox".to_string());
                        let dark_mode = settings_clone
                            .dark_mode
                            .as_ref()
                            .map(|s| s == "dark")
                            .unwrap_or(false);
                        crate::data::theme::apply_theme(&theme_name, dark_mode, cx);
                        view.user_settings = Some(settings_clone.clone());
                        view.apply_update_channel(settings_clone.update_channel.as_deref(), cx);
                        if let Some(home) = &view.home_page {
                            let settings = settings_clone.clone();
                            home.update(cx, |page, cx| {
                                page.apply_settings(settings.clone(), cx);
                            });
                        }
                    }
                    Ok(None) => {
                        tracing::warn!("No user settings found in database, using defaults");
                    }
                    Err(error) => {
                        tracing::warn!(?error, "Failed to load settings; keeping local settings");
                    }
                }

                // Layouts are always local and load independently of auth.
                view.restore_local_workspace(cx);
                view.is_restoring = false;
                cx.notify();
            });
        })
        .detach();
    }

    /// Persist a layout edited in a detached window without relying on the
    /// currently selected tab in the main window.
    fn save_detached_layout(&mut self, module_id: String, tree: PaneNode, _cx: &mut Context<Self>) {
        let Ok(layout_json) = serde_json::to_value(&tree) else {
            return;
        };

        if let Some(module) = self
            .dashboard_modules
            .iter_mut()
            .find(|m| m.id.as_ref() == Some(&module_id))
        {
            module.module = layout_json.clone();
        }
        if let Some(module) = self.layout_cache.get_mut(&module_id) {
            module.module = layout_json.clone();
        }

        // Layouts are always local files now; signed-in state does not change
        // where edits are stored.
        if let Err(error) = crate::data::layout_workspace::save_layout(&module_id, layout_json) {
            tracing::error!(?error, "Failed to save detached layout {module_id}");
        }
    }

    /// Save current settings to the API
    fn save_settings_to_api(&self, settings: SettingsData, cx: &mut Context<Self>) {
        let client = self.api_client.clone();
        // Layout selection path is local workspace state now; never send it to
        // the shared settings record.
        let mut settings = settings;
        settings.current_dashboard = None;

        tracing::info!("Saving settings to API");

        // Always keep a local cache, including when authenticated. This lets settings
        // survive offline startup and keeps local preferences available on this machine.
        for path in [Self::guest_settings_path(), Self::cloud_settings_path()]
            .into_iter()
            .flatten()
        {
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
            if let Err(error) =
                crate::data::tab_settings::save(self.tab_profile(), &tab_preferences)
            {
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

            // Update ApiClient with configured server_url. An empty value means
            // "use the hosted default" for signed-in users, not "clear the
            // backend" (which would make every API call fail).
            let configured_server_url = settings_data
                .server_url
                .as_deref()
                .and_then(crate::data::client::normalize_api_origin);
            let resolved_server_url = configured_server_url.or_else(|| {
                self.is_authenticated
                    .then(|| crate::data::client::DEFAULT_CLOUD_API_URL.to_string())
            });
            if let Some(server_url) = &resolved_server_url {
                self.api_client.set_base_url(server_url);
            }
            // Never persist a legacy/invalid origin back to the settings record.
            settings_data.server_url = resolved_server_url;

            // Preserve settings the native settings page does not expose while
            // replacing every field that it manages.
            if let Some(existing) = &self.user_settings {
                settings_data.current_dashboard = None;
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

            self.apply_update_channel(settings_data.update_channel.as_deref(), cx);

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

                if mode == crate::data::tab_settings::StartupTabMode::Empty || open_ids.is_empty() {
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
                    if !active_is_open && let Some(module_id) = open_ids.first() {
                        self.load_and_apply_module(module_id.clone(), cx);
                    }
                }

                if let Some(manager) = &self.dashboard_manager {
                    let open_ids: Vec<String> = self
                        .dashboard_modules
                        .iter()
                        .filter_map(|module| module.id.clone())
                        .filter(|id| !self.closed_dashboard_tabs.contains(id))
                        .collect();
                    manager.update(cx, |manager, cx| manager.set_open_modules(open_ids, cx));
                }
                self.sync_settings_dashboards(cx);
            }

            cx.notify();
        }
    }

    /// Apply the release channel stored in settings. When it changes, forget
    /// the previous check timestamp and immediately look for updates on the
    /// newly selected channel.
    fn apply_update_channel(&mut self, channel: Option<&str>, cx: &mut Context<Self>) {
        let Some(channel) = channel.and_then(UpdateChannel::parse) else {
            return;
        };
        if !self.update_manager.set_channel(channel) {
            return;
        }

        // The render-time check is one-shot; run a fresh check now instead.
        self.update_check_started = true;
        let mgr = self.update_manager.clone();
        cx.spawn(async move |this, cx| {
            let result = crate::data::client::tokio_runtime()
                .spawn_blocking(move || mgr.force_check())
                .await;
            if let Err(error) = result {
                tracing::warn!("Update check after channel switch failed: {error}");
            }
            let _ = this.update(cx, |_this, cx| cx.notify());
        })
        .detach();
    }

    /// Create the update banner and subscribe to its actions once per session.
    fn ensure_update_banner(&mut self, cx: &mut Context<Self>) {
        if self.update_banner.is_some() {
            return;
        }

        let mgr = self.update_manager.clone();
        let banner = cx.new(|_cx| UpdateBanner::new(mgr));

        cx.subscribe(
            &banner,
            |this: &mut MainView, _banner, event: &UpdateBannerEvent, cx| match event {
                UpdateBannerEvent::DownloadRequested => {
                    if let UpdateState::Available(info) = this.update_manager.state() {
                        this.start_update_download(*info, false, cx);
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
                    })
                    .detach();
                }
                UpdateBannerEvent::RestartRequested => {
                    // Prefer the platform restart helper: it waits for this
                    // process to exit before launching the replacement. On
                    // Linux `current_exe()` points at a deleted inode after
                    // `self_replace`, so pass the path captured at install
                    // time instead.
                    if let Some(exe) = UpdateManager::installed_exe_path() {
                        cx.set_restart_path(exe);
                        cx.restart();
                    } else if let Err(error) = UpdateManager::restart_app() {
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
                        if let Err(error) = result {
                            tracing::warn!("Update check task failed: {error}");
                        }
                        let _ = this.update(cx, |_this, cx| cx.notify());
                    })
                    .detach();
                }
            },
        )
        .detach();

        self.update_banner = Some(banner);
    }

    /// Apply a channel selected from the Settings UI and immediately install
    /// that channel's current build, even if it is older than the running one.
    fn select_update_channel(&mut self, cx: &mut Context<Self>) {
        let channel = self
            .settings_page
            .as_ref()
            .and_then(|page| page.read(cx).get_settings_data(cx).update_channel)
            .and_then(|value| UpdateChannel::parse(&value));
        let Some(channel) = channel else {
            return;
        };

        if !self.update_manager.set_channel(channel) {
            self.update_manager.prepare_channel_check();
        }
        self.save_settings_from_page(cx);
        self.force_update_channel_check(cx);
    }

    fn force_update_channel_check(&mut self, cx: &mut Context<Self>) {
        self.update_check_started = true;
        let mgr = self.update_manager.clone();
        cx.spawn(async move |this, cx| {
            let result = crate::data::client::tokio_runtime()
                .spawn_blocking(move || mgr.force_channel_check())
                .await;
            if let Err(error) = result {
                tracing::warn!("Channel update check failed: {error}");
            }
            let _ = this.update(cx, |this, cx| {
                this.auto_install_selected_channel_update(cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn auto_install_selected_channel_update(&mut self, cx: &mut Context<Self>) {
        if let UpdateState::Available(info) = self.update_manager.state() {
            self.start_update_download(*info, true, cx);
        }
    }

    /// Download an available update. When `auto_install` is set, install it as
    /// soon as the download finishes and leave the app in `RestartRequired`.
    fn start_update_download(
        &mut self,
        info: VersionInfo,
        auto_install: bool,
        cx: &mut Context<Self>,
    ) {
        let mgr = self.update_manager.clone();
        cx.spawn(async move |this, cx| {
            let mgr_worker = mgr.clone();
            let info_worker = info.clone();
            let download_task = crate::data::client::tokio_runtime().spawn(async move {
                tokio::task::spawn_blocking(move || mgr_worker.download_update(&info_worker)).await
            });

            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(100))
                    .await;
                let is_downloading = matches!(mgr.state(), UpdateState::Downloading { .. });
                if this.update(cx, |_this, cx| cx.notify()).is_err() || !is_downloading {
                    break;
                }
            }

            let downloaded = match download_task.await {
                Ok(Ok(Ok(_))) => true,
                Ok(Ok(Err(error))) => {
                    tracing::error!("Update download failed: {error}");
                    false
                }
                Ok(Err(error)) => {
                    tracing::error!("Update download task failed: {error}");
                    false
                }
                Err(error) => {
                    tracing::error!("Update download task failed: {error}");
                    false
                }
            };

            if downloaded && auto_install {
                if let UpdateState::ReadyToInstall { version, path } = mgr.state() {
                    let path = std::path::PathBuf::from(path);
                    let result = crate::data::client::tokio_runtime()
                        .spawn_blocking(move || UpdateManager::install_update(&path))
                        .await;
                    match result {
                        Ok(Ok(())) => mgr.mark_installed(&version),
                        Ok(Err(error)) => mgr.fail(error.to_string()),
                        Err(error) => mgr.fail(format!("Update install task failed: {error}")),
                    }
                }
            }

            let _ = this.update(cx, |_this, cx| cx.notify());
        })
        .detach();
    }

    /// Save the current layout immediately (called on LayoutModified events).
    /// Layouts are local files; there is no network path here.
    fn save_current_layout_immediately(&mut self, cx: &mut Context<Self>) {
        let Some(home) = &self.home_page else {
            return;
        };
        let Some(tree) = home.read(cx).pane_grid.read(cx).tree.clone() else {
            return;
        };
        let Some(module_id) = self.active_module.as_ref().and_then(|m| m.id.clone()) else {
            return;
        };
        let Ok(layout_json) = serde_json::to_value(&tree) else {
            return;
        };

        if let Err(error) =
            crate::data::layout_workspace::save_layout(&module_id, layout_json.clone())
        {
            tracing::error!(?error, "Failed to save layout {module_id}");
            if let Some(manager) = &self.dashboard_manager {
                manager.update(cx, |manager, cx| {
                    manager.update_error(format!("Could not save {module_id}: {error}"), cx);
                });
            }
        } else if let Some(cached_module) = self.layout_cache.get_mut(&module_id) {
            cached_module.module = layout_json;
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
                if let Some(panel) = &this.alerts_panel {
                    panel.update(cx, |panel, cx| panel.clear(cx));
                }
                if let Ok(mut store) = crate::data::alerts::AlertStore::global().lock() {
                    store.clear();
                }
                this.load_guest_layout(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Return the live `HomePage` for a dashboard, creating it on demand. Used
    /// both for the focused dashboard and for the second editor group.
    fn ensure_dashboard_home(
        &mut self,
        module_id: &str,
        cx: &mut Context<Self>,
    ) -> Option<Entity<HomePage>> {
        if let Some(home) = self.dashboard_views.get(module_id) {
            return Some(home.clone());
        }
        let module = self
            .dashboard_modules
            .iter()
            .find(|module| module.id.as_deref() == Some(module_id))?
            .clone();
        let home = cx.new(HomePage::new);
        let subscription_id = module_id.to_string();
        cx.subscribe(
            &home,
            move |this: &mut MainView, _source, _: &HomePageEvent, cx| {
                this.save_layout_for_dashboard(&subscription_id, cx);
            },
        )
        .detach();
        home.update(cx, |page, cx| {
            page.request_module_load(module.module.clone(), module_id.to_string(), cx);
            if let Some(settings) = self.user_settings.clone() {
                page.apply_settings(settings, cx);
            }
        });
        self.dashboard_views
            .insert(module_id.to_string(), home.clone());
        Some(home)
    }

    /// Save a dashboard's pane tree by module id. A split group can show a
    /// dashboard that is not the globally focused one.
    fn save_layout_for_dashboard(&mut self, module_id: &str, cx: &mut Context<Self>) {
        let Some(home) = self.dashboard_views.get(module_id) else {
            return;
        };
        let Some(tree) = home.read(cx).pane_grid.read(cx).tree.clone() else {
            return;
        };
        let Ok(layout_json) = serde_json::to_value(&tree) else {
            return;
        };
        if let Err(error) =
            crate::data::layout_workspace::save_layout(module_id, layout_json.clone())
        {
            tracing::error!(?error, "Failed to save layout {module_id}");
            if let Some(manager) = &self.dashboard_manager {
                let message = format!("Could not save {module_id}: {error}");
                manager.update(cx, |manager, cx| manager.update_error(message, cx));
            }
        } else if let Some(cached_module) = self.layout_cache.get_mut(module_id) {
            cached_module.module = layout_json;
        }
    }

    /// Load a module from the cache (instant) or API and apply it to the PaneGrid
    fn load_and_apply_module(&mut self, module_id: String, cx: &mut Context<Self>) {
        tracing::debug!("load_and_apply_module called with id: {}", module_id);

        // Selecting the active tab must not rebuild the pane tree or refresh
        // modules. Leaving the file editor for that same tab is still a real
        // switch, so only short-circuit when the dashboard is already showing.
        let already_active = self
            .active_module
            .as_ref()
            .and_then(|module| module.id.as_ref())
            == Some(&module_id);
        if already_active && self.current_page == Page::Home && self.active_file.is_none() {
            return;
        }

        let module_to_load = self
            .dashboard_modules
            .iter()
            .find(|m| m.id.as_ref() == Some(&module_id))
            .cloned();

        if let Some(module) = module_to_load {
            let tab = WorkspaceTab::Dashboard(module_id.clone());
            let group = match self.group_of_tab(&tab) {
                Some(group) => group,
                None => {
                    let group = self.focused_group.min(self.group_count().saturating_sub(1));
                    let index = self.group_tabs(group).len();
                    self.insert_tab_in_group(group, index, tab.clone());
                    group
                }
            };
            self.focused_group = group;
            self.group_active[group] = Some(tab);

            let Some(home) = self.ensure_dashboard_home(&module_id, cx) else {
                tracing::warn!(
                    "Could not find module ID {} in dashboard_modules",
                    module_id
                );
                return;
            };
            cx.set_global(crate::components::pages::home::GlobalHomeView {
                view: home.downgrade(),
            });
            self.home_page = Some(home);
            self.closed_dashboard_tabs.remove(&module_id);
            self.persist_open_tabs();
            self.sync_manager_open_tabs(cx);
            self.active_module = Some(module.clone());
            self.refresh_agent_context(cx);
            if let Some(dm) = &self.dashboard_manager {
                dm.update(cx, |manager, cx| {
                    manager.set_active_module(module_id.clone(), cx)
                });
            }
            self.current_page = Page::Home;
            self.active_file = None;

            // Remember the dashboard the user last worked in so the next
            // launch/login restores it instead of falling back to the first
            // module. Keep this here (not only in the dashboard manager) so
            // every entry point — tab clicks, closing a tab, reattaching a
            // detached window — persists the selection.
            self.remember_current_dashboard(&module_id);

            cx.notify();
        } else {
            tracing::warn!(
                "Could not find module ID {} in dashboard_modules",
                module_id
            );
        }
    }

    /// Persist the active layout path so it can be restored on the next launch.
    /// This is local workspace state only; it is never sent to the backend.
    fn remember_current_dashboard(&mut self, module_path: &str) {
        if crate::data::layout_workspace::active_layout().as_deref() == Some(module_path) {
            return;
        }
        if let Err(error) = crate::data::layout_workspace::set_active_layout(Some(module_path)) {
            tracing::warn!(?error, "Failed to remember active layout");
        }
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
                                    .text_size(px(13.0))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(theme.colors.foreground)
                                    .child(match sidebar_type {
                                        SecondarySidebar::Dashboard => "Layouts",
                                        SecondarySidebar::Feeds => "Feeds",
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
            SecondarySidebar::Notifications => self
                .alerts_panel
                .as_ref()
                .map(|panel| panel.clone().into_any_element())
                .unwrap_or_else(|| div().into_any_element()),
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
        // `terminal` directives in any Agent conversation need to reach this
        // workspace even when the conversation lives in a detached window.
        cx.set_global(GlobalMainView {
            view: cx.weak_entity(),
        });
        self.start_agent_context_refresh(cx);
        // Keep the alert desk alive for the whole session: chart panes read the
        // shared alert snapshot even when the sidebar has never been opened.
        self.ensure_alerts_panel(cx);
        self.ensure_update_banner(cx);
        if let Some(panel) = self.alerts_panel.clone() {
            panel.update(cx, |panel, cx| {
                panel.drain_notifications(window, cx);
            });
        }
        let theme = cx.theme().clone();
        let current_page = self.current_page;
        let split_workspace = self.split_visible()
            && matches!(current_page, Page::Home | Page::Dashboard | Page::File);
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
                    is_folder,
                    child_count,
                } => {
                    let dialog_entity = open_delete_dashboard_dialog(
                        module_id.clone(),
                        module_name,
                        is_folder,
                        child_count,
                        window,
                        cx,
                    );
                    cx.subscribe(
                        &dialog_entity,
                        |this: &mut MainView, _dialog, event: &DeleteDashboardDialogEvent, cx| {
                            match event {
                                DeleteDashboardDialogEvent::Confirm(module_id) => {
                                    this.delete_dashboard(module_id.clone(), cx);
                                }
                                DeleteDashboardDialogEvent::Cancel => {}
                            }
                        },
                    )
                    .detach();
                }
            }
        }

        // Lazily initialise the explorer with the local tree retained by
        // MainView. Workspace loading can finish before this entity exists.
        if self.dashboard_manager.is_none() {
            let entries = self.layout_entries.clone();
            let root_name = crate::data::layout_workspace::root()
                .ok()
                .and_then(|root| {
                    root.file_name()
                        .map(|name| name.to_string_lossy().to_string())
                })
                .unwrap_or_else(|| "Layouts".to_string());
            let active_path = self
                .active_module
                .as_ref()
                .and_then(|module| module.id.clone());
            let open_ids: Vec<String> = self
                .dashboard_modules
                .iter()
                .filter_map(|module| module.id.clone())
                .filter(|id| !self.closed_dashboard_tabs.contains(id))
                .collect();

            let dashboard_manager = cx.new(|cx| {
                let mut manager =
                    crate::components::ui::dashboard_manager::DashboardManager::new(cx);
                manager.update_tree(entries, root_name, cx);
                manager.set_open_modules(open_ids, cx);
                if let Some(path) = active_path {
                    manager.set_active_module(path, cx);
                }
                manager
            });

            cx.subscribe(
                &dashboard_manager,
                |this: &mut MainView,
                 _manager: Entity<crate::components::ui::dashboard_manager::DashboardManager>,
                 event: &crate::components::ui::dashboard_manager::DashboardManagerEvent,
                 cx| {
                    use crate::components::ui::dashboard_manager::DashboardManagerEvent as Event;
                    match event {
                        Event::OpenLayout(path) => {
                            if let Some(manager) = &this.dashboard_manager {
                                manager.update(cx, |manager, cx| {
                                    manager.set_active_module(path.clone(), cx);
                                });
                            }
                            this.closed_dashboard_tabs.remove(path);
                            this.load_and_apply_module(path.clone(), cx);
                        }
                        Event::OpenFile(path) => {
                            this.open_workspace_file(path.clone(), cx);
                        }
                        Event::DetachLayout(path) => {
                            this.detach_dashboard(path.clone(), cx);
                        }
                        Event::CloseLayout(path) => {
                            this.close_dashboard_tab(path.clone(), cx);
                        }
                        Event::NewFile(parent) => {
                            this.create_new_file(parent.clone(), "Untitled".to_string(), cx);
                        }
                        Event::NewLayout(parent) => {
                            this.create_new_dashboard(
                                parent.clone(),
                                "Untitled Layout".to_string(),
                                cx,
                            );
                        }
                        Event::NewFolder(parent) => {
                            this.create_new_folder(parent.clone(), "New Folder".to_string(), cx);
                        }
                        Event::RenameEntry { path, name } => {
                            this.rename_layout_entry(path.clone(), name.clone(), cx);
                        }
                        Event::DuplicateEntry(path) => {
                            this.duplicate_layout_entry(path.clone(), cx);
                        }
                        Event::MoveEntry { path, dest } => {
                            this.move_layout_entry(path.clone(), dest.clone(), cx);
                        }
                        Event::DeleteEntry {
                            path,
                            name,
                            is_folder,
                            child_count,
                        } => {
                            this.pending_dialog_action =
                                Some(PendingDialogAction::OpenDeleteDashboard {
                                    module_id: path.clone(),
                                    module_name: name.clone(),
                                    is_folder: *is_folder,
                                    child_count: *child_count,
                                });
                            cx.notify();
                        }
                        Event::RevealEntry(path) => {
                            if let Err(error) = crate::data::layout_workspace::reveal(path) {
                                if let Some(manager) = &this.dashboard_manager {
                                    manager.update(cx, |manager, cx| {
                                        manager.update_error(error.to_string(), cx);
                                    });
                                }
                            }
                        }
                        Event::OpenWorkspace => {
                            this.open_workspace_folder(cx);
                        }
                        Event::Refresh => {
                            this.reload_dashboards_from_store(cx);
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

        // Generic file from the explorer: keep the icon rail and Layouts
        // sidebar visible so other files can be opened without leaving.
        if current_page == Page::File && !split_workspace {
            let (overlay_bubbles, overlay_composer) = self.agent_overlay_elements(window, cx);
            let mono_font = cx.theme().mono_font_family.clone();
            let drag_active = cx.has_active_drag();
            let hover_left = drag_active && self.edge_hover == Some((0, true));
            let hover_right = drag_active && self.edge_hover == Some((0, false));
            let editor = self
                .file_editor
                .clone()
                .map(|editor| {
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_h(px(0.0))
                        .min_w_0()
                        .child(editor)
                        .into_any_element()
                })
                .unwrap_or_else(|| div().into_any_element());
            let main = div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h(px(0.0))
                .min_w_0()
                .child(FinTerTitleBar::new("").tabs(self.dashboard_tabs(entity.clone(), cx)))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_h(px(0.0))
                        .overflow_hidden()
                        .relative()
                        .on_drag_move::<DragWorkspaceTab>(cx.listener(
                            |this, event: &gpui::DragMoveEvent<DragWorkspaceTab>, _, cx| {
                                this.track_edge_hover(0, event, cx);
                            },
                        ))
                        .child(editor)
                        .children(overlay_bubbles)
                        .children(overlay_composer)
                        .when(hover_left, |this| {
                            this.child(Self::split_ghost(
                                0,
                                true,
                                entity.clone(),
                                &theme.colors,
                                &mono_font,
                            ))
                        })
                        .when(hover_right, |this| {
                            this.child(Self::split_ghost(
                                0,
                                false,
                                entity.clone(),
                                &theme.colors,
                                &mono_font,
                            ))
                        })
                        .child(Self::render_single_edge_zone(
                            true,
                            entity.clone(),
                            &theme.colors,
                        ))
                        .child(Self::render_single_edge_zone(
                            false,
                            entity.clone(),
                            &theme.colors,
                        )),
                );
            let sidebar_element = sidebar::build_sidebar_element(
                entity.clone(),
                self.current_page,
                self.active_secondary_sidebar,
                &user_info,
            );
            let secondary_sidebar_element = self
                .active_secondary_sidebar
                .map(|kind| self.build_secondary_sidebar(kind, cx));

            if is_macos {
                return div()
                    .flex()
                    .flex_row()
                    .size_full()
                    .bg(theme.colors.background)
                    .child(main)
                    .when_some(secondary_sidebar_element, |this, sidebar| {
                        this.child(sidebar)
                    })
                    .child(sidebar_element);
            } else {
                return div()
                    .flex()
                    .flex_row()
                    .size_full()
                    .bg(theme.colors.background)
                    .child(sidebar_element)
                    .when_some(secondary_sidebar_element, |this, sidebar| {
                        this.child(sidebar)
                    })
                    .child(main);
            }
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

                let channel_entity = entity.clone();
                let on_channel_selected = Arc::new(move |cx: &mut App| {
                    if let Some(e) = channel_entity.upgrade() {
                        cx.update_entity(&e, |view: &mut MainView, cx: &mut Context<MainView>| {
                            view.select_update_channel(cx);
                        });
                    }
                });

                self.settings_page = Some(cx.new(move |cx| {
                    SettingsPage::new_with_data(&settings_data, tab_profile, cx)
                        .with_dashboards(dashboards.clone())
                        .with_on_change(on_setting_changed.clone())
                        .with_channel_selected(on_channel_selected.clone())
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
                .flex()
                .flex_col()
                .when(
                    !matches!(
                        self.update_manager.state(),
                        UpdateState::Idle | UpdateState::Checking
                    ),
                    |this| {
                        if let Some(banner) = self.update_banner.as_ref() {
                            this.child(banner.clone())
                        } else {
                            this
                        }
                    },
                )
                .child(
                    div()
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
                        }),
                );

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
                            .child(FinTerTitleBar::new(page_title).child_before_title(back_button))
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
                            .child(FinTerTitleBar::new(page_title).child_before_title(back_button))
                            .child(settings_content),
                    );
            }
        }

        // Create home page lazily (main trading view)
        if self.home_page.is_none() {
            let active_id = self.active_module.as_ref().and_then(|m| m.id.clone());
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
                if let Err(error) = result {
                    tracing::warn!("Update check task failed: {error}");
                }
                let _ = this.update(cx, |_this, cx| cx.notify());
            })
            .detach();
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

        // Workspace-level Agent: a floating composer at the bottom and a
        // transcript pinned to the top-right. Both are absolutely positioned
        // children of the workspace body, so the pane grid keeps its full size.
        let (overlay_bubbles, overlay_composer) = self.agent_overlay_elements(window, cx);

        let mono_font = cx.theme().mono_font_family.clone();
        let single_drag_active = cx.has_active_drag() && !split_workspace;
        let single_workspace_page = matches!(current_page, Page::Home | Page::Dashboard);
        let single_hover_left =
            single_drag_active && single_workspace_page && self.edge_hover == Some((0, true));
        let single_hover_right =
            single_drag_active && single_workspace_page && self.edge_hover == Some((0, false));
        let workspace_width = f32::from(window.viewport_size().width)
            - crate::components::ui::sidebar::get_icon_sidebar_width()
            - if active_secondary_sidebar.is_some() {
                crate::components::ui::sidebar::get_secondary_sidebar_width()
            } else {
                0.0
            };

        let title_row: AnyElement = if split_workspace {
            self.render_split_title_row(entity.clone(), workspace_width, cx)
        } else {
            FinTerTitleBar::new("")
                .tabs(self.dashboard_tabs(entity.clone(), cx))
                .into_any_element()
        };

        let content_with_title_bar = div()
            .flex()
            .flex_col()
            .flex_1()
            .overflow_hidden()
            .child(title_row)
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
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .relative()
                    .when(!split_workspace, |this| {
                        this.on_drag_move::<DragWorkspaceTab>(cx.listener(
                            |this, event: &gpui::DragMoveEvent<DragWorkspaceTab>, _, cx| {
                                this.track_edge_hover(0, event, cx);
                            },
                        ))
                    })
                    .child(if split_workspace {
                        self.render_split_workspace(entity.clone(), cx)
                            .into_any_element()
                    } else {
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
                            })
                            .into_any_element()
                    })
                .children(overlay_bubbles)
                .children(overlay_composer)
                .when(!split_workspace, |this| {
                    this.child(Self::render_single_edge_zone(
                        true,
                        entity.clone(),
                        &theme.colors,
                    ))
                    .child(Self::render_single_edge_zone(
                        false,
                        entity.clone(),
                        &theme.colors,
                    ))
                })
                .when(single_hover_left, |this| {
                    this.child(Self::split_ghost(
                        0,
                        true,
                        entity.clone(),
                        &theme.colors,
                        &mono_font,
                    ))
                })
                .when(single_hover_right, |this| {
                    this.child(Self::split_ghost(
                        0,
                        false,
                        entity.clone(),
                        &theme.colors,
                        &mono_font,
                    ))
                }),
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

/// Read at most `limit` bytes of a UTF-8-ish file for the Agent context.
///
/// Returns `(text, truncated)`. Binary files (NUL bytes) and unreadable files
/// yield `(None, false)`.
fn read_agent_file(path: &std::path::Path, limit: usize) -> (Option<String>, bool) {
    use std::io::Read;

    let Ok(file) = std::fs::File::open(path) else {
        return (None, false);
    };
    let mut buffer = Vec::new();
    if file
        .take(limit as u64 + 1)
        .read_to_end(&mut buffer)
        .is_err()
    {
        return (None, false);
    }
    let truncated = buffer.len() > limit;
    buffer.truncate(limit);
    if buffer.contains(&0) {
        return (None, false);
    }
    let mut text = String::from_utf8_lossy(&buffer).into_owned();
    if truncated {
        while text.ends_with('\u{fffd}') {
            text.pop();
        }
    }
    (Some(text), truncated)
}

fn remap_relative_path(path: &str, old: &str, new: &str) -> Option<String> {
    if path == old {
        return Some(new.to_string());
    }
    path.strip_prefix(old)
        .and_then(|suffix| suffix.strip_prefix('/'))
        .map(|suffix| format!("{new}/{suffix}"))
}

/// Clamp the horizontal split ratio so neither editor group can disappear.
fn clamp_split_ratio(ratio: f32) -> f32 {
    if ratio.is_nan() {
        0.5
    } else {
        ratio.clamp(0.18, 0.82)
    }
}

/// Move `dragged` immediately before `target` in the strip. Returns `false`
/// when either tab is missing or the move would be a no-op.
#[cfg(test)]
fn reorder_workspace_tabs(
    tabs: &mut Vec<WorkspaceTab>,
    dragged: &WorkspaceTab,
    target: &WorkspaceTab,
) -> bool {
    if dragged == target {
        return false;
    }
    let Some(from) = tabs.iter().position(|tab| tab == dragged) else {
        return false;
    };
    let dragged = tabs.remove(from);
    let Some(to) = tabs.iter().position(|tab| tab == target) else {
        tabs.insert(from, dragged);
        return false;
    };
    tabs.insert(to, dragged);
    true
}

#[cfg(test)]
mod editor_group_tests {
    use super::*;

    fn view() -> MainView {
        MainView::new(ApiClient::new())
    }

    fn dashboard(id: &str) -> WorkspaceTab {
        WorkspaceTab::Dashboard(id.to_string())
    }

    fn file(path: &str) -> WorkspaceTab {
        WorkspaceTab::File(path.to_string())
    }

    #[test]
    fn moves_tabs_between_groups_and_collapses_when_empty() {
        let mut view = view();
        view.workspace_tabs = vec![dashboard("a"), file("notes.txt"), dashboard("b")];
        view.split_at = Some(2);
        view.focused_group = 0;
        view.ensure_group_actives();
        assert_eq!(view.group_tabs(0), &[dashboard("a"), file("notes.txt")]);
        assert_eq!(view.group_tabs(1), &[dashboard("b")]);

        assert_eq!(
            view.move_tab_to_group_at(&file("notes.txt"), 1, Some(0)),
            Some(0)
        );
        assert_eq!(view.group_tabs(0), &[dashboard("a")]);
        assert_eq!(view.group_tabs(1), &[file("notes.txt"), dashboard("b")]);

        assert_eq!(view.move_tab_to_group_at(&dashboard("a"), 1, None), Some(0));
        assert!(!view.split_visible());
        assert_eq!(
            view.workspace_tabs,
            vec![file("notes.txt"), dashboard("b"), dashboard("a")]
        );
    }

    #[test]
    fn removing_the_last_tab_in_a_group_collapses_the_split() {
        let mut view = view();
        view.workspace_tabs = vec![dashboard("a"), dashboard("b")];
        view.split_at = Some(1);
        view.group_active = [Some(dashboard("a")), Some(dashboard("b"))];
        view.focused_group = 1;

        assert_eq!(view.remove_tab_everywhere(&dashboard("b")), Some(1));
        assert!(!view.split_visible());
        assert_eq!(view.workspace_tabs, vec![dashboard("a")]);
        // The remaining group keeps its active tab after the collapse.
        assert_eq!(view.group_active[0], Some(dashboard("a")));
        assert_eq!(view.focused_group, 0);
    }

    #[test]
    fn clamps_split_ratio_to_keep_both_groups_visible() {
        assert_eq!(clamp_split_ratio(0.0), 0.18);
        assert_eq!(clamp_split_ratio(1.0), 0.82);
        assert_eq!(clamp_split_ratio(0.5), 0.5);
        assert_eq!(clamp_split_ratio(f32::NAN), 0.5);
    }

    #[test]
    fn insert_respects_the_group_boundary() {
        let mut view = view();
        view.workspace_tabs = vec![dashboard("a"), dashboard("b")];
        view.split_at = Some(1);

        view.insert_tab_in_group(0, 1, file("first.txt"));
        assert_eq!(
            view.workspace_tabs,
            vec![dashboard("a"), file("first.txt"), dashboard("b")]
        );
        assert_eq!(view.split_at, Some(2));

        view.insert_tab_in_group(1, 0, file("second.txt"));
        assert_eq!(
            view.workspace_tabs,
            vec![
                dashboard("a"),
                file("first.txt"),
                file("second.txt"),
                dashboard("b")
            ]
        );
    }
}

#[cfg(test)]
mod workspace_tab_tests {
    use super::{WorkspaceTab, reorder_workspace_tabs};

    fn tabs(spec: &[&str]) -> Vec<WorkspaceTab> {
        spec.iter()
            .map(|entry| match entry.strip_prefix("f:") {
                Some(path) => WorkspaceTab::File(path.to_string()),
                None => WorkspaceTab::Dashboard((*entry).to_string()),
            })
            .collect()
    }

    fn dashboards(order: &[WorkspaceTab]) -> Vec<String> {
        order
            .iter()
            .filter_map(|tab| match tab {
                WorkspaceTab::Dashboard(id) => Some(id.clone()),
                WorkspaceTab::File(_) => None,
            })
            .collect()
    }

    #[test]
    fn moves_file_between_dashboards() {
        let mut order = tabs(&["a", "b", "f:notes.txt"]);
        assert!(reorder_workspace_tabs(
            &mut order,
            &WorkspaceTab::File("notes.txt".into()),
            &WorkspaceTab::Dashboard("b".into()),
        ));
        assert_eq!(order, tabs(&["a", "f:notes.txt", "b"]));
        assert_eq!(dashboards(&order), vec!["a", "b"]);
    }

    #[test]
    fn moves_dashboard_after_file() {
        let mut order = tabs(&["a", "f:notes.txt", "b"]);
        assert!(reorder_workspace_tabs(
            &mut order,
            &WorkspaceTab::Dashboard("a".into()),
            &WorkspaceTab::Dashboard("b".into()),
        ));
        assert_eq!(order, tabs(&["f:notes.txt", "a", "b"]));
    }

    #[test]
    fn rejects_same_or_missing_tabs() {
        let mut order = tabs(&["a", "f:notes.txt"]);
        assert!(!reorder_workspace_tabs(
            &mut order,
            &WorkspaceTab::Dashboard("a".into()),
            &WorkspaceTab::Dashboard("a".into()),
        ));
        assert!(!reorder_workspace_tabs(
            &mut order,
            &WorkspaceTab::Dashboard("missing".into()),
            &WorkspaceTab::Dashboard("a".into()),
        ));
        assert!(!reorder_workspace_tabs(
            &mut order,
            &WorkspaceTab::Dashboard("a".into()),
            &WorkspaceTab::Dashboard("missing".into()),
        ));
        assert_eq!(order, tabs(&["a", "f:notes.txt"]));
    }
}

fn path_is_or_under(path: &str, parent: &str) -> bool {
    path == parent
        || path
            .strip_prefix(parent)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

#[cfg(test)]
mod terminal_selector_tests {
    use super::*;

    fn module(id: &str, name: &str) -> Module {
        Module {
            id: Some(id.to_string()),
            user_id: None,
            module_name: name.to_string(),
            module: serde_json::json!({}),
            created_at: None,
            updated_at: None,
        }
    }

    fn view_with(modules: Vec<Module>) -> MainView {
        let mut view = MainView::new(ApiClient::new());
        view.dashboard_modules = modules;
        view
    }

    #[test]
    fn resolves_by_exact_name_and_path() {
        let view = view_with(vec![
            module("Crypto.finter.json", "Crypto"),
            module("Research/Macro.finter.json", "Macro"),
        ]);
        assert_eq!(
            view.find_dashboard_path("Crypto").as_deref(),
            Some("Crypto.finter.json")
        );
        assert_eq!(
            view.find_dashboard_path("Research/Macro.finter.json")
                .as_deref(),
            Some("Research/Macro.finter.json")
        );
        assert_eq!(
            view.find_dashboard_path("Macro").as_deref(),
            Some("Research/Macro.finter.json")
        );
    }

    #[test]
    fn refuses_to_guess_between_duplicate_names() {
        let view = view_with(vec![
            module("A/Shared.finter.json", "Shared"),
            module("B/Shared.finter.json", "Shared"),
        ]);
        assert_eq!(view.find_dashboard_path("Shared"), None);
        assert_eq!(
            view.find_dashboard_path("A/Shared.finter.json").as_deref(),
            Some("A/Shared.finter.json")
        );
    }

    #[test]
    fn host_guard_covers_subfolders() {
        assert!(path_is_or_under("Research/Macro.finter.json", "Research"));
        assert!(!path_is_or_under(
            "ResearchOther/Macro.finter.json",
            "Research"
        ));
    }
}
