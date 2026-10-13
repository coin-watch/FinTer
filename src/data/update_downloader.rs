use crate::data::client::tokio_runtime;
use crate::data::error::{CoinWatchError, Result};
use crate::data::version::{APP_VERSION, VersionInfo, update_dir};
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Shared progress state for UI polling.
#[derive(Debug, Clone)]
pub struct DownloadProgress {
    /// Bytes downloaded so far.
    pub downloaded: u64,
    /// Total bytes expected (0 if unknown).
    pub total: u64,
    /// Progress fraction 0.0 - 1.0.
    pub fraction: f64,
    /// Whether the download is complete.
    pub complete: bool,
    /// Error message if download failed.
    pub error: Option<String>,
}

impl Default for DownloadProgress {
    fn default() -> Self {
        Self {
            downloaded: 0,
            total: 0,
            fraction: 0.0,
            complete: false,
            error: None,
        }
    }
}

/// Download the update binary for the current platform.
///
/// Returns the path to the downloaded file on success.
/// Progress is reported via the shared `DownloadProgress` mutex.
///
/// Blocking — run from a background thread.
pub fn download_update(
    version_info: &VersionInfo,
    progress: Arc<Mutex<DownloadProgress>>,
) -> Result<PathBuf> {
    let url = version_info
        .download_url_for_current_platform()
        .ok_or_else(|| {
            CoinWatchError::UpdateDownload("No download URL for current platform".into())
        })?;

    let expected_checksum = version_info.checksum_for_current_platform().ok_or_else(|| {
        CoinWatchError::UpdateDownload("Missing required SHA256 checksum".into())
    })?;
    if expected_checksum.len() != 64 || !expected_checksum.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(CoinWatchError::UpdateDownload("Invalid SHA256 checksum metadata".into()));
    }
    let parsed_url = reqwest::Url::parse(url)
        .map_err(|e| CoinWatchError::UpdateDownload(format!("Invalid download URL: {e}")))?;
    if parsed_url.scheme() != "https" {
        return Err(CoinWatchError::UpdateDownload("Download URL must use HTTPS".into()));
    }

    if crate::data::version::SemVer::parse(&version_info.version).is_none() {
        return Err(CoinWatchError::UpdateDownload("Invalid version".into()));
    }
    let filename = download_filename(&version_info.version);
    let dest = update_dir().join(&filename);
    let temp = dest.with_extension(format!("{}.part", uuid::Uuid::new_v4()));
    let expected_checksum = expected_checksum.to_owned();
    let download_url = parsed_url.to_string();
    let _ = std::fs::remove_file(&temp);

    tracing::info!(
        "update_downloader: downloading {} to {}",
        url,
        dest.display()
    );

    let rt = tokio_runtime();
    let result = rt.block_on(async {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(300))
            .build()
            .map_err(|e| CoinWatchError::UpdateDownload(format!("HTTP client error: {e}")))?;

        let resp = client
            .get(download_url)
            .header("User-Agent", format!("FinTer/{}", APP_VERSION))
            .send()
            .await
            .map_err(|e| CoinWatchError::UpdateDownload(format!("Network error: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
            return Err(CoinWatchError::UpdateDownload(format!(
                "HTTP {}",
                status.as_u16()
            )));
        }

        const MAX_BYTES: u64 = 512 * 1024 * 1024;
        let total = resp.content_length().unwrap_or(0);
        if total > MAX_BYTES { return Err(CoinWatchError::UpdateDownload("Artifact exceeds 512 MiB limit".into())); }
        {
            let mut p = progress.lock().unwrap();
            p.total = total;
        }

        // Stream response body to file while computing SHA256
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&temp)
            .map_err(|e| CoinWatchError::Io(format!("Failed to create file: {e}")))?;

        let mut hasher = Sha256::new();
        let mut downloaded: u64 = 0;

        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| CoinWatchError::UpdateDownload(format!("Download error: {e}")))?;
            if downloaded + chunk.len() as u64 > MAX_BYTES { return Err(CoinWatchError::UpdateDownload("Artifact exceeds 512 MiB limit".into())); }
            file.write_all(&chunk)
                .map_err(|e| CoinWatchError::Io(format!("Write error: {e}")))?;
            hasher.update(&chunk);
            downloaded += chunk.len() as u64;

            let fraction = if total > 0 {
                downloaded as f64 / total as f64
            } else {
                0.0
            };

            let mut p = progress.lock().unwrap();
            p.downloaded = downloaded;
            p.fraction = fraction;
        }

        if downloaded == 0 || (total != 0 && downloaded != total) { return Err(CoinWatchError::UpdateDownload("Truncated or empty artifact".into())); }
        file.sync_all()
            .map_err(|e| CoinWatchError::Io(format!("Sync error: {e}")))?;
        drop(file);

        // Verify SHA256 checksum
        let actual_hash = format!("{:x}", hasher.finalize());
        tracing::info!("update_downloader: SHA256 = {}", actual_hash);

        let expected_lower = expected_checksum.to_lowercase();
        if actual_hash != expected_lower {
            return Err(CoinWatchError::ChecksumMismatch { expected: expected_lower, actual: actual_hash });
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(metadata) = std::fs::metadata(&temp) {
                let mut perms = metadata.permissions();
                perms.set_mode(0o755);
                let _ = std::fs::set_permissions(&temp, perms);
            }
        }

        std::fs::rename(&temp, &dest).map_err(|e| CoinWatchError::Io(format!("Atomic staging failed: {e}")))?;
        std::fs::write(checksum_path(&dest), &expected_lower)
            .map_err(|e| CoinWatchError::Io(format!("Checksum sidecar failed: {e}")))?;

        {
            let mut p = progress.lock().unwrap();
            p.fraction = 1.0;
            p.complete = true;
        }

        tracing::info!(
            "update_downloader: download complete, {} bytes at {}",
            downloaded,
            dest.display()
        );

        Ok(dest)
    });
    if result.is_err() { let _ = std::fs::remove_file(&temp); }
    result
}

pub fn checksum_path(path: &std::path::Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".sha256");
    PathBuf::from(name)
}

/// Verify the SHA256 checksum of an existing file on disk.
pub fn verify_checksum(path: &std::path::Path, expected: &str) -> Result<()> {
    let mut file = std::fs::File::open(path).map_err(|e| CoinWatchError::Io(format!("Read error: {e}")))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let n = file.read(&mut buffer).map_err(|e| CoinWatchError::Io(format!("Read error: {e}")))?;
        if n == 0 { break; }
        hasher.update(&buffer[..n]);
    }
    let actual = format!("{:x}", hasher.finalize());
    let expected_lower = expected.to_lowercase();

    if actual != expected_lower {
        Err(CoinWatchError::ChecksumMismatch {
            expected: expected_lower,
            actual,
        })
    } else {
        Ok(())
    }
}

/// Generate a filename for the downloaded update binary.
fn download_filename(version: &str) -> String {
    let ext = platform_extension();
    format!("finter-v{}{}", version, ext)
}

/// Get the platform-specific file extension.
fn platform_extension() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        ""
    }
    #[cfg(target_os = "windows")]
    {
        ".exe"
    }
    #[cfg(target_os = "linux")]
    {
        ""
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        ""
    }
}

/// Clean up old update files, keeping only the most recent N versions.
pub fn cleanup_old_updates(keep: usize) {
    let dir = update_dir();
    let mut entries: Vec<_> = std::fs::read_dir(&dir)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name()
                .to_str()
                .is_some_and(|n| n.starts_with("finter-v"))
        })
        .collect();

    // Sort by modification time, newest first
    entries.sort_by(|a, b| {
        let a_time = a
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        let b_time = b
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        b_time.cmp(&a_time)
    });

    // Remove everything beyond the `keep` threshold
    for entry in entries.into_iter().skip(keep) {
        tracing::info!(
            "update_downloader: cleaning up old update: {}",
            entry.path().display()
        );
        std::fs::remove_file(entry.path()).ok();
    }
}

use std::time::SystemTime;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_download_filename() {
        let name = download_filename("0.1.1");
        assert!(name.starts_with("finter-v0.1.1"));
    }

    #[test]
    fn test_verify_checksum_success() {
        let dir = std::env::temp_dir();
        let path = dir.join("test_checksum_verify.bin");
        let data = b"hello world";
        std::fs::write(&path, data).unwrap();

        let expected = format!("{:x}", Sha256::digest(data));
        assert!(verify_checksum(&path, &expected).is_ok());

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_verify_checksum_failure() {
        let dir = std::env::temp_dir();
        let path = dir.join("test_checksum_fail.bin");
        std::fs::write(&path, b"hello world").unwrap();

        let result = verify_checksum(
            &path,
            "0000000000000000000000000000000000000000000000000000000000000000",
        );
        assert!(result.is_err());

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_platform_extension_known() {
        let ext = platform_extension();
        assert!(
            ["", ".exe", "", ""].contains(&ext),
            "Unexpected extension: {}",
            ext
        );
    }

    #[test]
    fn test_download_progress_default() {
        let p = DownloadProgress::default();
        assert_eq!(p.downloaded, 0);
        assert_eq!(p.total, 0);
        assert!(!p.complete);
        assert!(p.error.is_none());
    }
}
