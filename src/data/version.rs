use serde::{Deserialize, Serialize};
use std::fmt;

/// Current app version, embedded at compile time from Cargo.toml.
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Version endpoint URL for checking updates.
/// Points to public updater metadata published by GitHub Actions.
const DEFAULT_VERSION_URL: &str = "";

/// Build identity embedded by GitHub Actions for alpha/beta update checks.
pub const BUILD_ID: Option<&str> = option_env!("FINTER_BUILD_ID");

/// Get the version check URL, allowing override via environment variable.
pub fn version_check_url() -> String {
    std::env::var("FINTER_VERSION_URL").unwrap_or_else(|_| {
        option_env!("FINTER_VERSION_URL").unwrap_or(DEFAULT_VERSION_URL).to_string()
    })
}

/// Remote version metadata returned from version.json in GitHub Releases.
///
/// Schema matches the auto-generated version.json from GitHub Actions CI/CD:
/// ```json
/// {
///   "version": "0.1.1",
///   "released_at": "2026-04-28T10:30:00Z",
///   "changelog": "- Fixed lag on tape\n- Better error handling",
///   "downloads": {
///   "darwin-arm64": "https://github.com/coin-watch/FinTer/releases/download/v0.1.1/finter-v0.1.1-darwin-arm64",
///   "windows": "https://github.com/coin-watch/FinTer/releases/download/v0.1.1/finter-v0.1.1-windows.exe",
///   "linux": "https://github.com/coin-watch/FinTer/releases/download/v0.1.1/finter-v0.1.1-linux"
///   },
///   "checksums": {
///     "darwin-arm64": "abc123...",
///     "windows": "def456...",
///     "linux": "ghi789..."
///   }
/// }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VersionInfo {
    pub version: String,
    #[serde(default)]
    pub build_id: Option<String>,
    #[serde(default)]
    pub released_at: Option<String>,
    #[serde(default)]
    pub changelog: Option<String>,
    #[serde(default)]
    pub downloads: VersionDownloads,
    #[serde(default)]
    pub checksums: VersionChecksums,
}

/// Platform-specific GitHub Release download URLs.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct VersionDownloads {
    #[serde(default, rename = "darwin-arm64")]
    pub darwin_arm64: Option<String>,
    #[serde(default, rename = "darwin-x86")]
    pub darwin_x86: Option<String>,
    #[serde(default)]
    pub windows: Option<String>,
    #[serde(default)]
    pub linux: Option<String>,
}

/// Platform-specific SHA256 checksums for binary verification.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct VersionChecksums {
    #[serde(default, rename = "darwin-arm64")]
    pub darwin_arm64: Option<String>,
    #[serde(default, rename = "darwin-x86")]
    pub darwin_x86: Option<String>,
    #[serde(default)]
    pub windows: Option<String>,
    #[serde(default)]
    pub linux: Option<String>,
}

/// Parsed semantic version for comparison.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemVer {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl SemVer {
    /// Parse a version string like "0.1.0" or "v0.1.0".
    pub fn parse(version: &str) -> Option<Self> {
        let v = version.strip_prefix('v').unwrap_or(version);
        let parts: Vec<&str> = v.split('.').collect();
        if parts.len() != 3 {
            return None;
        }
        Some(Self {
            major: parts[0].parse().ok()?,
            minor: parts[1].parse().ok()?,
            patch: parts[2].parse().ok()?,
        })
    }

    /// Returns true if `self` is newer than `other`.
    pub fn is_newer_than(&self, other: &SemVer) -> bool {
        (self.major, self.minor, self.patch) > (other.major, other.minor, other.patch)
    }
}

impl fmt::Display for SemVer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl VersionInfo {
    /// Get the download URL for the current platform.
    pub fn download_url_for_current_platform(&self) -> Option<&str> {
        self.platform_download().map(|s| s.as_str())
    }

    /// Get the SHA256 checksum for the current platform.
    pub fn checksum_for_current_platform(&self) -> Option<&str> {
        self.platform_checksum().map(|s| s.as_str())
    }

    #[cfg(target_os = "macos")]
    fn platform_download(&self) -> Option<&String> {
        if cfg!(target_arch = "aarch64") {
            self.downloads.darwin_arm64.as_ref()
        } else {
            self.downloads.darwin_x86.as_ref()
        }
    }

    #[cfg(target_os = "windows")]
    fn platform_download(&self) -> Option<&String> {
        self.downloads.windows.as_ref()
    }

    #[cfg(target_os = "linux")]
    fn platform_download(&self) -> Option<&String> {
        self.downloads.linux.as_ref()
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    fn platform_download(&self) -> Option<&String> {
        None
    }

    #[cfg(target_os = "macos")]
    fn platform_checksum(&self) -> Option<&String> {
        if cfg!(target_arch = "aarch64") {
            self.checksums.darwin_arm64.as_ref()
        } else {
            self.checksums.darwin_x86.as_ref()
        }
    }

    #[cfg(target_os = "windows")]
    fn platform_checksum(&self) -> Option<&String> {
        self.checksums.windows.as_ref()
    }

    #[cfg(target_os = "linux")]
    fn platform_checksum(&self) -> Option<&String> {
        self.checksums.linux.as_ref()
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    fn platform_checksum(&self) -> Option<&String> {
        None
    }
}

/// Represents the current state of the update system.
#[derive(Debug, Clone, PartialEq)]
pub enum UpdateState {
    /// No update activity.
    Idle,
    /// Checking for updates.
    Checking,
    /// An update is available.
    Available(Box<VersionInfo>),
    /// Downloading update (progress 0.0 - 1.0).
    Downloading { version: String, progress: f64 },
    /// Download complete, ready to install.
    ReadyToInstall { version: String, path: String },
    /// Update installed, restart required to take effect.
    RestartRequired { version: String },
    /// Update failed with an error message.
    Failed(String),
}

/// The update storage directory under ~/.finter/updates/
pub fn update_dir() -> std::path::PathBuf {
    let base = dirs::config_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("finter")
        .join("updates");
    std::fs::create_dir_all(&base).ok();
    base
}

/// The backup directory for rollback: ~/.finter/app/backup/
pub fn backup_dir() -> std::path::PathBuf {
    let base = dirs::config_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("finter")
        .join("app")
        .join("backup");
    std::fs::create_dir_all(&base).ok();
    base
}

/// Path to the last-check timestamp file.
pub fn last_check_file() -> std::path::PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("finter")
        .join("last_update_check")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_semver_parse_basic() {
        let v = SemVer::parse("0.1.0").unwrap();
        assert_eq!(v.major, 0);
        assert_eq!(v.minor, 1);
        assert_eq!(v.patch, 0);
    }

    #[test]
    fn test_semver_parse_with_prefix() {
        let v = SemVer::parse("v1.2.3").unwrap();
        assert_eq!(v.major, 1);
        assert_eq!(v.minor, 2);
        assert_eq!(v.patch, 3);
    }

    #[test]
    fn test_semver_parse_invalid() {
        assert!(SemVer::parse("1.2").is_none());
        assert!(SemVer::parse("abc").is_none());
        assert!(SemVer::parse("").is_none());
    }

    #[test]
    fn test_semver_comparison() {
        let v010 = SemVer::parse("0.1.0").unwrap();
        let v011 = SemVer::parse("0.1.1").unwrap();
        let v020 = SemVer::parse("0.2.0").unwrap();
        let v100 = SemVer::parse("1.0.0").unwrap();

        assert!(v011.is_newer_than(&v010));
        assert!(v020.is_newer_than(&v011));
        assert!(v100.is_newer_than(&v020));
        assert!(!v010.is_newer_than(&v011));
        assert!(!v010.is_newer_than(&v010));
    }

    #[test]
    fn test_semver_display() {
        let v = SemVer::parse("1.2.3").unwrap();
        assert_eq!(v.to_string(), "1.2.3");
    }

    #[test]
    fn test_version_info_deserialization() {
        let json = r#"{
            "version": "0.1.1",
            "build_id": "sha123",
            "released_at": "2026-04-28T10:30:00Z",
            "changelog": "- Fixed lag",
            "downloads": {
                "darwin-arm64": "https://github.com/coin-watch/FinTer/releases/download/v0.1.1/finter-v0.1.1-darwin-arm64",
                "windows": "https://github.com/coin-watch/FinTer/releases/download/v0.1.1/finter-v0.1.1-windows.exe",
                "linux": "https://github.com/coin-watch/FinTer/releases/download/v0.1.1/finter-v0.1.1-linux"
            },
            "checksums": {
                "darwin-arm64": "abc123",
                "windows": "def456",
                "linux": "ghi789"
            }
        }"#;

        let info: VersionInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.version, "0.1.1");
        assert_eq!(info.released_at.as_deref(), Some("2026-04-28T10:30:00Z"));
        assert_eq!(info.changelog.as_deref(), Some("- Fixed lag"));
        assert_eq!(
            info.downloads.darwin_arm64.as_deref(),
            Some("https://github.com/coin-watch/FinTer/releases/download/v0.1.1/finter-v0.1.1-darwin-arm64")
        );
        assert_eq!(info.checksums.windows.as_deref(), Some("def456"));
    }

    #[test]
    fn test_version_info_minimal_deserialization() {
        let json = r#"{"version": "0.1.0"}"#;
        let info: VersionInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.version, "0.1.0");
        assert!(info.downloads.darwin_arm64.is_none());
        assert!(info.checksums.windows.is_none());
    }

    #[test]
    fn test_app_version_embedded() {
        let v = SemVer::parse(APP_VERSION);
        assert!(v.is_some(), "APP_VERSION should be a valid semver");
    }

    #[test]
    fn test_update_dir_creation() {
        let dir = update_dir();
        assert!(dir.exists());
    }
}
