use crate::error::{Result, ShioError};
use crate::filename::{
    FALLBACK_FILENAME, parse_content_disposition, path_filename, rar_part, sanitize_filename,
};
use crate::types::Download;
use reqwest::StatusCode;
use reqwest::header::{self, HeaderMap};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

const MAX_RETRIES: u32 = 30;
const MAX_RETRY_DELAY: Duration = Duration::from_mins(1);
const PROBE_BODY_TIMEOUT: Duration = Duration::from_secs(1);
const MAX_LANDING_PAGE_BYTES: u64 = 64 * 1024 * 1024;

pub(crate) struct HttpProbe {
    pub(crate) content_length: Option<u64>,
    pub(crate) accept_ranges: bool,
    pub(crate) content_type: Option<String>,
    pub(crate) validator: Option<String>,
    disposition_filename: Option<String>,
    url_filename: Option<String>,
}

impl HttpProbe {
    pub(crate) fn filename(&self) -> Option<&str> {
        self.disposition_filename
            .as_deref()
            .or(self.url_filename.as_deref())
    }

    /// Replaces only URL-path-derived names; user, fragment and archive-part names stay.
    pub(crate) fn server_filename(&self, download: &Download) -> Option<&str> {
        let url = url::Url::parse(&download.http()?.url).ok()?;
        let derived = path_filename(&url).map(|name| sanitize_filename(&name));
        if rar_part(&download.filename).is_some()
            || download.filename != derived.as_deref().unwrap_or(FALLBACK_FILENAME)
        {
            return None;
        }
        let name = self
            .disposition_filename
            .as_deref()
            .or_else(|| self.url_filename.as_deref().filter(|_| derived.is_none()))?;
        (name != download.filename).then_some(name)
    }
}

pub(crate) async fn probe_with_retries(
    client: &reqwest::Client,
    url: &str,
    custom_headers: &[(String, String)],
    max_retries: u32,
    cancel: &CancellationToken,
) -> Result<HttpProbe> {
    let mut attempt = 0;
    loop {
        let error = tokio::select! {
            () = cancel.cancelled() => return Err(ShioError::Cancelled),
            result = probe(client, url, custom_headers) => match result {
                Ok(probe) => return Ok(probe),
                Err(error) => error,
            },
        };
        attempt += 1;
        let Some(delay) = retry_delay(&error, attempt, max_retries) else {
            return Err(error);
        };
        tracing::warn!(
            "probe retry {attempt}/{max_retries} in {delay:?}: {}",
            error.safe_diagnostic()
        );
        tokio::select! {
            () = cancel.cancelled() => return Err(ShioError::Cancelled),
            () = tokio::time::sleep(delay) => {},
        }
    }
}

pub(crate) async fn probe(
    client: &reqwest::Client,
    url: &str,
    custom_headers: &[(String, String)],
) -> Result<HttpProbe> {
    let request = client
        .get(crate::worker::url_without_fragment(url))
        .header(header::RANGE, "bytes=0-1");
    let response = apply_custom_headers(request, custom_headers)?
        .send()
        .await?;
    let status = response.status();
    let headers = response.headers();
    let empty = status == StatusCode::RANGE_NOT_SATISFIABLE
        && header_str(headers, header::CONTENT_RANGE)
            .is_some_and(|range| range.trim() == "bytes */0");
    if !status.is_success() && !empty {
        return Err(http_error(&response));
    }
    reject_encoded_body(headers)?;

    let content_length = if empty {
        Some(0)
    } else if status == StatusCode::PARTIAL_CONTENT {
        header_str(headers, header::CONTENT_RANGE)
            .and_then(|range| range.rsplit_once('/'))
            .and_then(|(_, total)| total.trim().parse().ok())
    } else {
        header_str(headers, header::CONTENT_LENGTH).and_then(|length| length.parse().ok())
    };
    let content_type = header_str(headers, header::CONTENT_TYPE).map(|value| {
        let essence = value.split(';').next().unwrap_or(value);
        essence.trim().to_ascii_lowercase()
    });
    let disposition =
        header_str(headers, header::CONTENT_DISPOSITION).map(parse_content_disposition);
    let attachment = disposition.as_ref().is_some_and(|d| d.attachment);
    if !attachment && looks_like_html_landing_page(content_type.as_deref(), content_length) {
        return Err(ShioError::NotADirectFile {
            content_type: content_type.unwrap_or_else(|| "unknown".into()),
        });
    }

    // RFC 9110 §13.1.5: never fall back to Last-Modified when the only entity tag is weak.
    let validator = match header_str(headers, header::ETAG) {
        Some(etag) => (!etag.starts_with("W/")).then(|| etag.to_owned()),
        None => header_str(headers, header::LAST_MODIFIED).map(str::to_owned),
    };
    let probe = HttpProbe {
        content_length,
        accept_ranges: status == StatusCode::PARTIAL_CONTENT,
        content_type,
        validator,
        disposition_filename: disposition.and_then(|d| d.filename),
        url_filename: path_filename(response.url()).map(|name| sanitize_filename(&name)),
    };
    // hyper pools a connection only once its body is read, so the first transfer can reuse it.
    if probe.accept_ranges && response.content_length().is_some_and(|length| length <= 2) {
        let _ = tokio::time::timeout(PROBE_BODY_TIMEOUT, response.bytes()).await;
    }
    Ok(probe)
}

/// Every request sends `Accept-Encoding: identity`; any other coding would reach disk as-is.
pub(crate) fn reject_encoded_body(headers: &HeaderMap) -> Result<()> {
    match headers.get(header::CONTENT_ENCODING) {
        Some(coding) if !coding.as_bytes().eq_ignore_ascii_case(b"identity") => {
            Err(ShioError::Other(
                "server sent an encoded body despite Accept-Encoding: identity".into(),
            ))
        },
        _ => Ok(()),
    }
}

pub(crate) const fn is_transient(error: &ShioError) -> bool {
    match error {
        ShioError::Network(_) | ShioError::SizeMismatch { .. } => true,
        ShioError::Http { code, .. } => matches!(code, 408 | 429 | 500..=599),
        _ => false,
    }
}

pub(crate) fn retry_delay(error: &ShioError, attempt: u32, max_retries: u32) -> Option<Duration> {
    if !is_transient(error) || attempt > max_retries.min(MAX_RETRIES) {
        return None;
    }
    let delay = match error {
        ShioError::Http {
            retry_after: Some(delay),
            ..
        } => *delay,
        _ => Duration::from_secs(1_u64 << (attempt - 1).min(6)),
    };
    Some(delay.min(MAX_RETRY_DELAY))
}

fn header_str(headers: &HeaderMap, name: header::HeaderName) -> Option<&str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

pub(crate) fn http_error(response: &reqwest::Response) -> ShioError {
    let retry_after = header_str(response.headers(), header::RETRY_AFTER).and_then(|value| {
        value
            .trim()
            .parse()
            .map(Duration::from_secs)
            .ok()
            .or_else(|| {
                let at = chrono::DateTime::parse_from_rfc2822(value).ok()?;
                (at.to_utc() - chrono::Utc::now()).to_std().ok()
            })
    });
    ShioError::Http {
        code: response.status().as_u16(),
        retry_after,
    }
}

pub(crate) fn apply_custom_headers(
    mut request: reqwest::RequestBuilder,
    headers: &[(String, String)],
) -> Result<reqwest::RequestBuilder> {
    validate_custom_headers(headers)?;
    for (key, value) in headers {
        let name = header::HeaderName::from_bytes(key.as_bytes())
            .map_err(|e| ShioError::Config(format!("invalid header {key}: {e}")))?;
        if name == header::RANGE || name == header::IF_RANGE {
            return Err(ShioError::Config(format!(
                "header {key} is managed by the download engine"
            )));
        }
        let value = header::HeaderValue::from_str(value)
            .map_err(|e| ShioError::Config(format!("invalid header {key}: {e}")))?;
        request = request.header(name, value);
    }
    Ok(request)
}

pub(crate) fn validate_custom_headers(headers: &[(String, String)]) -> Result<()> {
    const MAX_HEADERS: usize = 64;
    const MAX_HEADER_NAME_BYTES: usize = 256;
    const MAX_HEADER_VALUE_BYTES: usize = 8 * 1024;
    const MAX_HEADER_BYTES: usize = 32 * 1024;

    let total: usize = headers
        .iter()
        .map(|(name, value)| name.len() + value.len())
        .sum();
    if headers.len() > MAX_HEADERS
        || total > MAX_HEADER_BYTES
        || headers.iter().any(|(name, value)| {
            name.len() > MAX_HEADER_NAME_BYTES || value.len() > MAX_HEADER_VALUE_BYTES
        })
    {
        return Err(ShioError::Config(
            "custom HTTP headers are too large".into(),
        ));
    }
    Ok(())
}

fn looks_like_html_landing_page(content_type: Option<&str>, content_length: Option<u64>) -> bool {
    content_type
        .is_some_and(|ct| ct.starts_with("text/html") || ct.starts_with("application/xhtml"))
        && content_length.is_none_or(|n| n < MAX_LANDING_PAGE_BYTES)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    pub(crate) async fn one_shot_server(
        response: Vec<u8>,
        hold_open: bool,
    ) -> (String, tokio::sync::oneshot::Receiver<String>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test server");
        let address = listener.local_addr().expect("test server address");
        let (request_tx, request_rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept request");
            let mut request = vec![0_u8; 4096];
            let read = socket.read(&mut request).await.expect("read request");
            let _ = request_tx.send(String::from_utf8_lossy(&request[..read]).into_owned());
            socket.write_all(&response).await.expect("write response");
            if hold_open {
                std::future::pending::<()>().await;
            }
        });
        (format!("http://{address}/file.bin"), request_rx)
    }

    async fn probe_response(response: &str) -> Result<HttpProbe> {
        let (url, _request) = one_shot_server(response.as_bytes().to_vec(), false).await;
        probe(&reqwest::Client::new(), &url, &[]).await
    }

    #[tokio::test]
    async fn probe_classifies_ranges_validators_and_errors() {
        let ranged = probe_response(
            "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 0-1/1234\r\nContent-Length: 2\r\nETag: \"v1\"\r\n\r\nab",
        )
        .await
        .expect("ranged probe");
        assert_eq!(ranged.content_length, Some(1234));
        assert!(ranged.accept_ranges);
        assert_eq!(ranged.validator.as_deref(), Some("\"v1\""));

        let ignored = probe_response(
            "HTTP/1.1 200 OK\r\nAccept-Ranges: bytes\r\nContent-Length: 3\r\nETag: W/\"weak\"\r\nLast-Modified: Tue, 25 Aug 2026 12:00:00 GMT\r\n\r\nabc",
        )
        .await
        .expect("full-body probe");
        assert_eq!(ignored.content_length, Some(3));
        assert!(!ignored.accept_ranges);
        assert_eq!(ignored.validator, None);

        let empty = probe_response(
            "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */0\r\nContent-Length: 0\r\n\r\n",
        )
        .await
        .expect("empty file probe");
        assert_eq!(empty.content_length, Some(0));

        let gone = probe_response("HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */9\r\nContent-Length: 0\r\n\r\n").await;
        assert!(matches!(gone, Err(ShioError::Http { code: 416, .. })));

        let busy = probe_response(
            "HTTP/1.1 503 Service Unavailable\r\nRetry-After: 7\r\nContent-Length: 0\r\n\r\n",
        )
        .await;
        assert!(matches!(
            busy,
            Err(ShioError::Http { code: 503, retry_after: Some(delay) }) if delay == Duration::from_secs(7)
        ));
    }

    #[test]
    fn html_landing_pages_are_blocked_unless_large() {
        for (content_type, length, blocked) in [
            (Some("text/html"), Some(5_000_000), true),
            (Some("application/xhtml+xml"), None, true),
            (Some("text/html"), Some(100 * 1024 * 1024), false),
            (Some("video/mp4"), None, false),
            (None, None, false),
        ] {
            assert_eq!(
                looks_like_html_landing_page(content_type, length),
                blocked,
                "{content_type:?} {length:?}"
            );
        }
    }

    #[test]
    fn server_filename_only_replaces_url_derived_names() {
        let probe = |disposition: Option<&str>, url_name: Option<&str>| HttpProbe {
            content_length: None,
            accept_ranges: false,
            content_type: None,
            validator: None,
            disposition_filename: disposition.map(str::to_owned),
            url_filename: url_name.map(str::to_owned),
        };
        let download = |url: &str, name: &str| {
            let mut download = Download::new(url.into(), std::path::PathBuf::from("/tmp"));
            download.filename = name.into();
            download
        };
        let server = probe(Some("real.zip"), Some("cdn-object"));
        let redirect = probe(None, Some("real.zip"));
        for (download, probe, expected) in [
            (
                download("https://x.test/get.php", "get.php"),
                &server,
                Some("real.zip"),
            ),
            (
                download("https://x.test/", "download"),
                &redirect,
                Some("real.zip"),
            ),
            (
                download("https://x.test/foo.zip", "foo.zip"),
                &redirect,
                None,
            ),
            (
                download("https://x.test/get.php", "mine.zip"),
                &server,
                None,
            ),
            (
                download("https://x.test/id#release.part01.rar", "release.part01.rar"),
                &server,
                None,
            ),
            (
                download("https://x.test/a.part01.rar", "a.part01.rar"),
                &server,
                None,
            ),
        ] {
            assert_eq!(
                probe.server_filename(&download),
                expected,
                "{}",
                download.filename
            );
        }
    }

    #[tokio::test]
    async fn custom_headers_are_validated_before_any_request() {
        let client = reqwest::Client::new();
        for headers in [
            vec![("bad header".to_string(), "value".to_string())],
            vec![("Range".to_string(), "bytes=10-20".to_string())],
            vec![("X-Metadata".to_string(), "x".repeat(8 * 1024)); 5],
        ] {
            let result = probe(&client, "http://127.0.0.1:1/file.bin", &headers).await;
            assert!(matches!(result, Err(ShioError::Config(_))));
        }
    }
}
