use std::path::PathBuf;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ShioError {
    #[error("network: {0}")]
    Network(#[from] reqwest::Error),

    #[error("HTTP {code}")]
    Http {
        code: u16,
        retry_after: Option<std::time::Duration>,
    },

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

    #[error("the database was not created by this version of Shio")]
    UnrecognizedDatabase,

    #[error("config: {0}")]
    Config(String),

    #[error("remote file changed while resuming")]
    ResourceChanged,

    #[error("destination already exists: {0}")]
    DestinationExists(PathBuf),

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

    #[error("the site blocked automated access")]
    BrowserCheck,

    #[error("the download page needs a captcha")]
    PageNeedsCaptcha,

    #[error("no file was found on the download page")]
    NoFileOnPage,

    #[error("the download page is password protected")]
    PagePasswordProtected,

    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, ShioError>;

const MAX_LABEL_CHARS: usize = 48;

impl ShioError {
    // reqwest's Display includes the request URL, which may carry tokens.
    pub(crate) fn safe_diagnostic(&self) -> String {
        match self {
            Self::Network(_) => self.short_label(),
            _ => self.to_string(),
        }
    }

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
                _ => format!("HTTP {code}"),
            },
            Self::Io(e) => match e.kind() {
                std::io::ErrorKind::PermissionDenied => "permission denied".into(),
                std::io::ErrorKind::NotFound => "path not found".into(),
                std::io::ErrorKind::StorageFull => "disk full".into(),
                _ => format!("disk error: {e}"),
            },
            Self::ResourceChanged => "remote file changed; restart required".into(),
            Self::DestinationExists(_) => "destination already exists".into(),
            Self::NotADirectFile { .. } => "not a direct file link".into(),
            Self::SizeMismatch { .. } => "incomplete download".into(),
            Self::Cancelled => "cancelled".into(),
            Self::PasswordRequired => "password required".into(),
            Self::BrowserCheck => "blocked by a bot check".into(),
            Self::PageNeedsCaptcha => "needs a captcha".into(),
            Self::NoFileOnPage => "no file found on page".into(),
            Self::PagePasswordProtected => "link is password protected".into(),
            Self::DatabaseJson { .. }
            | Self::DatabaseTimestamp { .. }
            | Self::DatabaseValue { .. }
            | Self::UnrecognizedDatabase => "corrupt database row".into(),
            Self::Database(_) | Self::Config(_) | Self::Extract(_) | Self::Other(_) => {
                let label = self.to_string();
                match label.char_indices().nth(MAX_LABEL_CHARS - 1) {
                    Some((end, _)) => format!("{}…", &label[..end]),
                    None => label,
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ShioError;

    #[test]
    fn short_label_truncates_unicode_at_a_character_boundary() {
        let label = ShioError::Config("界".repeat(60)).short_label();
        assert!(label.ends_with('…'));
        assert_eq!(label.chars().count(), super::MAX_LABEL_CHARS);
    }
}
