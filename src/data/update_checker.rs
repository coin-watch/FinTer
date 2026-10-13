use crate::data::error::{CoinWatchError, Result};
use crate::data::version::{VersionInfo, last_check_file};
use std::time::{SystemTime, UNIX_EPOCH};

/// Interval between automatic version checks (1 hour).
const CHECK_INTERVAL_SECS: u64 = 60 * 60;

/// Fetch the latest version info from the remote version.json endpoint.
///
/// Blocking — run from a background thread via `tokio_runtime().block_on(...)`.
pub fn fetch_latest_version() -> Result<VersionInfo> {
    let url = crate::data::version::version_check_url();
    if url.trim().is_empty() {
        return Err(CoinWatchError::UpdateCheck(
            "No update URL configured".into(),
        ));
    }
    fetch_version_metadata(&url)
}

async fn fetch_version_metadata_async(url: String) -> Result<VersionInfo> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| CoinWatchError::UpdateCheck(format!("HTTP client error: {e}")))?;
    let response = client
        .get(url)
        .header("User-Agent", format!("FinTer/{}", crate::data::version::APP_VERSION))
        .send().await
        .map_err(|e| CoinWatchError::UpdateCheck(format!("Network error: {e}")))?;
    if !response.status().is_success() {
        return Err(CoinWatchError::UpdateCheck(format!("HTTP {}", response.status())));
    }
    response.json().await.map_err(|e| CoinWatchError::UpdateCheck(format!("Invalid version metadata: {e}")))
}

fn fetch_version_metadata(url: &str) -> Result<VersionInfo> {
    let parsed = reqwest::Url::parse(url)
        .map_err(|e| CoinWatchError::UpdateCheck(format!("Invalid version URL: {e}")))?;
    if parsed.scheme() != "https" {
        return Err(CoinWatchError::UpdateCheck("Version URL must use HTTPS".into()));
    }
    crate::data::client::tokio_runtime().block_on(fetch_version_metadata_async(parsed.to_string()))
}

fn is_update_available(
    current: &crate::data::version::SemVer,
    current_build: Option<&str>,
    version_url: &str,
    remote: &crate::data::version::SemVer,
    remote_build: Option<&str>,
) -> bool {
    remote.is_newer_than(current) || (
        remote == current
            && ["/alpha/", "/beta/"].iter().any(|channel| version_url.contains(channel))
            && remote_build.is_some()
            && remote_build != current_build
    )
}

/// Check if the remote version is newer than the current app version.
pub fn check_for_update() -> Result<Option<VersionInfo>> {
    let check_url = crate::data::version::version_check_url();
    if check_url.trim().is_empty() {
        return Ok(None);
    }
    let info = fetch_latest_version()?;
    let current = crate::data::version::SemVer::parse(crate::data::version::APP_VERSION)
        .ok_or_else(|| CoinWatchError::UpdateCheck("Invalid current app version".into()))?;
    let remote = crate::data::version::SemVer::parse(&info.version)
        .ok_or_else(|| CoinWatchError::UpdateCheck("Invalid remote app version".into()))?;
    if !is_update_available(
        &current,
        crate::data::version::BUILD_ID,
        &check_url,
        &remote,
        info.build_id.as_deref(),
    ) { return Ok(None); }
    let url = info.download_url_for_current_platform()
        .ok_or_else(|| CoinWatchError::UpdateCheck("No download for current platform".into()))?;
    let checksum = info.checksum_for_current_platform()
        .ok_or_else(|| CoinWatchError::UpdateCheck("Missing required checksum".into()))?;
    validate_download_metadata(url, checksum)?;
    Ok(Some(info))
}

fn validate_download_metadata(url: &str, checksum: &str) -> Result<()> {
    let parsed = reqwest::Url::parse(url).map_err(|e| CoinWatchError::UpdateCheck(format!("Invalid download URL: {e}")))?;
    if parsed.scheme() != "https" { return Err(CoinWatchError::UpdateCheck("Download URL must use HTTPS".into())); }
    if checksum.len() != 64 || !checksum.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(CoinWatchError::UpdateCheck("Checksum must be 64 hexadecimal characters".into()));
    }
    Ok(())
}

/// Returns true if enough time has passed since the last version check.
/// If the check file doesn't exist, returns true (first-time check).
pub fn should_check_now() -> bool {
    let path = last_check_file();
    match std::fs::read_to_string(&path) {
        Ok(content) => {
            if let Ok(ts) = content.trim().parse::<u64>() {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                let elapsed = now.saturating_sub(ts);
                elapsed >= CHECK_INTERVAL_SECS
            } else {
                true
            }
        }
        Err(_) => true,
    }
}

/// Record the current time as the last version check timestamp.
pub fn record_check_time() {
    let path = last_check_file();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    if let Err(e) = std::fs::write(&path, now.to_string()) {
        tracing::warn!("update_checker: failed to write check timestamp: {}", e);
    }
}

/// Get the platform identifier string used in version.json keys.
pub fn current_platform() -> &'static str {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        "darwin-arm64"
    }
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    {
        "darwin-x86"
    }
    #[cfg(target_os = "windows")]
    {
        "windows"
    }
    #[cfg(target_os = "linux")]
    {
        "linux"
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        "unknown"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::version::SemVer;
    use std::sync::Mutex;

    static FILE_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn test_should_check_now_no_file() {
        let _lock = FILE_LOCK.lock().unwrap();
        let path = last_check_file();
        let _ = std::fs::remove_file(&path);
        assert!(should_check_now());
    }

    #[test]
    fn test_record_and_check_timing() {
        let _lock = FILE_LOCK.lock().unwrap();
        record_check_time();
        // Just recorded, so should NOT need to check again
        assert!(!should_check_now());
    }

    #[test]
    fn test_rejects_insecure_or_unverified_downloads() {
        assert!(validate_download_metadata("http://example.com/finter", &"a".repeat(64)).is_err());
        assert!(validate_download_metadata("https://example.com/finter", "abc").is_err());
        assert!(validate_download_metadata("https://example.com/finter", &"g".repeat(64)).is_err());
        assert!(validate_download_metadata("https://example.com/finter", &"A".repeat(64)).is_ok());
    }

    #[test]
    fn test_same_version_new_channel_build_is_update() {
        let current = SemVer::parse("0.1.0").unwrap();
        let remote = SemVer::parse("0.1.0").unwrap();
        assert!(is_update_available(
            &current, Some("old-sha"), "https://example.com/alpha/version.json", &remote, Some("new-sha")
        ));
        assert!(is_update_available(
            &current, Some("old-sha"), "https://example.com/beta/version.json", &remote, Some("new-sha")
        ));
        assert!(!is_update_available(
            &current, Some("new-sha"), "https://example.com/alpha/version.json", &remote, Some("new-sha")
        ));
        assert!(!is_update_available(
            &current, Some("old-sha"), "https://example.com/latest/version.json", &remote, Some("new-sha")
        ));
        assert!(!is_update_available(
            &current, Some("old-sha"), "https://example.com/alpha/version.json", &remote, None
        ));
    }

    #[test]
    fn test_current_platform_known() {
        let platform = current_platform();
        assert!(
            ["darwin-arm64", "darwin-x86", "windows", "linux", "unknown"].contains(&platform),
            "Unexpected platform: {}",
            platform
        );
    }
}
