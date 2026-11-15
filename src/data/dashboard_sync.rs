//! Push/pull sync between the local dashboard files and the cloud.
//!
//! The desktop app treats its local files as the source of truth: it works
//! offline, and a dashboard edit never waits on the network. This module is
//! the optional bridge to the server, which stores the same envelope in R2.
//!
//! Conflict rule: last write wins on `updated_at`. Both sides carry the field
//! and both bump it on edit, so the newer timestamp is the newer intent. That
//! is deliberately simple — a dashboard is a single user's layout, and the
//! alternative (merging two pane trees) produces layouts neither side asked
//! for.
//!
//! Nothing here runs automatically. `pull_all` and `push_all` are called
//! explicitly, so a network failure can never block the UI or lose a local
//! edit.

use crate::data::client::ApiClient;
use crate::data::dashboard_files::{self, DashboardFile};
use crate::data::error::{CoinWatchError, Result};
use serde::Deserialize;

/// What a sync run did, for logging and for the settings UI.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct SyncReport {
    /// Dashboards written to the local store.
    pub pulled: usize,
    /// Dashboards written to the server.
    pub pushed: usize,
    /// Local dashboards that were newer than the server's copy.
    pub local_wins: usize,
    /// Server dashboards that were newer than the local copy.
    pub remote_wins: usize,
    /// Dashboards that existed on only one side.
    pub local_only: usize,
    pub remote_only: usize,
    /// Per-dashboard failures. A single failure does not abort the run.
    pub errors: Vec<String>,
}

impl SyncReport {
    pub fn is_clean(&self) -> bool {
        self.errors.is_empty()
    }
}

/// The server returns rows in the old table shape for compatibility; this is
/// that shape.
#[derive(Debug, Deserialize)]
struct RemoteRow {
    id: String,
    #[serde(default)]
    module_name: String,
    #[serde(default)]
    module: serde_json::Value,
    #[serde(default)]
    is_public: bool,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    updated_at: Option<String>,
}

impl RemoteRow {
    fn into_file(self) -> Option<DashboardFile> {
        // A row without a tree cannot become a dashboard; skip rather than
        // writing an empty one over a good local file.
        if self.module.is_null() {
            return None;
        }
        let created = self
            .created_at
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
            .map(|d| d.with_timezone(&chrono::Utc))
            .unwrap_or_else(chrono::Utc::now);
        let updated = self
            .updated_at
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
            .map(|d| d.with_timezone(&chrono::Utc))
            .unwrap_or(created);

        Some(DashboardFile {
            id: self.id,
            name: if self.module_name.is_empty() {
                "Untitled".to_string()
            } else {
                self.module_name
            },
            tree: self.module,
            is_public: self.is_public,
            created_at: created,
            updated_at: updated,
        })
    }
}

/// Fetch every dashboard for the signed-in user from the server.
fn fetch_remote(client: &ApiClient) -> Result<Vec<DashboardFile>> {
    let rt = crate::data::client::tokio_runtime();

    rt.block_on(async {
        let response = client.auth_get("/api/modules").send().await?;

        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(CoinWatchError::Auth("Not signed in".into()));
        }
        if !response.status().is_success() {
            return Err(CoinWatchError::Api {
                status: response.status().as_u16(),
                message: "GET /api/modules failed".into(),
            });
        }

        let rows: Vec<RemoteRow> = response
            .json()
            .await
            .map_err(|e| CoinWatchError::Deserialization(format!("Invalid dashboard list: {e}")))?;

        Ok(rows.into_iter().filter_map(RemoteRow::into_file).collect())
    })
}

/// Push one local dashboard to the server.
fn push_one(client: &ApiClient, dashboard: &DashboardFile) -> Result<()> {
    let body = serde_json::json!({
        "id": dashboard.id,
        "module_name": dashboard.name,
        "module": dashboard.tree,
        "is_public": dashboard.is_public,
    });
    let rt = crate::data::client::tokio_runtime();

    rt.block_on(async {
        let response = client
            .auth_patch(&format!("/api/modules/{}", dashboard.id))
            .json(&body)
            .send()
            .await?;

        // A dashboard that exists locally but not on the server is a create, not
        // an update — PATCH answers 404 in that case.
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            let created = client.auth_post("/api/modules").json(&body).send().await?;
            if !created.status().is_success() {
                return Err(CoinWatchError::Api {
                    status: created.status().as_u16(),
                    message: "POST /api/modules failed".into(),
                });
            }
            return Ok(());
        }

        if !response.status().is_success() {
            return Err(CoinWatchError::Api {
                status: response.status().as_u16(),
                message: format!("PATCH /api/modules/{} failed", dashboard.id),
            });
        }
        Ok(())
    })
}

/// What to do with one dashboard during a pull.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PullAction {
    /// Not present locally — write it.
    Create,
    /// Remote is newer — overwrite the local copy.
    Overwrite,
    /// Local is same age or newer — leave it alone.
    Keep,
}

/// Decide what a pull should do, given both sides' timestamps.
///
/// Pure so the conflict rule can be tested without a network or a filesystem.
pub fn pull_action(local: Option<&DashboardFile>, remote: &DashboardFile) -> PullAction {
    match local {
        None => PullAction::Create,
        Some(local) if remote.updated_at > local.updated_at => PullAction::Overwrite,
        Some(_) => PullAction::Keep,
    }
}

/// What to do with one dashboard during a push.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushAction {
    /// Not present remotely — create it there.
    Create,
    /// Local is newer — overwrite the remote copy.
    Overwrite,
    /// Remote is same age or newer — leave it alone.
    Skip,
}

/// Decide what a push should do, given both sides' timestamps.
pub fn push_action(local: &DashboardFile, remote: Option<&DashboardFile>) -> PushAction {
    match remote {
        None => PushAction::Create,
        Some(remote) if local.updated_at > remote.updated_at => PushAction::Overwrite,
        Some(_) => PushAction::Skip,
    }
}

/// Pull remote dashboards into the local store, keeping whichever side is newer.
///
/// Returns the report rather than the merged set: the caller reloads the
/// dashboard list from the store afterwards, so there is one source of truth
/// for what is on disk.
pub fn pull_all(client: &ApiClient) -> Result<SyncReport> {
    let mut report = SyncReport::default();
    let remote = fetch_remote(client)?;
    let local = dashboard_files::list().unwrap_or_default();

    let local_by_id: std::collections::HashMap<&str, &DashboardFile> =
        local.iter().map(|d| (d.id.as_str(), d)).collect();

    for remote_dash in &remote {
        match pull_action(
            local_by_id.get(remote_dash.id.as_str()).copied(),
            remote_dash,
        ) {
            PullAction::Create => {
                report.remote_only += 1;
                match dashboard_files::upsert(remote_dash.clone()) {
                    Ok(()) => report.pulled += 1,
                    Err(e) => report.errors.push(format!("{}: {e}", remote_dash.id)),
                }
            }
            PullAction::Overwrite => {
                report.remote_wins += 1;
                match dashboard_files::upsert(remote_dash.clone()) {
                    Ok(()) => report.pulled += 1,
                    Err(e) => report.errors.push(format!("{}: {e}", remote_dash.id)),
                }
            }
            PullAction::Keep => report.local_wins += 1,
        }
    }

    let remote_ids: std::collections::HashSet<&str> =
        remote.iter().map(|d| d.id.as_str()).collect();
    report.local_only = local
        .iter()
        .filter(|d| !remote_ids.contains(d.id.as_str()))
        .count();

    Ok(report)
}

/// Push local dashboards to the server, skipping ones the server already has newer.
pub fn push_all(client: &ApiClient) -> Result<SyncReport> {
    let mut report = SyncReport::default();
    let local = dashboard_files::list().unwrap_or_default();
    let remote = fetch_remote(client)?;

    let remote_by_id: std::collections::HashMap<&str, &DashboardFile> =
        remote.iter().map(|d| (d.id.as_str(), d)).collect();

    for local_dash in &local {
        match push_action(
            local_dash,
            remote_by_id.get(local_dash.id.as_str()).copied(),
        ) {
            PushAction::Create => {
                report.local_only += 1;
                match push_one(client, local_dash) {
                    Ok(()) => report.pushed += 1,
                    Err(e) => report.errors.push(format!("{}: {e}", local_dash.id)),
                }
            }
            PushAction::Overwrite => {
                report.local_wins += 1;
                match push_one(client, local_dash) {
                    Ok(()) => report.pushed += 1,
                    Err(e) => report.errors.push(format!("{}: {e}", local_dash.id)),
                }
            }
            PushAction::Skip => report.remote_wins += 1,
        }
    }

    let local_ids: std::collections::HashSet<&str> = local.iter().map(|d| d.id.as_str()).collect();
    report.remote_only = remote
        .iter()
        .filter(|d| !local_ids.contains(d.id.as_str()))
        .count();

    Ok(report)
}

/// Pull then push, so a run converges both directions.
pub fn sync_all(client: &ApiClient) -> Result<SyncReport> {
    let pull = pull_all(client)?;
    let push = push_all(client)?;

    Ok(SyncReport {
        pulled: pull.pulled,
        pushed: push.pushed,
        local_wins: pull.local_wins.max(push.local_wins),
        remote_wins: pull.remote_wins.max(push.remote_wins),
        local_only: pull.local_only,
        remote_only: pull.remote_only,
        errors: pull.errors.into_iter().chain(push.errors).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_row_without_tree_is_skipped() {
        let row = RemoteRow {
            id: "a".into(),
            module_name: "x".into(),
            module: serde_json::Value::Null,
            is_public: false,
            created_at: None,
            updated_at: None,
        };
        assert!(row.into_file().is_none());
    }

    #[test]
    fn remote_row_maps_fields() {
        let row = RemoteRow {
            id: "a".into(),
            module_name: "My Dash".into(),
            module: serde_json::json!({"kind": "pane"}),
            is_public: true,
            created_at: Some("2026-01-01T00:00:00Z".into()),
            updated_at: Some("2026-01-02T00:00:00Z".into()),
        };
        let file = row.into_file().unwrap();
        assert_eq!(file.name, "My Dash");
        assert!(file.is_public);
        assert_eq!(file.created_at.to_rfc3339(), "2026-01-01T00:00:00+00:00");
        assert_eq!(file.updated_at.to_rfc3339(), "2026-01-02T00:00:00+00:00");
    }

    #[test]
    fn missing_name_falls_back_to_untitled() {
        let row = RemoteRow {
            id: "a".into(),
            module_name: String::new(),
            module: serde_json::json!({}),
            is_public: false,
            created_at: None,
            updated_at: None,
        };
        assert_eq!(row.into_file().unwrap().name, "Untitled");
    }

    #[test]
    fn missing_timestamps_do_not_panic() {
        let row = RemoteRow {
            id: "a".into(),
            module_name: "x".into(),
            module: serde_json::json!({}),
            is_public: false,
            created_at: None,
            updated_at: None,
        };
        let file = row.into_file().unwrap();
        assert_eq!(file.created_at, file.updated_at);
    }

    #[test]
    fn report_starts_clean() {
        let report = SyncReport::default();
        assert!(report.is_clean());
        assert_eq!(report.pulled, 0);
    }

    fn dash(id: &str, updated: &str) -> DashboardFile {
        DashboardFile {
            id: id.into(),
            name: id.into(),
            tree: serde_json::json!({}),
            is_public: false,
            created_at: chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
                .unwrap()
                .with_timezone(&chrono::Utc),
            updated_at: chrono::DateTime::parse_from_rfc3339(updated)
                .unwrap()
                .with_timezone(&chrono::Utc),
        }
    }

    #[test]
    fn pull_creates_when_absent_locally() {
        let remote = dash("a", "2026-01-02T00:00:00Z");
        assert_eq!(pull_action(None, &remote), PullAction::Create);
    }

    #[test]
    fn pull_overwrites_only_when_remote_is_newer() {
        let local = dash("a", "2026-01-01T00:00:00Z");
        let newer = dash("a", "2026-01-02T00:00:00Z");
        let older = dash("a", "2025-12-31T00:00:00Z");
        assert_eq!(pull_action(Some(&local), &newer), PullAction::Overwrite);
        assert_eq!(pull_action(Some(&local), &older), PullAction::Keep);
    }

    #[test]
    fn pull_keeps_local_on_equal_timestamps() {
        // Equal timestamps mean "no one edited" — rewriting would be churn.
        let local = dash("a", "2026-01-01T00:00:00Z");
        let remote = dash("a", "2026-01-01T00:00:00Z");
        assert_eq!(pull_action(Some(&local), &remote), PullAction::Keep);
    }

    #[test]
    fn push_creates_when_absent_remotely() {
        let local = dash("a", "2026-01-01T00:00:00Z");
        assert_eq!(push_action(&local, None), PushAction::Create);
    }

    #[test]
    fn push_overwrites_only_when_local_is_newer() {
        let local = dash("a", "2026-01-02T00:00:00Z");
        let older = dash("a", "2026-01-01T00:00:00Z");
        let newer = dash("a", "2026-01-03T00:00:00Z");
        assert_eq!(push_action(&local, Some(&older)), PushAction::Overwrite);
        assert_eq!(push_action(&local, Some(&newer)), PushAction::Skip);
    }

    #[test]
    fn push_skips_on_equal_timestamps() {
        let local = dash("a", "2026-01-01T00:00:00Z");
        let remote = dash("a", "2026-01-01T00:00:00Z");
        assert_eq!(push_action(&local, Some(&remote)), PushAction::Skip);
    }

    #[test]
    fn pull_and_push_agree_on_who_wins() {
        // The pair must be symmetric: if a pull would overwrite, the matching
        // push must skip, otherwise a sync run would ping-pong the same file.
        let local = dash("a", "2026-01-01T00:00:00Z");
        let remote = dash("a", "2026-01-02T00:00:00Z");
        assert_eq!(pull_action(Some(&local), &remote), PullAction::Overwrite);
        assert_eq!(push_action(&local, Some(&remote)), PushAction::Skip);

        let local2 = dash("a", "2026-01-03T00:00:00Z");
        assert_eq!(pull_action(Some(&local2), &remote), PullAction::Keep);
        assert_eq!(push_action(&local2, Some(&remote)), PushAction::Overwrite);
    }
}
