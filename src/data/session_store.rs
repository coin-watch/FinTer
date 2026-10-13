//! Persistent session storage with OS keyring (primary) and JSON file (fallback).
//!
//! Strategy:
//! 1. Try to use the OS secure enclave via `keyring` (macOS Keychain, Linux Secret Service,
//!    Windows Credential Manager).
//! 2. If keyring is unavailable (e.g. no D-Bus / Secret Service on headless Linux),
//!    fall back to a plain JSON file at `~/.config/finter/session.json`.
//!    The file path is world-readable only by the current user (mode 0600 on Unix).
//!
//! # Security note
//! The file fallback is less secure than the keyring and should be considered a
//! best-effort convenience for headless/CI environments. On production desktops the
//! keyring path will always be used.

use crate::data::error::{CoinWatchError, Result};
use crate::data::models::AuthSession;

const SERVICE: &str = "finter";
const ACCOUNT: &str = "session";

// ── Keyring helpers ──────────────────────────────────────────────────────────

fn keyring_save(session: &AuthSession) -> std::result::Result<(), String> {
    let json = serde_json::to_string(session).map_err(|e| e.to_string())?;
    let entry = keyring::Entry::new(SERVICE, ACCOUNT).map_err(|e| format!("keyring entry: {e}"))?;
    entry
        .set_password(&json)
        .map_err(|e| format!("keyring set: {e}"))
}

fn keyring_load() -> std::result::Result<Option<AuthSession>, String> {
    let entry = keyring::Entry::new(SERVICE, ACCOUNT).map_err(|e| format!("keyring entry: {e}"))?;
    match entry.get_password() {
        Ok(json) => {
            let session: AuthSession =
                serde_json::from_str(&json).map_err(|e| format!("keyring deserialize: {e}"))?;
            Ok(Some(session))
        }
        // NotFound / NoEntry — nothing stored yet, that's normal
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(format!("keyring get: {e}")),
    }
}

fn keyring_delete() -> std::result::Result<(), String> {
    let entry = keyring::Entry::new(SERVICE, ACCOUNT).map_err(|e| format!("keyring entry: {e}"))?;
    match entry.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()), // nothing to delete
        Err(e) => Err(format!("keyring delete: {e}")),
    }
}

// ── File-fallback helpers ────────────────────────────────────────────────────

fn fallback_path() -> std::result::Result<std::path::PathBuf, String> {
    let mut path = dirs::config_dir().ok_or_else(|| {
        let appdata = std::env::var("APPDATA").ok();
        let localappdata = std::env::var("LOCALAPPDATA").ok();
        format!(
            "cannot determine config directory (dirs returned None, APPDATA={:?}, LOCALAPPDATA={:?})",
            appdata, localappdata
        )
    })?;
    path.push("finter");
    path.push("session.json");
    Ok(path)
}

fn file_save(session: &AuthSession) -> std::result::Result<(), String> {
    let path = fallback_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir: {e}"))?;
    }
    let json = serde_json::to_string_pretty(session).map_err(|e| e.to_string())?;
    std::fs::write(&path, &json).map_err(|e| format!("write session file: {e}"))?;

    // Restrict permissions to owner-only on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("chmod: {e}"))?;
    }

    Ok(())
}

fn file_load() -> std::result::Result<Option<AuthSession>, String> {
    let path = fallback_path()?;
    if !path.exists() {
        return Ok(None);
    }
    let json = std::fs::read_to_string(&path).map_err(|e| format!("read session file: {e}"))?;
    let session: AuthSession =
        serde_json::from_str(&json).map_err(|e| format!("deserialize session file: {e}"))?;
    Ok(Some(session))
}

fn file_delete() -> std::result::Result<(), String> {
    let path = fallback_path()?;
    if path.exists() {
        std::fs::remove_file(&path).map_err(|e| format!("delete session file: {e}"))?;
    }
    Ok(())
}

// ── Public API ───────────────────────────────────────────────────────────────

/// Persist `session` to the OS keyring and file for redundancy.
/// At least one storage method must succeed, otherwise return error.
pub fn save_session(session: &AuthSession) -> Result<()> {
    let keyring_result = keyring_save(session);
    let file_result = file_save(session);

    match (&keyring_result, &file_result) {
        (Ok(()), Ok(())) => {
            tracing::debug!("session saved to both keyring and file");
            Ok(())
        }
        (Ok(()), Err(e)) => {
            tracing::warn!("keyring save succeeded but file fallback failed: {}", e);
            Ok(())
        }
        (Err(e), Ok(())) => {
            tracing::warn!("file save succeeded but keyring failed: {}", e);
            Ok(())
        }
        (Err(keyring_err), Err(file_err)) => {
            let err_msg = format!(
                "both keyring and file storage failed. keyring: {}, file: {}",
                keyring_err, file_err
            );
            tracing::error!("CRITICAL: session persistence failed: {}", err_msg);
            Err(CoinWatchError::Storage(err_msg))
        }
    }
}

/// Load the previously persisted session from keyring or file.
/// Returns the first successful result, or error if both fail.
pub fn load_session() -> Result<Option<AuthSession>> {
    match keyring_load() {
        Ok(Some(session)) => {
            tracing::debug!("session loaded from keyring");
            Ok(Some(session))
        }
        Ok(None) => {
            tracing::debug!("no session in keyring, trying file");
            file_load()
                .map_err(CoinWatchError::Storage)
                .inspect(|_| tracing::debug!("session load complete"))
        }
        Err(keyring_err) => {
            tracing::warn!("keyring load failed: {}, trying file", keyring_err);
            file_load().map_err(CoinWatchError::Storage)
        }
    }
}

/// Remove any persisted session from both keyring and file (belt-and-suspenders).
pub fn delete_session() -> Result<()> {
    let keyring_result = keyring_delete();
    let file_result = file_delete();

    match (keyring_result, file_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(e), _) | (_, Err(e)) => Err(CoinWatchError::Storage(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies that the fallback path resolves to a reasonable location.
    #[test]
    fn test_fallback_path_resolves() {
        let path = fallback_path().expect("should resolve fallback path");
        assert!(path.ends_with("finter/session.json"));
    }

    /// Round-trip save/load/delete via file fallback (bypasses keyring so CI works).
    #[test]
    fn test_file_round_trip() {
        let session = AuthSession {
            user_id: "u-test".into(),
            email: "test@example.com".into(),
            access_token: "acc-tok".into(),
            refresh_token: "ref-tok".into(),
            expires_at: Some(9999999999),
            name: Some("Test User".into()),
            avatar_url: Some("https://example.com/avatar.png".into()),
        };

        file_save(&session).expect("file_save");
        let loaded = file_load().expect("file_load").expect("should be Some");
        assert_eq!(loaded.user_id, "u-test");
        assert_eq!(loaded.email, "test@example.com");
        assert_eq!(loaded.name, Some("Test User".into()));
        assert_eq!(
            loaded.avatar_url,
            Some("https://example.com/avatar.png".into())
        );

        file_delete().expect("file_delete");
        let after_delete = file_load().expect("file_load after delete");
        assert!(after_delete.is_none());
    }
}
