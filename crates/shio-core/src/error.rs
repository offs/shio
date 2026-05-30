use thiserror::Error;

#[derive(Error, Debug)]
pub enum ShioError {
    #[error("network: {0}")]
    Network(#[from] reqwest::Error),

    #[error("HTTP {code}: {message}")]
    Http { code: u16, message: String },

    #[error("i/o: {0}")]
    Io(#[from] std::io::Error),

    #[error("database: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("database json in {field}: {source}")]
    DatabaseJson {
        field: &'static str,
        #[source]
        source: serde_json::Error,
    },

    #[error("database timestamp in {field}: {source}")]
    DatabaseTimestamp {
        field: &'static str,
        #[source]
        source: chrono::ParseError,
    },

    #[error("database value in {field}: {value}")]
    DatabaseValue { field: &'static str, value: String },

    #[error("unsupported database schema version {found}; expected {expected}")]
    UnsupportedDatabaseVersion { found: i64, expected: i64 },

    #[error("config: {0}")]
    Config(String),

    #[error("invalid url: {0}")]
    InvalidUrl(String),

    #[error("server does not support resuming")]
    NotResumable,

    #[error("URL is not a direct file link (server returned {content_type})")]
    NotADirectFile { content_type: String },

    #[error("size mismatch: expected {expected} bytes, got {actual}")]
    SizeMismatch { expected: u64, actual: u64 },

    #[error("cancelled")]
    Cancelled,

    #[error("extract: {0}")]
    Extract(String),

    #[error("password required")]
    PasswordRequired,

    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, ShioError>;

impl ShioError {
    pub fn short_label(&self) -> String {
        match self {
            Self::Network(e) => {
                if e.is_timeout() {
                    "connection timed out".into()
                } else if e.is_connect() {
                    "connection refused".into()
                } else {
                    "network error".into()
                }
            },
            Self::Http { code, .. } => match *code {
                401 => "unauthorized (401)".into(),
                403 => "forbidden (403)".into(),
                404 => "not found (404)".into(),
                429 => "rate limited (429)".into(),
                500..=599 => format!("server error ({code})"),
                _ => format!("http {code}"),
            },
            Self::Io(e) => match e.kind() {
                std::io::ErrorKind::PermissionDenied => "permission denied".into(),
                std::io::ErrorKind::NotFound => "path not found".into(),
                std::io::ErrorKind::StorageFull => "disk full".into(),
                _ => format!("disk error: {e}"),
            },
            Self::NotResumable => "server does not support resume".into(),
            Self::NotADirectFile { .. } => "not a direct file link".into(),
            Self::SizeMismatch { .. } => "incomplete download".into(),
            Self::Cancelled => "cancelled".into(),
            Self::PasswordRequired => "password required".into(),
            Self::Extract(msg) => friendly_extract(msg),
            Self::InvalidUrl(_) => "invalid url".into(),
            Self::DatabaseJson { .. }
            | Self::DatabaseTimestamp { .. }
            | Self::DatabaseValue { .. }
            | Self::UnsupportedDatabaseVersion { .. } => "corrupt database row".into(),
            Self::Database(_) | Self::Config(_) | Self::Other(_) => {
                let s = self.to_string();
                if s.len() > 48 {
                    format!("{}…", &s[..47])
                } else {
                    s
                }
            },
        }
    }
}

fn friendly_extract(msg: &str) -> String {
    let lower = msg.to_ascii_lowercase();
    if lower.contains("password") || lower.contains("encrypt") {
        "archive is password-protected".into()
    } else if lower.contains("crc") || lower.contains("checksum") || lower.contains("corrupt") {
        "archive is corrupt".into()
    } else if lower.contains("unsupported") || lower.contains("unknown format") {
        "unsupported archive format".into()
    } else if lower.contains("space") || lower.contains("disk full") {
        "disk full".into()
    } else {
        "extract failed".into()
    }
}
