use thiserror::Error;

/// Unified error type for all CoinWatch data operations.
///
/// Each variant maps to a distinct failure domain so callers can
/// pattern-match and present appropriate UI feedback.
#[derive(Error, Debug)]
pub enum CoinWatchError {
    #[error("Network error: {0}")]
    Network(#[from] reqwest::Error),

    #[error("Authentication failed: {0}")]
    Auth(String),

    #[error("Token expired")]
    TokenExpired,

    #[error("API error ({status}): {message}")]
    Api { status: u16, message: String },

    /// CoinWatch market-data request was rejected because the account balance
    /// is below the endpoint's credit cost.
    #[error("Insufficient credits: {message} (balance {balance}, required {required})")]
    InsufficientCredits { message: String, balance: i64, required: i64 },

    #[error("Deserialization error: {0}")]
    Deserialization(String),

    #[error("Exchange error: {0}")]
    Exchange(String),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Invalid request: {0}")]
    InvalidRequest(String),

    #[error("Session storage error: {0}")]
    Storage(String),

    #[error("Update check failed: {0}")]
    UpdateCheck(String),

    #[error("Update download failed: {0}")]
    UpdateDownload(String),

    #[error("Checksum verification failed: expected {expected}, got {actual}")]
    ChecksumMismatch { expected: String, actual: String },

    #[error("IO error: {0}")]
    Io(String),
}

/// Convenience alias used throughout the data layer.
pub type Result<T> = std::result::Result<T, CoinWatchError>;
