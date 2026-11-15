//! Integration check for the dashboard file store.
//!
//! Run with: `cargo test --test dashboard_files_roundtrip`
//!
//! These exercise the public API against a scratch directory so they do not
//! touch the developer's real `~/.config/finter/dashboards`.

use std::path::PathBuf;

// The store's directory-scoped functions are the contract the app relies on;
// the public wrappers only add `dirs::config_dir()`.
use finter::data::dashboard_files::{
    delete_in, list_in, load_in, migrate_rows_in, upsert_in, DashboardFile, LegacyRow,
};

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("finter-it-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn dash(id: &str, name: &str) -> DashboardFile {
    let now = chrono::Utc::now();
    DashboardFile {
        id: id.into(),
        name: name.into(),
        tree: serde_json::json!({"kind": "pane", "id": "p1"}),
        is_public: false,
        created_at: now,
        updated_at: now,
    }
}

#[test]
fn create_list_delete_roundtrip() {
    let dir = scratch("crud");

    upsert_in(&dir, dash("a", "Alpha")).unwrap();
    upsert_in(&dir, dash("b", "Beta")).unwrap();

    let all = list_in(&dir).unwrap();
    assert_eq!(all.len(), 2);

    let a = load_in(&dir, "a").unwrap().unwrap();
    assert_eq!(a.name, "Alpha");

    delete_in(&dir, "a").unwrap();
    assert!(load_in(&dir, "a").unwrap().is_none());
    assert_eq!(list_in(&dir).unwrap().len(), 1);

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn files_survive_a_fresh_read() {
    let dir = scratch("persist");
    upsert_in(&dir, dash("keep", "Persisted")).unwrap();

    // Simulate a restart: re-read from disk with no in-memory state.
    let reloaded = list_in(&dir).unwrap();
    assert_eq!(reloaded.len(), 1);
    assert_eq!(reloaded[0].id, "keep");
    assert_eq!(reloaded[0].name, "Persisted");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn migration_then_list_matches_source_rows() {
    let dir = scratch("migrate-it");
    let rows = vec![
        LegacyRow {
            id: "p1".into(),
            name: "Preset One".into(),
            tree: serde_json::json!({"kind": "pane", "id": "x"}),
            is_public: true,
            created_at: Some("2026-02-01T00:00:00Z".into()),
            updated_at: Some("2026-02-02T00:00:00Z".into()),
            from_preset: true,
        },
        LegacyRow {
            id: "m1".into(),
            name: "Module One".into(),
            tree: serde_json::json!({"kind": "pane", "id": "y"}),
            is_public: false,
            created_at: None,
            updated_at: None,
            from_preset: false,
        },
    ];

    let report = migrate_rows_in(&dir, rows, false).unwrap();
    assert_eq!(report.written, 2);
    assert_eq!(report.failed, 0);

    let all = list_in(&dir).unwrap();
    assert_eq!(all.len(), 2, "every source row becomes one file");

    let ids: Vec<_> = all.iter().map(|d| d.id.as_str()).collect();
    assert!(ids.contains(&"p1"));
    assert!(ids.contains(&"m1"));

    let p = load_in(&dir, "p1").unwrap().unwrap();
    assert!(p.is_public, "public flag carried over from layout_presets");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn invalid_ids_are_rejected_before_touching_disk() {
    let dir = scratch("traversal");
    assert!(load_in(&dir, "../escape").is_err());
    assert!(delete_in(&dir, "../escape").is_err());
    assert!(upsert_in(&dir, dash("../escape", "Bad")).is_err());
    std::fs::remove_dir_all(&dir).ok();
}
