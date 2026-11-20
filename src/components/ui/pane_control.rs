//! Cross-window dashboard registry and Agent-driven pane-grid control.
//!
//! Every open dashboard — whether it lives in a tab of the main window or in its
//! own detached window — registers its `HomePage` here. The Agent module (which
//! itself runs inside one of those dashboards) resolves a target dashboard from
//! this registry and applies a [`UiAction`] to it directly, so a single Agent
//! conversation can rearrange panes on any screen of the terminal.
//!
//! The registry lives in the component layer because it holds `WeakEntity`
//! handles to `HomePage`; the wire types it consumes live in
//! [`crate::data::pane_control`] so the data layer stays UI-free.
use gpui::{App, Global, WeakEntity};

use crate::components::pages::home::HomePage;
use crate::data::pane_control::{ParsedUiAction, UiAction, UiActionResult, parse_axis};
use crate::data::pane_layout::{
    build_preset, config_for, ensure_agent_pane, find_agent_pane, find_pane, infer_base_config,
    is_agent_type, normalize_layout, normalize_module_type, MAX_PANES, MAX_RATIO, MIN_RATIO,
};
use crate::data::pane_tree::{Axis, ModuleData, PaneNode};

/// One open dashboard, reachable from any window.
#[derive(Clone)]
pub struct DashboardTarget {
    pub id: String,
    pub name: String,
    /// True when the dashboard is rendered in its own top-level window.
    pub detached: bool,
    pub home: WeakEntity<HomePage>,
}

/// Process-wide list of open dashboards. Populated by `MainView` for tabbed
/// dashboards and by each detached window for its own dashboard.
#[derive(Default)]
pub struct DashboardRegistry {
    targets: Vec<DashboardTarget>,
}

impl Global for DashboardRegistry {}

impl DashboardRegistry {
    /// Replace the tabbed-dashboard entries, keeping detached ones intact.
    pub fn sync_main(&mut self, entries: Vec<DashboardTarget>) {
        self.targets.retain(|target| target.detached);
        for entry in entries {
            self.upsert(entry);
        }
    }

    /// Add or refresh a single dashboard entry.
    pub fn register(&mut self, target: DashboardTarget) {
        self.upsert(target);
    }

    fn upsert(&mut self, target: DashboardTarget) {
        self.targets.retain(|existing| existing.id != target.id);
        self.targets.push(target);
    }

    /// Forget a dashboard that was closed or re-attached.
    pub fn remove(&mut self, id: &str) {
        self.targets.retain(|target| target.id != id);
    }

    /// Every entry whose `HomePage` is still alive.
    pub fn live(&self) -> Vec<DashboardTarget> {
        self.targets
            .iter()
            .filter(|target| target.home.upgrade().is_some())
            .cloned()
            .collect()
    }

    /// Resolve an Agent selector (`id`, `name`, `active`, `all`, or empty) into
    /// the dashboards it addresses.
    pub fn resolve(&self, selector: Option<&str>, active: Option<&str>) -> Vec<DashboardTarget> {
        let live = self.live();
        let selector = selector.map(str::trim).filter(|value| !value.is_empty());
        match selector {
            Some("all") | Some("*") => live,
            None | Some("active") | Some("current") => {
                let selected: Vec<DashboardTarget> = live
                    .iter()
                    .filter(|target| Some(target.id.as_str()) == active)
                    .cloned()
                    .collect();
                if selected.is_empty() {
                    live.into_iter().take(1).collect()
                } else {
                    selected
                }
            }
            Some(wanted) => live
                .into_iter()
                .filter(|target| target.id == wanted || target.name.eq_ignore_ascii_case(wanted))
                .collect(),
        }
    }
}

/// A compact, model-readable description of every open dashboard and its panes.
pub fn dashboard_catalog(cx: &App) -> serde_json::Value {
    let targets = cx
        .try_global::<DashboardRegistry>()
        .map(|registry| registry.live())
        .unwrap_or_default();
    let dashboards: Vec<serde_json::Value> = targets
        .iter()
        .filter_map(|target| {
            let home = target.home.upgrade()?;
            let panes = home
                .read(cx)
                .pane_grid
                .read(cx)
                .tree
                .as_ref()?
                .describe_panes();
            let panes: Vec<serde_json::Value> = panes
                .into_iter()
                .map(|pane| {
                    serde_json::json!({
                        "ref": format!("#{}", pane.index),
                        "id": pane.id,
                        "type": pane.module_type,
                        "path": pane.path,
                        "agent": is_agent_type(&pane.module_type),
                        "config": pane.config,
                    })
                })
                .collect();
            Some(serde_json::json!({
                "id": target.id,
                "name": target.name,
                "detached": target.detached,
                "panes": panes,
            }))
        })
        .collect();
    serde_json::json!({
        "count": dashboards.len(),
        "module_types": crate::data::pane_layout::MODULE_TYPES,
        "presets": crate::data::pane_layout::PRESETS,
        "max_panes": crate::data::pane_layout::MAX_PANES,
        "dashboards": dashboards,
    })
}

/// The id of the dashboard the user is currently working in.
///
/// `GlobalHomeView` is refreshed on every mouse-down inside a dashboard, so it
/// names the window/tab under the cursor rather than whichever dashboard
/// happened to render last. Falls back to the first live dashboard.
pub fn active_dashboard_id(cx: &App) -> Option<String> {
    let focused = cx
        .try_global::<crate::components::pages::home::GlobalHomeView>()
        .and_then(|global| global.view.upgrade())
        .map(|home| home.entity_id());
    let registry = cx.try_global::<DashboardRegistry>()?;
    let live = registry.live();
    if let Some(focused) = focused {
        if let Some(target) = live.iter().find(|target| {
            target
                .home
                .upgrade()
                .map(|home| home.entity_id() == focused)
                .unwrap_or(false)
        }) {
            return Some(target.id.clone());
        }
    }
    live.first().map(|target| target.id.clone())
}

/// Parses and applies every `ui` directive in an assistant reply.
///
/// `active` is the id of the dashboard the conversation belongs to, used when an
/// action omits its `dashboard` selector. Returns one receipt per addressed
/// dashboard, in directive order.
pub fn execute_ui_actions_in_reply(
    text: &str,
    active: Option<&str>,
    caller_pane: Option<&str>,
    cx: &mut App,
) -> Vec<UiActionResult> {
    let resolved_active = active
        .map(str::to_string)
        .or_else(|| active_dashboard_id(cx));
    let active = resolved_active.as_deref();
    let mut results = Vec::new();
    for parsed in crate::data::pane_control::parse_ui_actions_in_text(text) {
        match parsed {
            ParsedUiAction::Action(action) => {
                results.extend(execute_ui_action(&action, active, caller_pane, cx));
            }
            ParsedUiAction::Invalid(message) => {
                results.push(UiActionResult::failure("ui", message));
            }
        }
    }
    results
}

/// Applies one action to every dashboard its selector resolves to.
pub fn execute_ui_action(
    action: &UiAction,
    active: Option<&str>,
    caller_pane: Option<&str>,
    cx: &mut App,
) -> Vec<UiActionResult> {
    let targets = cx
        .try_global::<DashboardRegistry>()
        .map(|registry| registry.resolve(action.dashboard(), active))
        .unwrap_or_default();

    if targets.is_empty() {
        let available = cx
            .try_global::<DashboardRegistry>()
            .map(|registry| {
                registry
                    .live()
                    .iter()
                    .map(|target| format!("{} ({})", target.name, target.id))
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        let message = if available.is_empty() {
            "No dashboards are open.".to_string()
        } else {
            format!("No dashboard matches that selector. Open: {available}")
        };
        return vec![UiActionResult::failure(action.action_name(), message)];
    }

    let mut results = Vec::new();
    for target in targets {
        let dashboard_id = target.id.clone();
        let result = target
            .home
            .update(cx, |page, cx| page.apply_ui_action(action, caller_pane, cx))
            .unwrap_or_else(|_| {
                UiActionResult::failure(action.action_name(), "Dashboard was closed.")
            });
        let mut result = result.with_dashboard(dashboard_id);
        if let Some(pane) = action.pane() {
            result = result.with_pane(pane);
        }
        results.push(result);
    }
    results
}

/// Applies one action to a single dashboard's pane grid.
///
/// This is the only place that mutates a layout on the Agent's behalf, so every
/// command funnels through the same validation, notification, and save path the
/// mouse-driven controls use.
pub fn apply_action_to_grid(
    grid: &mut crate::components::ui::pane_grid::renderer::PaneGridRenderer,
    action: &UiAction,
    caller_pane: Option<&str>,
    cx: &mut gpui::Context<crate::components::ui::pane_grid::renderer::PaneGridRenderer>,
) -> UiActionResult {
    let name = action.action_name();
    let Some(tree) = grid.tree.clone() else {
        return UiActionResult::failure(name, "This dashboard has no layout loaded yet.");
    };

    let base = infer_base_config(&tree);
    let agent_count = tree
        .describe_panes()
        .iter()
        .filter(|pane| is_agent_type(&pane.module_type))
        .count();
    let is_agent_pane = |pane_id: &str| {
        find_pane(&tree, pane_id)
            .and_then(|data| data.module_type)
            .map(|module_type| is_agent_type(&module_type))
            .unwrap_or(false)
    };
    // The pane hosting this conversation must survive every action. With more
    // than one agent pane, only the caller's pane is protected.
    let protected_agent = |pane_id: &str| {
        is_agent_pane(pane_id) && (caller_pane == Some(pane_id) || agent_count <= 1)
    };

    // Resolve the pane reference once; every pane-scoped action needs it.
    let resolved_pane = action.pane().and_then(|reference| {
        tree.resolve_pane_reference(reference).or_else(|| {
            // `resize` targets a split, so fall through to split resolution.
            if matches!(action, UiAction::Resize { .. }) {
                tree.resolve_split_reference(reference)
            } else {
                None
            }
        })
    });

    if let Some(reference) = action.pane() {
        if resolved_pane.is_none() {
            let available = tree
                .describe_panes()
                .iter()
                .map(|pane| format!("#{} {} ({})", pane.index, pane.module_type, pane.id))
                .collect::<Vec<_>>()
                .join(", ");
            return UiActionResult::failure(
                name,
                format!("No pane matches '{reference}'. Panes: {available}"),
            );
        }
    }

    match action {
        UiAction::SetModule {
            module_type,
            config,
            ..
        } => {
            let pane_id = resolved_pane.clone().unwrap_or_default();
            let module_type = normalize_module_type(module_type);
            if protected_agent(&pane_id) && !is_agent_type(&module_type) {
                return UiActionResult::failure(
                    name,
                    "The agent pane cannot be replaced while it is the only agent pane.",
                );
            }
            let requested_empty = config.is_null()
                || config
                    .as_object()
                    .map(|object| object.is_empty())
                    .unwrap_or(false);
            let effective = if requested_empty {
                find_pane(&tree, &pane_id)
                    .map(|data| data.config)
                    .filter(|value| value.is_object())
                    .unwrap_or_else(|| config.clone())
            } else {
                config.clone()
            };
            let config = config_for(&module_type, &base, &effective);
            let mut updated = tree.clone();
            let mut applied = false;
            updated.modify_node(&pane_id, &mut |node| {
                if let PaneNode::Pane { data, .. } = node {
                    data.module_type = Some(module_type.clone());
                    data.config = config.clone();
                    applied = true;
                }
            });
            if !applied {
                return UiActionResult::failure(
                    name,
                    "Pane disappeared before it could be updated.",
                );
            }
            grid.set_tree(updated, cx);
            UiActionResult::success(name, format!("Pane is now a {module_type} module."))
        }
        UiAction::UpdateConfig { config, .. } => {
            let pane_id = resolved_pane.clone().unwrap_or_default();
            let mut updated = tree.clone();
            let config = config.clone();
            let mut applied = false;
            updated.modify_node(&pane_id, &mut |node| {
                if let PaneNode::Pane { data, .. } = node {
                    if let (Some(current), Some(update)) =
                        (data.config.as_object_mut(), config.as_object())
                    {
                        current.extend(update.clone());
                    } else if !config.is_null() {
                        data.config = config.clone();
                    }
                    applied = true;
                }
            });
            if !applied {
                return UiActionResult::failure(
                    name,
                    "Pane disappeared before it could be updated.",
                );
            }
            grid.set_tree(updated, cx);
            UiActionResult::success(name, "Pane config updated.")
        }
        UiAction::Split {
            axis,
            module_type,
            config,
            ..
        } => {
            let pane_id = resolved_pane.clone().unwrap_or_default();
            let axis = parse_axis(axis.as_deref()).unwrap_or(Axis::Vertical);
            let right_data = match module_type.as_deref() {
                Some(raw) if !raw.trim().is_empty() => {
                    let kind = normalize_module_type(raw);
                    ModuleData {
                        module_type: Some(kind.clone()),
                        config: config_for(&kind, &base, config),
                    }
                }
                _ => ModuleData {
                    module_type: Some("empty".into()),
                    config: serde_json::json!({}),
                },
            };
            let mut updated = tree.clone();
            let mut applied = false;
            updated.modify_node(&pane_id, &mut |node| {
                if let PaneNode::Pane { id, data } = node.clone() {
                    let left = PaneNode::Pane {
                        id: id.clone(),
                        data: data.clone(),
                    };
                    let right = PaneNode::Pane {
                        id: PaneNode::generate_id(),
                        data: right_data.clone(),
                    };
                    *node = PaneNode::Split {
                        id: PaneNode::generate_id(),
                        axis: axis.clone(),
                        ratio: 0.5,
                        a: Box::new(left),
                        b: Box::new(right),
                    };
                    applied = true;
                }
            });
            if !applied {
                return UiActionResult::failure(name, "Pane disappeared before it could be split.");
            }
            grid.set_tree(updated, cx);
            UiActionResult::success(name, "Pane split in two.")
        }
        UiAction::SplitMatrix { rows, cols, .. } => {
            let pane_id = resolved_pane.clone().unwrap_or_default();
            if *rows == 0 || *cols == 0 || (*rows <= 1 && *cols <= 1) {
                return UiActionResult::failure(name, "A matrix needs at least two cells.");
            }
            if *rows > 6 || *cols > 6 {
                return UiActionResult::failure(
                    name,
                    "A matrix is limited to 6 rows and 6 columns.",
                );
            }
            let current_panes = tree.describe_panes().len();
            let additional = rows.saturating_mul(*cols).saturating_sub(1);
            if current_panes + additional > MAX_PANES {
                return UiActionResult::failure(
                    name,
                    format!(
                        "That matrix would create {} panes; the maximum is {MAX_PANES}.",
                        current_panes + additional
                    ),
                );
            }
            let mut updated = tree.clone();
            if !updated.split_matrix(&pane_id, *rows, *cols) {
                return UiActionResult::failure(name, "Pane disappeared before it could be split.");
            }
            if let Err(error) = normalize_layout(&mut updated, &base) {
                return UiActionResult::failure(name, error);
            }
            grid.set_tree(updated, cx);
            UiActionResult::success(name, format!("Pane replaced with a {rows}×{cols} grid."))
        }
        UiAction::Close { .. } => {
            let pane_id = resolved_pane.clone().unwrap_or_default();
            if protected_agent(&pane_id) {
                return UiActionResult::failure(
                    name,
                    "The agent pane cannot close itself. Open another agent pane first.",
                );
            }
            match tree.clone().remove_node(&pane_id) {
                Some(updated) => {
                    grid.set_tree(updated, cx);
                    UiActionResult::success(name, "Pane closed.")
                }
                None => UiActionResult::failure(
                    name,
                    "That is the last pane; a dashboard cannot be emptied.",
                ),
            }
        }
        UiAction::Swap { with_pane, .. } => {
            let pane_id = resolved_pane.clone().unwrap_or_default();
            let Some(other) = tree.resolve_pane_reference(with_pane) else {
                return UiActionResult::failure(name, format!("No pane matches '{with_pane}'."));
            };
            if other == pane_id {
                return UiActionResult::failure(name, "A pane cannot be swapped with itself.");
            }
            if is_agent_pane(&pane_id) || is_agent_pane(&other) {
                return UiActionResult::failure(
                    name,
                    "Agent panes cannot be swapped; use set_module or update_config instead.",
                );
            }
            let mut updated = tree.clone();
            if updated.swap_panes(&pane_id, &other) {
                grid.set_tree(updated, cx);
                UiActionResult::success(name, "Panes swapped.")
            } else {
                UiActionResult::failure(name, "Swap failed.")
            }
        }
        UiAction::Maximize { .. } => {
            let pane_id = resolved_pane.clone().unwrap_or_default();
            grid.maximized_pane = Some(pane_id);
            cx.notify();
            UiActionResult::success(name, "Pane maximized.")
        }
        UiAction::Restore { .. } => {
            grid.maximized_pane = None;
            cx.notify();
            UiActionResult::success(name, "Restored the full layout.")
        }
        UiAction::Focus { .. } => {
            let pane_id = resolved_pane.clone().unwrap_or_default();
            grid.active_pane_id = Some(pane_id);
            cx.notify();
            UiActionResult::success(name, "Pane focused.")
        }
        UiAction::Resize { ratio, .. } => {
            let split_id = resolved_pane.clone().unwrap_or_default();
            let ratio = ratio.clamp(MIN_RATIO, MAX_RATIO);
            let mut updated = tree.clone();
            let mut applied = false;
            updated.modify_node(&split_id, &mut |node| {
                if let PaneNode::Split { ratio: current, .. } = node {
                    *current = ratio;
                    applied = true;
                }
            });
            if !applied {
                return UiActionResult::failure(name, "No split matches that reference.");
            }
            grid.set_tree(updated, cx);
            UiActionResult::success(name, format!("Split resized to {:.0}%.", ratio * 100.0))
        }
        UiAction::SetLayout { layout, .. } => {
            match serde_json::from_value::<PaneNode>(layout.clone()) {
                Ok(mut updated) => {
                    if let Err(error) = normalize_layout(&mut updated, &base) {
                        return UiActionResult::failure(name, error);
                    }
                    let agent = find_agent_pane(&tree, caller_pane);
                    if let Err(error) = ensure_agent_pane(&mut updated, agent) {
                        return UiActionResult::failure(name, error);
                    }
                    grid.set_tree(updated, cx);
                    UiActionResult::success(
                        name,
                        "Dashboard layout replaced; the agent pane was preserved.",
                    )
                }
                Err(error) => UiActionResult::failure(
                    name,
                    format!("Layout is not a valid pane tree: {error}"),
                ),
            }
        }
        UiAction::ApplyPreset { preset, config, .. } => match build_preset(preset, &base, config) {
            Ok(mut updated) => {
                if let Err(error) = normalize_layout(&mut updated, &base) {
                    return UiActionResult::failure(name, error);
                }
                let agent = find_agent_pane(&tree, caller_pane);
                if let Err(error) = ensure_agent_pane(&mut updated, agent) {
                    return UiActionResult::failure(name, error);
                }
                grid.set_tree(updated, cx);
                UiActionResult::success(name, format!("Applied the '{preset}' preset."))
            }
            Err(error) => UiActionResult::failure(name, error),
        },
    }
}
