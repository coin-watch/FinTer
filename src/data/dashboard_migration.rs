//! One-off migration from the legacy Supabase `modules` / `layout_presets`
//! tables into on-disk dashboard files.
//!
//! The tables are read through their original HTTP endpoints, which are still
//! deployed. This module deliberately does **not** go through
//! `data::modules` / `data::layout_presets`, because those now read the local
//! file store — using them here would make the migration read its own output.
//!
//! Run once per machine, then the endpoints and tables can be retired.

use crate::data::client::ApiClient;
use crate::data::dashboard_files::{self, LegacyRow, MigrationReport};
use crate::data::error::{CoinWatchError, Result};
use crate::data::models::{LayoutPreset, Module};

/// Raw `GET /api/modules` against the legacy table.
fn fetch_legacy_modules(client: &ApiClient) -> Result<Vec<Module>> {
    let user_id = client
        .user_id()
        .ok_or_else(|| CoinWatchError::Auth("Not logged in".into()))?;

    let rt = crate::data::client::tokio_runtime();
    let req = client.auth_get(&format!("/api/modules?user_id={user_id}"));

    rt.block_on(async {
        let resp = req.send().await?;
        let status = resp.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            tracing::info!("legacy /api/modules not deployed; nothing to migrate");
            return Ok(vec![]);
        }
        let body = resp.text().await?;
        if !status.is_success() {
            return Err(CoinWatchError::Api {
                status: status.as_u16(),
                message: body,
            });
        }
        let trimmed = body.trim();
        if trimmed.is_empty() {
            return Ok(vec![]);
        }
        serde_json::from_str(trimmed)
            .map_err(|e| CoinWatchError::Deserialization(format!("legacy modules: {e}")))
    })
}

/// Raw `GET /api/layout-presets` against the legacy table.
fn fetch_legacy_presets(client: &ApiClient) -> Result<Vec<LayoutPreset>> {
    let user_id = client
        .user_id()
        .ok_or_else(|| CoinWatchError::Auth("Not logged in".into()))?;

    let rt = crate::data::client::tokio_runtime();
    let req = client.auth_get(&format!("/api/layout-presets?user_id={user_id}"));

    rt.block_on(async {
        let resp = req.send().await?;
        let status = resp.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            tracing::info!("legacy /api/layout-presets not deployed; nothing to migrate");
            return Ok(vec![]);
        }
        let body = resp.text().await?;
        if !status.is_success() {
            return Err(CoinWatchError::Api {
                status: status.as_u16(),
                message: body,
            });
        }
        let trimmed = body.trim();
        if trimmed.is_empty() {
            return Ok(vec![]);
        }
        serde_json::from_str(trimmed)
            .map_err(|e| CoinWatchError::Deserialization(format!("legacy presets: {e}")))
    })
}

/// Read both legacy tables and convert them into migration rows.
pub fn collect_legacy_rows(client: &ApiClient) -> Result<Vec<LegacyRow>> {
    let mut rows = Vec::new();

    for preset in fetch_legacy_presets(client)? {
        let Some(id) = preset.id else {
            tracing::warn!("skipping layout preset without id: {}", preset.name);
            continue;
        };
        rows.push(LegacyRow {
            id,
            name: preset.name,
            tree: preset.tree,
            is_public: preset.is_public.unwrap_or(false),
            created_at: preset.created_at,
            updated_at: preset.updated_at,
            from_preset: true,
        });
    }

    for module in fetch_legacy_modules(client)? {
        let Some(id) = module.id else {
            tracing::warn!("skipping module without id: {}", module.module_name);
            continue;
        };
        rows.push(LegacyRow {
            id,
            name: module.module_name,
            tree: module.module,
            is_public: false,
            created_at: module.created_at,
            updated_at: module.updated_at,
            from_preset: false,
        });
    }

    Ok(rows)
}

/// Fetch the legacy tables and write them out as dashboard files.
///
/// `overwrite` controls whether rows whose id already exists on disk are
/// rewritten; leaving it false makes the migration safe to re-run.
pub fn migrate_from_api(client: &ApiClient, overwrite: bool) -> Result<MigrationReport> {
    let rows = collect_legacy_rows(client)?;
    tracing::info!("migrating {} legacy dashboard rows", rows.len());
    dashboard_files::migrate_rows(rows, overwrite)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_row_conversion_keeps_identity() {
        let preset = LayoutPreset {
            id: Some("p1".into()),
            user_id: Some("u1".into()),
            name: "Shared".into(),
            tree: serde_json::json!({"x": 1}),
            is_public: Some(true),
            created_at: Some("2026-01-01T00:00:00Z".into()),
            updated_at: None,
        };

        let row = LegacyRow {
            id: preset.id.clone().unwrap(),
            name: preset.name.clone(),
            tree: preset.tree.clone(),
            is_public: preset.is_public.unwrap_or(false),
            created_at: preset.created_at.clone(),
            updated_at: preset.updated_at.clone(),
            from_preset: true,
        };

        assert_eq!(row.id, "p1");
        assert!(row.is_public);
        assert!(row.from_preset);
    }
}
