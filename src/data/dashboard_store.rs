//! Persistent dashboard storage for DockAreaState.
//!
//! Stores the dock layout (panels, splits, tabs) to `~/.config/finter/dashboards/`
//! so users can restore their workspace between sessions and manage multiple dashboards.
//!
//! # Usage
//! ```ignore
//! // Save a named dashboard
//! let state: DockAreaState = dock_area.dump(cx);
//! dashboard_store::save_dashboard("My Trading Dashboard", &state)?;
//!
//! // Load a specific dashboard
//! if let Some(state) = dashboard_store::load_dashboard("My Trading Dashboard")? {
//!     dock_area.load(state, ...)?;
//! }
//!
//! // List all dashboards
//! let dashboards = dashboard_store::list_dashboards()?;
//!
//! // Backward compatibility - single dashboard operations
//! dashboard_store::save(&state)?;  // Saves to default dashboard
//! dashboard_store::load()?;         // Loads default dashboard
//! ```

use chrono::{DateTime, Utc};
use gpui_component::dock::DockAreaState;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tracing::{debug, info};

use crate::data::modules::PaneData;
use gpui_component::pane_grid::Configuration;

/// Metadata about a saved dashboard.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashboardMetadata {
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub last_modified: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// A saved dashboard with its state and metadata (legacy DockAreaState format).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedDashboard {
    pub metadata: DashboardMetadata,
    pub state: DockAreaState,
}

/// A saved dashboard with Configuration<PaneData> format (new tree format).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedDashboardTree {
    pub metadata: DashboardMetadata,
    pub config: Configuration<PaneData>,
}

const DEFAULT_DASHBOARD_NAME: &str = "Default";

/// Get the path to the dashboards directory.
fn dashboards_dir() -> Result<PathBuf, String> {
    let mut path = dirs::config_dir().ok_or("cannot determine config directory")?;
    path.push("finter");
    path.push("dashboards");
    Ok(path)
}

/// Get the path to a specific dashboard file.
fn dashboard_file_path(name: &str) -> Result<PathBuf, String> {
    let mut path = dashboards_dir()?;
    // Sanitize the name to be filesystem-safe
    let safe_name = sanitize_filename(name);
    path.push(format!("{}.json", safe_name));
    Ok(path)
}

/// Get the path to a specific tree dashboard file (new format).
fn dashboard_tree_file_path(name: &str) -> Result<PathBuf, String> {
    let mut path = dashboards_dir()?;
    // Sanitize the name to be filesystem-safe
    let safe_name = sanitize_filename(name);
    path.push(format!("{}.tree.json", safe_name));
    Ok(path)
}

/// Get the path to the legacy single dashboard file (for backward compatibility).
fn legacy_dashboard_path() -> Result<PathBuf, String> {
    let mut path = dirs::config_dir().ok_or("cannot determine config directory")?;
    path.push("finter");
    path.push("layout.json");
    Ok(path)
}

/// Sanitize a dashboard name to be filesystem-safe.
fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            _ => c,
        })
        .collect()
}

/// Save a named dashboard to disk.
pub fn save_dashboard(name: &str, state: &DockAreaState) -> Result<(), String> {
    save_dashboard_with_description(name, state, None)
}

/// Save a named dashboard with description to disk.
pub fn save_dashboard_with_description(
    name: &str,
    state: &DockAreaState,
    description: Option<String>,
) -> Result<(), String> {
    let path = dashboard_file_path(name)?;

    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir: {e}"))?;
    }

    // Check if dashboard already exists to preserve created_at
    let created_at = match load_dashboard(name)? {
        Some(existing) => existing.metadata.created_at,
        None => Utc::now(),
    };

    let dashboard = SavedDashboard {
        metadata: DashboardMetadata {
            name: name.to_string(),
            created_at,
            last_modified: Utc::now(),
            description,
        },
        state: state.clone(),
    };

    let json = serde_json::to_string_pretty(&dashboard).map_err(|e| format!("serialize: {e}"))?;
    std::fs::write(&path, &json).map_err(|e| format!("write dashboard file: {e}"))?;

    info!("Dashboard '{}' saved to {:?}", name, path);
    Ok(())
}

/// Load a named dashboard from disk.
pub fn load_dashboard(name: &str) -> Result<Option<SavedDashboard>, String> {
    let path = dashboard_file_path(name)?;

    if !path.exists() {
        debug!("No saved dashboard '{}' at {:?}", name, path);
        return Ok(None);
    }

    let json = std::fs::read_to_string(&path).map_err(|e| format!("read dashboard file: {e}"))?;

    let dashboard: SavedDashboard =
        serde_json::from_str(&json).map_err(|e| format!("deserialize dashboard: {e}"))?;

    info!("Dashboard '{}' loaded from {:?}", name, path);
    Ok(Some(dashboard))
}

// ──────────────────────────────────────────────
// New tree format: Configuration<PaneData>
// ──────────────────────────────────────────────

/// Save a named dashboard to disk in tree format.
pub fn save_dashboard_tree(name: &str, config: &Configuration<PaneData>) -> Result<(), String> {
    save_dashboard_tree_with_description(name, config, None)
}

/// Save a named dashboard with description to disk in tree format.
pub fn save_dashboard_tree_with_description(
    name: &str,
    config: &Configuration<PaneData>,
    description: Option<String>,
) -> Result<(), String> {
    let path = dashboard_tree_file_path(name)?;

    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir: {e}"))?;
    }

    // Check if dashboard already exists to preserve created_at
    let created_at = match load_dashboard_tree(name)? {
        Some(existing) => existing.metadata.created_at,
        None => Utc::now(),
    };

    let dashboard = SavedDashboardTree {
        metadata: DashboardMetadata {
            name: name.to_string(),
            created_at,
            last_modified: Utc::now(),
            description,
        },
        config: config.clone(),
    };

    let json = serde_json::to_string_pretty(&dashboard).map_err(|e| format!("serialize: {e}"))?;
    std::fs::write(&path, &json).map_err(|e| format!("write tree dashboard file: {e}"))?;

    info!("Dashboard '{}' saved (tree format) to {:?}", name, path);
    Ok(())
}

/// Load a named dashboard from disk in tree format.
pub fn load_dashboard_tree(name: &str) -> Result<Option<SavedDashboardTree>, String> {
    let path = dashboard_tree_file_path(name)?;

    if !path.exists() {
        debug!("No saved tree dashboard '{}' at {:?}", name, path);
        return Ok(None);
    }

    let json =
        std::fs::read_to_string(&path).map_err(|e| format!("read tree dashboard file: {e}"))?;

    let dashboard: SavedDashboardTree =
        serde_json::from_str(&json).map_err(|e| format!("deserialize tree dashboard: {e}"))?;

    info!("Dashboard '{}' loaded (tree format) from {:?}", name, path);
    Ok(Some(dashboard))
}

/// List all saved dashboards.
pub fn list_dashboards() -> Result<Vec<DashboardMetadata>, String> {
    let dir = dashboards_dir()?;

    if !dir.exists() {
        return Ok(Vec::new());
    }

    let entries = std::fs::read_dir(&dir).map_err(|e| format!("read dashboards dir: {e}"))?;

    let mut dashboards = Vec::new();

    for entry in entries {
        let entry = entry.map_err(|e| format!("read dir entry: {e}"))?;
        let path = entry.path();

        if path.extension().and_then(|s| s.to_str()) == Some("json") {
            let json = std::fs::read_to_string(&path).map_err(|e| format!("read file: {e}"))?;

            if let Ok(dashboard) = serde_json::from_str::<SavedDashboard>(&json) {
                dashboards.push(dashboard.metadata);
            }
        }
    }

    // Sort by last modified (newest first)
    dashboards.sort_by_key(|d| std::cmp::Reverse(d.last_modified));

    Ok(dashboards)
}

/// Delete a named dashboard.
pub fn delete_dashboard(name: &str) -> Result<(), String> {
    let path = dashboard_file_path(name)?;

    if path.exists() {
        std::fs::remove_file(&path).map_err(|e| format!("delete dashboard file: {e}"))?;
        info!("Dashboard '{}' deleted", name);
    }

    Ok(())
}

/// Delete a named dashboard in tree format.
pub fn delete_dashboard_tree(name: &str) -> Result<(), String> {
    let path = dashboard_tree_file_path(name)?;

    if path.exists() {
        std::fs::remove_file(&path).map_err(|e| format!("delete tree dashboard file: {e}"))?;
        info!("Dashboard '{}' deleted (tree format)", name);
    }

    Ok(())
}

/// Rename a dashboard.
pub fn rename_dashboard(old_name: &str, new_name: &str) -> Result<(), String> {
    // Load the old dashboard
    let mut dashboard =
        load_dashboard(old_name)?.ok_or_else(|| format!("Dashboard '{}' not found", old_name))?;

    // Update metadata
    dashboard.metadata.name = new_name.to_string();
    dashboard.metadata.last_modified = Utc::now();

    // Save with new name
    save_dashboard_with_description(new_name, &dashboard.state, dashboard.metadata.description)?;

    // Delete old file
    delete_dashboard(old_name)?;

    Ok(())
}

/// Duplicate a dashboard with a new name.
pub fn duplicate_dashboard(source_name: &str, new_name: &str) -> Result<(), String> {
    let source = load_dashboard(source_name)?
        .ok_or_else(|| format!("Dashboard '{}' not found", source_name))?;

    save_dashboard_with_description(new_name, &source.state, source.metadata.description)?;

    Ok(())
}

// ──────────────────────────────────────────────
// Backward compatibility - single dashboard operations
// ──────────────────────────────────────────────

/// Save the dock area state to disk (backward compatible).
/// This saves to the default dashboard.
pub fn save(state: &DockAreaState) -> Result<(), String> {
    save_dashboard(DEFAULT_DASHBOARD_NAME, state)
}

/// Save the tree configuration to disk (new format).
/// This saves to the default dashboard.
pub fn save_tree(config: &Configuration<PaneData>) -> Result<(), String> {
    save_dashboard_tree(DEFAULT_DASHBOARD_NAME, config)
}

/// Load the dock area state from disk (backward compatible).
/// This loads the default dashboard, or migrates from legacy single dashboard file.
/// Returns `Ok(None)` if no dashboard exists.
pub fn load() -> Result<Option<DockAreaState>, String> {
    // First, try to load default dashboard
    if let Some(dashboard) = load_dashboard(DEFAULT_DASHBOARD_NAME)? {
        return Ok(Some(dashboard.state));
    }

    // Try to migrate from legacy single dashboard file
    let legacy_path = legacy_dashboard_path()?;
    if legacy_path.exists() {
        let json = std::fs::read_to_string(&legacy_path)
            .map_err(|e| format!("read legacy dashboard file: {e}"))?;

        let state: DockAreaState = serde_json::from_str(&json)
            .map_err(|e| format!("deserialize legacy dashboard: {e}"))?;

        // Save as default dashboard in new format
        save_dashboard(DEFAULT_DASHBOARD_NAME, &state)?;

        // Optionally delete legacy file (commented out for safety)
        // std::fs::remove_file(&legacy_path).ok();

        info!("Migrated legacy dashboard to default dashboard");
        return Ok(Some(state));
    }

    Ok(None)
}

/// Load the tree configuration from disk (new format).
/// This loads the default dashboard.
/// Returns `Ok(None)` if no dashboard exists.
pub fn load_tree() -> Result<Option<Configuration<PaneData>>, String> {
    if let Some(dashboard) = load_dashboard_tree(DEFAULT_DASHBOARD_NAME)? {
        return Ok(Some(dashboard.config));
    }
    Ok(None)
}

/// Delete the saved dashboard file (backward compatible).
/// This deletes the default dashboard.
pub fn clear() -> Result<(), String> {
    delete_dashboard(DEFAULT_DASHBOARD_NAME)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_legacy_dashboard_path_resolves() {
        let path = legacy_dashboard_path();
        assert!(path.is_ok());
        let path = path.unwrap();
        assert!(path.to_string_lossy().contains("finter"));
        assert!(path.to_string_lossy().ends_with("layout.json"));
    }

    #[test]
    fn test_dashboards_dir_resolves() {
        let dir = dashboards_dir();
        assert!(dir.is_ok());
        let dir = dir.unwrap();
        assert!(dir.to_string_lossy().contains("finter"));
        assert!(dir.to_string_lossy().ends_with("dashboards"));
    }

    #[test]
    fn test_sanitize_filename() {
        assert_eq!(sanitize_filename("My Dashboard"), "My Dashboard");
        assert_eq!(sanitize_filename("My/Dashboard"), "My_Dashboard");
        assert_eq!(sanitize_filename("Dashboard:2024"), "Dashboard_2024");
        assert_eq!(sanitize_filename("Test<>Dashboard"), "Test__Dashboard");
    }
}
