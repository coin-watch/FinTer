use crate::data::error::CoinWatchError;
use crate::data::update_checker;
use crate::data::update_downloader::{self, DownloadProgress};
use crate::data::version::{APP_VERSION, UpdateState, VersionInfo, backup_dir};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// On-disk path of the executable that was replaced by the most recent
/// successful update.
///
/// `std::env::current_exe()` cannot be used after `self_replace` on Linux: the
/// running process keeps the old inode, so `/proc/self/exe` resolves to
/// `"<path> (deleted)"` and spawning it fails. Remember the real path while the
/// replacement happens so the restart button can launch the new binary.
static INSTALLED_EXE: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Manages the full update lifecycle: check → download → verify → prompt → install.
///
/// This struct is designed to be held as a global or per-window singleton.
/// UI code polls `state()` to drive notifications and dialogs.
pub struct UpdateManager {
    state: Arc<Mutex<UpdateState>>,
    download_progress: Arc<Mutex<DownloadProgress>>,
    /// Bumped whenever the release channel changes. In-flight checks and
    /// downloads compare against it so results from the old channel cannot
    /// overwrite the new channel's state.
    generation: Arc<AtomicU64>,
}

impl Default for UpdateManager {
    fn default() -> Self {
        Self::new()
    }
}

impl UpdateManager {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(UpdateState::Idle)),
            download_progress: Arc::new(Mutex::new(DownloadProgress::default())),
            generation: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Get the current update state (for UI polling).
    pub fn state(&self) -> UpdateState {
        let state = self.state.lock().unwrap().clone();
        if let UpdateState::Downloading { version, .. } = state {
            UpdateState::Downloading { version, progress: self.download_progress.lock().unwrap().fraction }
        } else {
            state
        }
    }

    /// Get download progress (for progress bar rendering).
    pub fn download_progress(&self) -> DownloadProgress {
        self.download_progress.lock().unwrap().clone()
    }

    /// Switch the release channel used for future update checks.
    ///
    /// Returns `true` when the channel actually changed. The caller can then
    /// trigger an immediate check, since the previous metadata is no longer
    /// relevant.
    pub fn set_channel(&self, channel: crate::data::version::UpdateChannel) -> bool {
        if crate::data::version::selected_channel() == channel {
            return false;
        }

        crate::data::version::set_selected_channel(channel);
        self.generation.fetch_add(1, Ordering::SeqCst);
        update_checker::reset_check_time();
        *self.state.lock().unwrap() = UpdateState::Idle;
        *self.download_progress.lock().unwrap() = DownloadProgress::default();
        tracing::info!(
            "update_manager: switched update channel to {}",
            channel.as_str()
        );
        true
    }

    fn is_current(&self, generation: u64) -> bool {
        self.generation.load(Ordering::SeqCst) == generation
    }

    /// Invalidate any in-flight check or download and reset the update state.
    ///
    /// Used when the user explicitly re-selects the current channel, so the
    /// program still re-scans that channel instead of trusting stale state.
    pub fn prepare_channel_check(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        update_checker::reset_check_time();
        *self.state.lock().unwrap() = UpdateState::Idle;
        *self.download_progress.lock().unwrap() = DownloadProgress::default();
    }

    /// Perform a version check if enough time has elapsed.
    /// Returns `Some(VersionInfo)` if an update is available.
    ///
    /// Blocking — call from a background thread.
    pub fn check_for_update(&self) -> Option<VersionInfo> {
        if !update_checker::should_check_now() {
            tracing::debug!("update_manager: skipping check (too recent)");
            return None;
        }

        let allow_any_version = crate::data::version::selected_channel()
            != crate::data::version::default_channel();
        self.run_check(allow_any_version)
    }

    /// Force a version check regardless of timing.
    ///
    /// Blocking — call from a background thread.
    pub fn force_check(&self) -> Option<VersionInfo> {
        let allow_any_version = crate::data::version::selected_channel()
            != crate::data::version::default_channel();
        self.run_check(allow_any_version)
    }

    /// Force a check for the currently selected channel and accept any version
    /// it publishes, including an older version than the running binary.
    ///
    /// Blocking — call from a background thread.
    pub fn force_channel_check(&self) -> Option<VersionInfo> {
        self.run_check(true)
    }

    fn run_check(&self, allow_any_version: bool) -> Option<VersionInfo> {
        let generation = self.generation.load(Ordering::SeqCst);
        {
            let mut s = self.state.lock().unwrap();
            *s = UpdateState::Checking;
        }

        match update_checker::check_for_update_with(allow_any_version) {
            Ok(Some(info)) => {
                if !self.is_current(generation) {
                    return None;
                }
                update_checker::record_check_time();
                let mut s = self.state.lock().unwrap();
                *s = UpdateState::Available(Box::new(info.clone()));
                tracing::info!(
                    "update_manager: update available: {} -> {}",
                    APP_VERSION,
                    info.version
                );
                Some(info)
            }
            Ok(None) => {
                if !self.is_current(generation) {
                    return None;
                }
                update_checker::record_check_time();
                let mut s = self.state.lock().unwrap();
                *s = UpdateState::Idle;
                None
            }
            Err(e) => {
                if !self.is_current(generation) {
                    return None;
                }
                tracing::warn!("update_manager: check failed: {}", e);
                let mut s = self.state.lock().unwrap();
                *s = UpdateState::Failed(e.to_string());
                None
            }
        }
    }

    /// Download the available update in the background.
    ///
    /// Blocking — call from a background thread.
    pub fn download_update(&self, version_info: &VersionInfo) -> Result<PathBuf, CoinWatchError> {
        let generation = self.generation.load(Ordering::SeqCst);
        {
            let mut s = self.state.lock().unwrap();
            *s = UpdateState::Downloading {
                version: version_info.version.clone(),
                progress: 0.0,
            };
        }

        // Reset progress
        {
            let mut p = self.download_progress.lock().unwrap();
            *p = DownloadProgress::default();
        }

        let progress = self.download_progress.clone();
        let state = self.state.clone();
        let version = version_info.version.clone();

        match update_downloader::download_update(version_info, progress) {
            Ok(path) => {
                if !self.is_current(generation) {
                    tracing::info!(
                        "update_manager: discarding download from a previous update channel"
                    );
                    return Err(CoinWatchError::UpdateDownload(
                        "Update channel changed during download".into(),
                    ));
                }
                let mut s = state.lock().unwrap();
                *s = UpdateState::ReadyToInstall {
                    version: version.clone(),
                    path: path.to_string_lossy().into_owned(),
                };
                tracing::info!(
                    "update_manager: download complete, ready to install v{}",
                    version
                );

                // Clean up old updates (keep last 2)
                update_downloader::cleanup_old_updates(2);

                Ok(path)
            }
            Err(e) => {
                if !self.is_current(generation) {
                    return Err(e);
                }
                let mut s = state.lock().unwrap();
                *s = UpdateState::Failed(e.to_string());
                Err(e)
            }
        }
    }

    /// Create a backup of the current binary before installing an update.
    pub fn backup_current_binary() -> Result<PathBuf, CoinWatchError> {
        let current_exe = std::env::current_exe()
            .map_err(|e| CoinWatchError::Io(format!("Cannot find current executable: {e}")))?;

        let backup = backup_dir().join(format!("v{}", APP_VERSION));
        std::fs::create_dir_all(&backup)
            .map_err(|e| CoinWatchError::Io(format!("Cannot create backup dir: {e}")))?;

        let backup_path = backup.join(
            current_exe
                .file_name()
                .unwrap_or_else(|| std::ffi::OsStr::new("finter")),
        );

        std::fs::copy(&current_exe, &backup_path)
            .map_err(|e| CoinWatchError::Io(format!("Backup failed: {e}")))?;

        tracing::info!(
            "update_manager: backed up current binary to {}",
            backup_path.display()
        );

        Ok(backup_path)
    }

    /// Replace running executable with the updated binary.
    pub fn install_update(update_path: &Path) -> Result<(), CoinWatchError> {
        let current_exe = std::env::current_exe()
            .map_err(|e| CoinWatchError::Io(format!("Cannot find current executable: {e}")))?;
        let expected = update_downloader::checksum_path(update_path);
        let checksum = std::fs::read_to_string(&expected)
            .map_err(|e| CoinWatchError::UpdateDownload(format!("Missing verified checksum: {e}")))?;
        update_downloader::verify_checksum(update_path, checksum.trim())?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(metadata) = std::fs::metadata(update_path) {
                let mut perms = metadata.permissions();
                perms.set_mode(0o755);
                let _ = std::fs::set_permissions(update_path, perms);
            }
        }

        let backup = Self::backup_current_binary()?;
        if let Err(error) = self_replace::self_replace(update_path) {
            let _ = std::fs::copy(&backup, &current_exe);
            return Err(CoinWatchError::Io(format!("Executable replacement failed; backup restored: {error}")));
        }

        // Record the path *before* the process image becomes stale. This is what
        // the restart action must launch.
        if let Ok(mut installed) = INSTALLED_EXE.lock() {
            *installed = Some(current_exe);
        }

        Ok(())
    }

    /// Path of the executable installed by the most recent update, if any.
    pub fn installed_exe_path() -> Option<PathBuf> {
        INSTALLED_EXE.lock().ok().and_then(|path| path.clone())
    }

    /// Restart the application with the current command line arguments.
    pub fn restart_app() -> Result<(), CoinWatchError> {
        let exe = Self::installed_exe_path()
            .or_else(|| std::env::current_exe().ok())
            .ok_or_else(|| CoinWatchError::Io("Cannot find executable to restart".into()))?;

        // Defensive: if we ever fall back to `current_exe()` after a Linux
        // self-replace, strip the kernel's " (deleted)" marker.
        let exe = strip_deleted_suffix(&exe);
        if !exe.exists() {
            return Err(CoinWatchError::Io(format!(
                "Cannot restart: executable not found at {}",
                exe.display()
            )));
        }

        let mut cmd = std::process::Command::new(&exe);
        cmd.args(std::env::args().skip(1));
        cmd.stdin(std::process::Stdio::null());
        cmd.stdout(std::process::Stdio::null());
        cmd.stderr(std::process::Stdio::null());
        cmd.spawn().map_err(|e| {
            CoinWatchError::Io(format!(
                "Failed to restart application from {}: {e}",
                exe.display()
            ))
        })?;
        Ok(())
    }

    /// Mark the update as installed, transitioning to RestartRequired state.
    pub fn mark_installed(&self, version: &str) {
        let mut s = self.state.lock().unwrap();
        *s = UpdateState::RestartRequired {
            version: version.to_string(),
        };
    }

    /// Check if a crash happened during a previous update (lock file exists).
    /// Returns the version to rollback to, if applicable.
    pub fn check_crash_rollback() -> Option<String> {
        let crash_marker = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("finter")
            .join("update_in_progress");

        if crash_marker.exists() {
            tracing::warn!(
                "update_manager: crash marker detected, previous update may have failed"
            );
            // Read the version that was being updated to
            let version = std::fs::read_to_string(&crash_marker).ok();
            // Clean up the marker
            std::fs::remove_file(&crash_marker).ok();
            return version;
        }
        None
    }

    /// Write a marker file indicating an update is in progress.
    /// If the app crashes during update, this marker will persist.
    pub fn mark_update_in_progress(version: &str) {
        let marker = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("finter")
            .join("update_in_progress");

        if let Some(parent) = marker.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        std::fs::write(&marker, version).ok();
    }

    /// Remove the update-in-progress marker (update succeeded).
    pub fn clear_update_marker() {
        let marker = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("finter")
            .join("update_in_progress");
        std::fs::remove_file(&marker).ok();
    }

    /// List available rollback versions from the backup directory.
    pub fn available_rollback_versions() -> Vec<String> {
        let dir = backup_dir();
        let mut versions: Vec<String> = std::fs::read_dir(&dir)
            .ok()
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_ok_and(|ft| ft.is_dir()))
            .filter_map(|e| {
                e.file_name()
                    .to_str()
                    .and_then(|n| n.strip_prefix('v'))
                    .map(|s| s.to_string())
            })
            .collect();
        versions.sort();
        versions.reverse();
        versions
    }

    pub fn fail(&self, message: String) {
        *self.state.lock().unwrap() = UpdateState::Failed(message);
    }

    /// Reset state to idle (dismiss notification).
    pub fn dismiss(&self) {
        let mut s = self.state.lock().unwrap();
        *s = UpdateState::Idle;
    }
}

/// Linux appends " (deleted)" to `/proc/self/exe` once the running executable
/// has been replaced or unlinked. Strip it so the path can be spawned again.
fn strip_deleted_suffix(path: &Path) -> PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;

        const SUFFIX: &[u8] = b" (deleted)";
        let bytes = path.as_os_str().as_bytes();
        if let Some(stripped) = bytes.strip_suffix(SUFFIX) {
            return PathBuf::from(std::ffi::OsStr::from_bytes(stripped));
        }
    }

    path.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::version::UpdateState;

    #[test]
    fn test_update_manager_default_state() {
        let mgr = UpdateManager::new();
        assert_eq!(mgr.state(), UpdateState::Idle);
    }

    #[test]
    fn test_update_manager_dismiss() {
        let mgr = UpdateManager::new();
        {
            let mut s = mgr.state.lock().unwrap();
            *s = UpdateState::Failed("test error".into());
        }
        assert!(matches!(mgr.state(), UpdateState::Failed(_)));
        mgr.dismiss();
        assert_eq!(mgr.state(), UpdateState::Idle);
    }

    #[test]
    fn test_crash_marker_lifecycle() {
        // Clean up any existing marker
        UpdateManager::clear_update_marker();
        assert!(UpdateManager::check_crash_rollback().is_none());

        // Set marker
        UpdateManager::mark_update_in_progress("0.1.1");
        let version = UpdateManager::check_crash_rollback();
        assert_eq!(version, Some("0.1.1".to_string()));

        // Marker should be cleaned up after check
        assert!(UpdateManager::check_crash_rollback().is_none());
    }

    #[test]
    fn test_available_rollback_versions_empty() {
        // Should not panic even if backup dir is empty
        let versions = UpdateManager::available_rollback_versions();
        // May or may not be empty depending on test environment
        let _ = versions;
    }

    #[test]
    fn test_download_progress_default() {
        let mgr = UpdateManager::new();
        let p = mgr.download_progress();
        assert_eq!(p.downloaded, 0);
        assert!(!p.complete);
    }

    #[test]
    fn test_strip_deleted_suffix() {
        assert_eq!(
            strip_deleted_suffix(Path::new("/tmp/finter (deleted)")),
            PathBuf::from("/tmp/finter")
        );
        assert_eq!(
            strip_deleted_suffix(Path::new("/tmp/finter")),
            PathBuf::from("/tmp/finter")
        );
    }
}
