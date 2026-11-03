//! Local-only tab/session preferences.
//!
//! Stored in `~/.config/finter/guest_tab_settings.json` and
//! `~/.config/finter/cloud_tab_settings.json` and never synced to the backend.
//! This keeps "which dashboard tabs are open" a per-machine choice, the same
//! way a text editor remembers its open files.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// How dashboard tabs should be chosen when FinTer starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum StartupTabMode {
    /// Reopen the tabs that were open when the app last ran.
    #[default]
    Restore,
    /// Open every saved dashboard.
    All,
    /// Start with no dashboard tabs open.
    Empty,
    /// Open only the dashboards explicitly selected in Settings → Tabs.
    Selected,
}

impl StartupTabMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Restore => "restore",
            Self::All => "all",
            Self::Empty => "empty",
            Self::Selected => "selected",
        }
    }

    pub fn from_value(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "all" => Self::All,
            "empty" => Self::Empty,
            "selected" => Self::Selected,
            _ => Self::Restore,
        }
    }
}

/// Local tab preferences.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TabSettings {
    #[serde(default)]
    pub startup_mode: StartupTabMode,
    /// Tab ids that were open at the end of the previous session.
    #[serde(default)]
    pub open_tab_ids: Vec<String>,
    /// Tab ids the user pinned for [`StartupTabMode::Selected`].
    #[serde(default)]
    pub selected_tab_ids: Vec<String>,
}

impl Default for TabSettings {
    fn default() -> Self {
        Self {
            startup_mode: StartupTabMode::Restore,
            open_tab_ids: Vec::new(),
            selected_tab_ids: Vec::new(),
        }
    }
}

impl TabSettings {
    /// Resolve which tab ids should be open for the current startup mode.
    ///
    /// Ids that no longer exist are ignored. If a non-empty mode produces no
    /// open tabs (for example, a first launch with no saved session), every
    /// available dashboard is returned so the workspace is never blank.
    /// [`StartupTabMode::Empty`] deliberately returns no tabs.
    pub fn resolve_open_tabs(&self, available_ids: &[String]) -> Vec<String> {
        let filtered = |ids: &[String]| {
            ids.iter()
                .filter(|id| available_ids.iter().any(|available| available == *id))
                .cloned()
                .collect::<Vec<_>>()
        };

        let resolved = match self.startup_mode {
            StartupTabMode::All => available_ids.to_vec(),
            StartupTabMode::Empty => Vec::new(),
            StartupTabMode::Restore => filtered(&self.open_tab_ids),
            StartupTabMode::Selected => filtered(&self.selected_tab_ids),
        };

        if resolved.is_empty() && self.startup_mode != StartupTabMode::Empty {
            available_ids.to_vec()
        } else {
            resolved
        }
    }
}

/// Which local profile owns a tab-settings file.
///
/// Guest and cloud sessions keep separate files, matching the rest of FinTer's
/// local storage split.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabProfile {
    Guest,
    Cloud,
}

impl TabProfile {
    fn file_name(self) -> &'static str {
        match self {
            Self::Guest => "guest_tab_settings.json",
            Self::Cloud => "cloud_tab_settings.json",
        }
    }
}

fn tab_settings_path(profile: TabProfile) -> Option<PathBuf> {
    dirs::config_dir().map(|path| path.join("finter").join(profile.file_name()))
}

/// Load local tab preferences, falling back to defaults when the file is
/// missing or unreadable.
pub fn load(profile: TabProfile) -> TabSettings {
    let Some(path) = tab_settings_path(profile) else {
        return TabSettings::default();
    };
    let Ok(bytes) = std::fs::read(&path) else {
        return TabSettings::default();
    };
    serde_json::from_slice(&bytes).unwrap_or_default()
}

/// Persist local tab preferences. The file is written atomically so a crash
/// cannot leave a partially written settings file behind.
pub fn save(profile: TabProfile, settings: &TabSettings) -> Result<(), String> {
    use std::io::Write;

    let path = tab_settings_path(profile)
        .ok_or_else(|| "cannot determine config directory".to_string())?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("mkdir: {error}"))?;
    }

    let temporary = path.with_extension("tmp");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }

    let mut file = options
        .open(&temporary)
        .map_err(|error| format!("open tab settings: {error}"))?;
    file.write_all(
        serde_json::to_string_pretty(settings)
            .map_err(|error| format!("serialize tab settings: {error}"))?
            .as_bytes(),
    )
    .map_err(|error| format!("write tab settings: {error}"))?;
    file.sync_all()
        .map_err(|error| format!("sync tab settings: {error}"))?;
    drop(file);

    std::fs::rename(&temporary, &path).map_err(|error| format!("rename tab settings: {error}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn default_mode_restores_last_session() {
        let settings = TabSettings::default();
        assert_eq!(settings.startup_mode, StartupTabMode::Restore);
    }

    #[test]
    fn all_mode_opens_every_dashboard() {
        let settings = TabSettings {
            startup_mode: StartupTabMode::All,
            open_tab_ids: ids(&["a"]),
            selected_tab_ids: ids(&["b"]),
        };
        assert_eq!(
            settings.resolve_open_tabs(&ids(&["a", "b", "c"])),
            ids(&["a", "b", "c"])
        );
    }

    #[test]
    fn restore_mode_keeps_saved_order_and_ignores_missing_ids() {
        let settings = TabSettings {
            startup_mode: StartupTabMode::Restore,
            open_tab_ids: ids(&["c", "missing", "a"]),
            selected_tab_ids: Vec::new(),
        };
        assert_eq!(
            settings.resolve_open_tabs(&ids(&["a", "b", "c"])),
            ids(&["c", "a"])
        );
    }

    #[test]
    fn selected_mode_uses_selected_ids() {
        let settings = TabSettings {
            startup_mode: StartupTabMode::Selected,
            open_tab_ids: ids(&["a", "b"]),
            selected_tab_ids: ids(&["b"]),
        };
        assert_eq!(
            settings.resolve_open_tabs(&ids(&["a", "b", "c"])),
            ids(&["b"])
        );
    }

    #[test]
    fn empty_selection_falls_back_to_all_dashboards() {
        let settings = TabSettings {
            startup_mode: StartupTabMode::Selected,
            open_tab_ids: Vec::new(),
            selected_tab_ids: Vec::new(),
        };
        assert_eq!(
            settings.resolve_open_tabs(&ids(&["a", "b"])),
            ids(&["a", "b"])
        );
    }

    #[test]
    fn empty_mode_starts_with_no_tabs() {
        let settings = TabSettings {
            startup_mode: StartupTabMode::Empty,
            open_tab_ids: ids(&["a"]),
            selected_tab_ids: ids(&["b"]),
        };
        assert!(
            settings
                .resolve_open_tabs(&ids(&["a", "b", "c"]))
                .is_empty()
        );
    }

    #[test]
    fn mode_round_trips_through_strings() {
        for mode in [
            StartupTabMode::Restore,
            StartupTabMode::All,
            StartupTabMode::Empty,
            StartupTabMode::Selected,
        ] {
            assert_eq!(StartupTabMode::from_value(mode.as_str()), mode);
        }
        assert_eq!(
            StartupTabMode::from_value("something-else"),
            StartupTabMode::Restore
        );
    }
}
