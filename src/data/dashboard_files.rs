//! File-backed dashboard storage.
//!
//! Dashboards live as one JSON file each under `~/.config/finter/dashboards/`,
//! named by a stable UUID (`<id>.json`). The filename is the identity; the
//! display name lives inside the file and may change freely without moving it.
//!
//! This replaces the Supabase `modules` and `layout_presets` tables as the
//! source of truth. The on-disk payload keeps the same shape the tables held
//! (`tree` / `module` was already a serialized `PaneNode`), so migrating an
//! existing row is a straight JSON copy with added metadata.
//!
//! # Layout
//!
//! ```text
//! ~/.config/finter/dashboards/
//!   0f8c…-….json   { "id", "name", "tree", "is_public", "created_at", "updated_at" }
//! ```
//!
//! # Usage
//! ```ignore
//! let d = dashboard_files::create("My Dashboard", tree)?;
//! let all = dashboard_files::list()?;
//! dashboard_files::update(&d.id, Some("Renamed"), Some(tree))?;
//! dashboard_files::delete(&d.id)?;
//! ```

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tracing::{debug, info};

use crate::data::error::{CoinWatchError, Result};

/// A dashboard persisted as a single JSON file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashboardFile {
    /// Stable identifier; also the file stem.
    pub id: String,
    /// Human-readable name, editable without renaming the file.
    #[serde(default)]
    pub name: String,
    /// Serialized `PaneNode` tree.
    #[serde(default)]
    pub tree: serde_json::Value,
    #[serde(default)]
    pub is_public: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl DashboardFile {
    /// Convert to the wire shape the UI already consumes (`models::Module`).
    pub fn to_module(&self) -> crate::data::models::Module {
        crate::data::models::Module {
            id: Some(self.id.clone()),
            user_id: None,
            module_name: self.name.clone(),
            module: self.tree.clone(),
            created_at: Some(self.created_at.to_rfc3339()),
            updated_at: Some(self.updated_at.to_rfc3339()),
        }
    }

    /// Convert to the wire shape the UI already consumes (`models::LayoutPreset`).
    pub fn to_preset(&self) -> crate::data::models::LayoutPreset {
        crate::data::models::LayoutPreset {
            id: Some(self.id.clone()),
            user_id: None,
            name: self.name.clone(),
            tree: self.tree.clone(),
            is_public: Some(self.is_public),
            created_at: Some(self.created_at.to_rfc3339()),
            updated_at: Some(self.updated_at.to_rfc3339()),
        }
    }
}

/// Directory holding dashboard files.
pub fn dashboards_dir() -> Result<PathBuf> {
    let mut path = dirs::config_dir()
        .ok_or_else(|| CoinWatchError::Storage("cannot determine config directory".into()))?;
    path.push("finter");
    path.push("dashboards");
    Ok(path)
}

fn dashboard_path(dir: &Path, id: &str) -> Result<PathBuf> {
    if !is_valid_id(id) {
        return Err(CoinWatchError::InvalidRequest(format!(
            "invalid dashboard id: {id:?}"
        )));
    }
    Ok(dir.join(format!("{id}.json")))
}

/// Dashboard ids are UUIDs; reject anything that could escape the directory.
fn is_valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn read_file(path: &Path) -> Result<DashboardFile> {
    let json = std::fs::read_to_string(path)
        .map_err(|e| CoinWatchError::Storage(format!("read {}: {e}", path.display())))?;

    // Older builds wrote the bare `PaneNode` tree into this directory without
    // an envelope. Accept those and lift them into the current shape, using the
    // file stem as the id, so upgrading does not orphan a user's dashboards.
    let value: serde_json::Value = serde_json::from_str(&json)
        .map_err(|e| CoinWatchError::Deserialization(format!("parse {}: {e}", path.display())))?;

    if value.get("id").is_some() && value.get("tree").is_some() {
        return serde_json::from_value(value)
            .map_err(|e| CoinWatchError::Deserialization(format!("parse {}: {e}", path.display())));
    }

    let id = path
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| {
            CoinWatchError::Storage(format!("dashboard file has no stem: {}", path.display()))
        })?
        .to_string();

    let mtime = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .map(DateTime::<Utc>::from)
        .unwrap_or_else(Utc::now);

    debug!("lifting legacy bare-tree dashboard {} into envelope", path.display());
    Ok(DashboardFile {
        id,
        name: String::new(),
        tree: value,
        is_public: false,
        created_at: mtime,
        updated_at: mtime,
    })
}

fn write_file(path: &Path, dashboard: &DashboardFile) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| CoinWatchError::Storage(format!("mkdir {}: {e}", parent.display())))?;
    }
    let json = serde_json::to_string_pretty(dashboard)
        .map_err(|e| CoinWatchError::Deserialization(format!("serialize dashboard: {e}")))?;

    // Write to a sibling temp file then rename, so a crash mid-write cannot
    // leave a truncated dashboard behind.
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &json)
        .map_err(|e| CoinWatchError::Storage(format!("write {}: {e}", tmp.display())))?;
    std::fs::rename(&tmp, path)
        .map_err(|e| CoinWatchError::Storage(format!("rename to {}: {e}", path.display())))?;
    Ok(())
}

// ──────────────────────────────────────────────
// Directory-scoped operations
//
// The public API below resolves the user's config directory; these take it
// explicitly so tests (and a future portable/R2 staging mode) can point at an
// arbitrary directory.
// ──────────────────────────────────────────────

/// List every dashboard in `dir`, newest-updated first.
///
/// Files that fail to parse are logged and skipped rather than failing the
/// whole listing — one corrupt file must not hide the user's other dashboards.
pub fn list_in(dir: &Path) -> Result<Vec<DashboardFile>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let entries = std::fs::read_dir(dir)
        .map_err(|e| CoinWatchError::Storage(format!("read {}: {e}", dir.display())))?;

    let mut dashboards = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| CoinWatchError::Storage(format!("dir entry: {e}")))?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        match read_file(&path) {
            Ok(dashboard) => dashboards.push(dashboard),
            Err(e) => tracing::warn!("skipping unreadable dashboard {}: {e}", path.display()),
        }
    }

    dashboards.sort_by_key(|d| std::cmp::Reverse(d.updated_at));
    Ok(dashboards)
}

/// Load one dashboard by id from `dir`.
pub fn load_in(dir: &Path, id: &str) -> Result<Option<DashboardFile>> {
    let path = dashboard_path(dir, id)?;
    if !path.exists() {
        debug!("no dashboard {id} at {}", path.display());
        return Ok(None);
    }
    read_file(&path).map(Some)
}

/// Write a dashboard with an explicit id, creating or replacing it.
pub fn upsert_in(dir: &Path, dashboard: DashboardFile) -> Result<()> {
    write_file(&dashboard_path(dir, &dashboard.id)?, &dashboard)
}

/// Delete a dashboard by id from `dir`. Missing files are not an error.
pub fn delete_in(dir: &Path, id: &str) -> Result<()> {
    let path = dashboard_path(dir, id)?;
    if path.exists() {
        std::fs::remove_file(&path)
            .map_err(|e| CoinWatchError::Storage(format!("delete {}: {e}", path.display())))?;
        info!("deleted dashboard {id}");
    }
    Ok(())
}

// ──────────────────────────────────────────────
// Public API (uses the user's config directory)
// ──────────────────────────────────────────────

/// List every dashboard on disk, newest-updated first.
pub fn list() -> Result<Vec<DashboardFile>> {
    list_in(&dashboards_dir()?)
}

/// List dashboards in the `Module` wire shape the dashboard manager expects.
pub fn list_modules() -> Result<Vec<crate::data::models::Module>> {
    Ok(list()?.iter().map(DashboardFile::to_module).collect())
}

/// List dashboards in the `LayoutPreset` wire shape.
pub fn list_presets() -> Result<Vec<crate::data::models::LayoutPreset>> {
    Ok(list()?.iter().map(DashboardFile::to_preset).collect())
}

/// Load one dashboard by id.
pub fn load(id: &str) -> Result<Option<DashboardFile>> {
    load_in(&dashboards_dir()?, id)
}

/// Create a new dashboard and return it (with its generated id).
pub fn create(name: &str, tree: serde_json::Value) -> Result<DashboardFile> {
    let dir = dashboards_dir()?;
    let now = Utc::now();
    let dashboard = DashboardFile {
        id: uuid::Uuid::new_v4().to_string(),
        name: name.to_string(),
        tree,
        is_public: false,
        created_at: now,
        updated_at: now,
    };
    upsert_in(&dir, dashboard.clone())?;
    info!("created dashboard '{}' ({})", dashboard.name, dashboard.id);
    Ok(dashboard)
}

/// Update a dashboard's name and/or tree. Returns the updated record.
pub fn update(
    id: &str,
    name: Option<&str>,
    tree: Option<serde_json::Value>,
) -> Result<DashboardFile> {
    let dir = dashboards_dir()?;
    let path = dashboard_path(&dir, id)?;
    let mut dashboard = read_file(&path)?;

    if let Some(name) = name {
        dashboard.name = name.to_string();
    }
    if let Some(tree) = tree {
        dashboard.tree = tree;
    }
    dashboard.updated_at = Utc::now();

    write_file(&path, &dashboard)?;
    info!("updated dashboard '{}' ({})", dashboard.name, dashboard.id);
    Ok(dashboard)
}

/// Write a dashboard with an explicit id, creating or replacing it.
pub fn upsert(dashboard: DashboardFile) -> Result<()> {
    upsert_in(&dashboards_dir()?, dashboard)
}

/// Delete a dashboard by id. Missing files are not an error.
pub fn delete(id: &str) -> Result<()> {
    delete_in(&dashboards_dir()?, id)
}

/// Save a tree into an existing dashboard, creating one named `name` if the
/// id is absent or no longer on disk.
///
/// This is the shape the UI's "save layout" path needs: it holds an id it got
/// from a previous listing, and must not fail just because the file vanished.
pub fn save_tree(id: Option<&str>, name: &str, tree: serde_json::Value) -> Result<DashboardFile> {
    if let Some(id) = id {
        if load(id)?.is_some() {
            return update(id, Some(name), Some(tree));
        }
    }
    create(name, tree)
}

// ──────────────────────────────────────────────
// Migration from the legacy tables
// ──────────────────────────────────────────────

/// One row read out of `modules` or `layout_presets`.
#[derive(Debug, Clone, PartialEq)]
pub struct LegacyRow {
    pub id: String,
    pub name: String,
    pub tree: serde_json::Value,
    pub is_public: bool,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    /// True when the row came from `layout_presets`.
    pub from_preset: bool,
}

/// Outcome of one migration run.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct MigrationReport {
    pub written: usize,
    pub skipped_existing: usize,
    pub failed: usize,
    pub from_presets: usize,
}

/// Write legacy table rows out as dashboard files.
///
/// Rows whose id already exists on disk are skipped unless `overwrite` is set,
/// which makes the migration safe to re-run. Preset rows win over module rows
/// sharing an id, because only presets carry `is_public`.
pub fn migrate_rows_in(
    dir: &Path,
    rows: Vec<LegacyRow>,
    overwrite: bool,
) -> Result<MigrationReport> {
    let mut report = MigrationReport::default();

    // Presets first so a shared id keeps its is_public flag.
    let mut ordered: Vec<LegacyRow> = rows.iter().filter(|r| r.from_preset).cloned().collect();
    ordered.extend(rows.into_iter().filter(|r| !r.from_preset));

    let mut seen: Vec<String> = Vec::new();
    for row in ordered {
        if seen.contains(&row.id) {
            continue;
        }
        seen.push(row.id.clone());

        match load_in(dir, &row.id) {
            Ok(Some(_)) if !overwrite => {
                report.skipped_existing += 1;
                continue;
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!("could not check existing dashboard {}: {e}", row.id);
                report.failed += 1;
                continue;
            }
        }

        let dashboard = DashboardFile {
            id: row.id,
            name: row.name,
            tree: row.tree,
            is_public: row.is_public,
            created_at: parse_ts(row.created_at.as_deref()),
            updated_at: parse_ts(row.updated_at.as_deref()),
        };

        match upsert_in(dir, dashboard) {
            Ok(()) => {
                report.written += 1;
                if row.from_preset {
                    report.from_presets += 1;
                }
            }
            Err(e) => {
                tracing::error!("failed to write migrated dashboard: {e}");
                report.failed += 1;
            }
        }
    }

    info!(
        "dashboard migration: {} written ({} from presets), {} already present, {} failed",
        report.written, report.from_presets, report.skipped_existing, report.failed
    );
    Ok(report)
}

/// Write legacy rows into the user's dashboards directory.
pub fn migrate_rows(rows: Vec<LegacyRow>, overwrite: bool) -> Result<MigrationReport> {
    migrate_rows_in(&dashboards_dir()?, rows, overwrite)
}

/// Parse an RFC3339 timestamp, falling back to now for missing/garbage values.
fn parse_ts(value: Option<&str>) -> DateTime<Utc> {
    value
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(Utc::now)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "finter-dash-test-{tag}-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample(id: &str, name: &str) -> DashboardFile {
        let now = Utc::now();
        DashboardFile {
            id: id.into(),
            name: name.into(),
            tree: serde_json::json!({"Pane": {}}),
            is_public: false,
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn rejects_path_traversal_ids() {
        assert!(!is_valid_id("../etc/passwd"));
        assert!(!is_valid_id("a/b"));
        assert!(!is_valid_id(""));
        assert!(!is_valid_id("has space"));
        assert!(!is_valid_id(&"x".repeat(65)));
    }

    #[test]
    fn accepts_uuid_and_simple_ids() {
        assert!(is_valid_id("0f8c9a2e-1b3d-4c5f-8a9b-0c1d2e3f4a5b"));
        assert!(is_valid_id("default"));
        assert!(is_valid_id("a_b-c"));
    }

    #[test]
    fn module_and_preset_shapes_carry_identity() {
        let d = sample("abc", "My Dash");
        let m = d.to_module();
        assert_eq!(m.id.as_deref(), Some("abc"));
        assert_eq!(m.module_name, "My Dash");

        let p = d.to_preset();
        assert_eq!(p.id.as_deref(), Some("abc"));
        assert_eq!(p.name, "My Dash");
    }

    #[test]
    fn round_trips_through_disk() {
        let dir = temp_dir("roundtrip");
        upsert_in(&dir, sample("id-1", "First")).unwrap();

        let loaded = load_in(&dir, "id-1").unwrap().expect("should exist");
        assert_eq!(loaded.name, "First");
        assert_eq!(loaded.tree, serde_json::json!({"Pane": {}}));

        let all = list_in(&dir).unwrap();
        assert_eq!(all.len(), 1);

        delete_in(&dir, "id-1").unwrap();
        assert!(load_in(&dir, "id-1").unwrap().is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn list_skips_corrupt_files_without_failing() {
        let dir = temp_dir("corrupt");
        upsert_in(&dir, sample("good", "Good")).unwrap();
        std::fs::write(dir.join("bad.json"), "{not json").unwrap();

        let all = list_in(&dir).unwrap();
        assert_eq!(all.len(), 1, "corrupt file should be skipped");
        assert_eq!(all[0].id, "good");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn list_is_newest_updated_first() {
        let dir = temp_dir("order");
        let mut older = sample("old", "Old");
        older.updated_at = Utc::now() - chrono::Duration::hours(2);
        upsert_in(&dir, older).unwrap();
        upsert_in(&dir, sample("new", "New")).unwrap();

        let all = list_in(&dir).unwrap();
        assert_eq!(all[0].id, "new");
        assert_eq!(all[1].id, "old");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn migration_preserves_ids_and_public_flag() {
        let dir = temp_dir("migrate");
        let rows = vec![
            LegacyRow {
                id: "preset-1".into(),
                name: "Shared".into(),
                tree: serde_json::json!({"a": 1}),
                is_public: true,
                created_at: Some("2026-01-01T00:00:00Z".into()),
                updated_at: Some("2026-01-02T00:00:00Z".into()),
                from_preset: true,
            },
            LegacyRow {
                id: "module-1".into(),
                name: "Mine".into(),
                tree: serde_json::json!({"b": 2}),
                is_public: false,
                created_at: None,
                updated_at: None,
                from_preset: false,
            },
        ];

        let report = migrate_rows_in(&dir, rows, false).unwrap();
        assert_eq!(report.written, 2);
        assert_eq!(report.from_presets, 1);
        assert_eq!(report.failed, 0);

        let preset = load_in(&dir, "preset-1").unwrap().unwrap();
        assert_eq!(preset.name, "Shared");
        assert!(preset.is_public);
        assert_eq!(
            preset.updated_at.to_rfc3339(),
            "2026-01-02T00:00:00+00:00",
            "timestamp should be preserved"
        );

        let module = load_in(&dir, "module-1").unwrap().unwrap();
        assert_eq!(module.name, "Mine");
        assert!(!module.is_public);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn migration_is_idempotent_and_respects_overwrite() {
        let dir = temp_dir("idempotent");
        let rows = || {
            vec![LegacyRow {
                id: "dup".into(),
                name: "Original".into(),
                tree: serde_json::json!({}),
                is_public: false,
                created_at: None,
                updated_at: None,
                from_preset: false,
            }]
        };

        let first = migrate_rows_in(&dir, rows(), false).unwrap();
        assert_eq!(first.written, 1);

        // Second run must not rewrite.
        let second = migrate_rows_in(&dir, rows(), false).unwrap();
        assert_eq!(second.written, 0);
        assert_eq!(second.skipped_existing, 1);

        // With overwrite it writes again.
        let third = migrate_rows_in(&dir, rows(), true).unwrap();
        assert_eq!(third.written, 1);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn lifts_legacy_bare_tree_file_into_envelope() {
        let dir = temp_dir("legacy-lift");
        // Older builds wrote the raw PaneNode tree with no envelope.
        std::fs::write(
            dir.join("legacy-id.json"),
            r#"{"Pane":{"id":"p1","data":{"module_type":"chart","config":{}}}}"#,
        )
        .unwrap();

        let loaded = load_in(&dir, "legacy-id").unwrap().expect("should lift");
        assert_eq!(loaded.id, "legacy-id");
        assert_eq!(loaded.tree["Pane"]["id"], "p1");
        assert!(!loaded.is_public);
        assert!(loaded.name.is_empty());

        // And it still appears in listings.
        let all = list_in(&dir).unwrap();
        assert_eq!(all.len(), 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn migration_prefers_preset_over_module_with_same_id() {
        let dir = temp_dir("preset-wins");
        let rows = vec![
            LegacyRow {
                id: "shared".into(),
                name: "From Module".into(),
                tree: serde_json::json!({"m": 1}),
                is_public: false,
                created_at: None,
                updated_at: None,
                from_preset: false,
            },
            LegacyRow {
                id: "shared".into(),
                name: "From Preset".into(),
                tree: serde_json::json!({"p": 1}),
                is_public: true,
                created_at: None,
                updated_at: None,
                from_preset: true,
            },
        ];

        migrate_rows_in(&dir, rows, false).unwrap();
        let loaded = load_in(&dir, "shared").unwrap().unwrap();
        assert_eq!(loaded.name, "From Preset");
        assert!(loaded.is_public, "preset's is_public must survive");
        std::fs::remove_dir_all(&dir).ok();
    }
}
