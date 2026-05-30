use crate::error::{Result, ShioError};

pub(crate) struct ServerProbe {
    pub(crate) content_length: Option<u64>,
    pub(crate) accept_ranges: bool,
    pub(crate) response_filename: String,
    pub(crate) final_url: String,
    pub(crate) content_type: Option<String>,
    pub(crate) has_attachment: bool,
}

pub(crate) async fn probe_server(
    client: &reqwest::Client,
    url: &str,
    custom_headers: &[(String, String)],
) -> Result<ServerProbe> {
    let req = client.get(url).header(reqwest::header::RANGE, "bytes=0-1");
    let req = apply_custom_headers(req, custom_headers)?;

    let response = req.send().await?;
    let status = response.status();
    if !status.is_success() && status != reqwest::StatusCode::PARTIAL_CONTENT {
        return Err(ShioError::Http {
            code: status.as_u16(),
            message: status.canonical_reason().unwrap_or("unknown").to_string(),
        });
    }

    let final_url = response.url().to_string();
    let headers = response.headers().clone();

    let content_type = headers
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.split(';').next().unwrap_or(s).trim().to_ascii_lowercase());

    let has_attachment = headers
        .get(reqwest::header::CONTENT_DISPOSITION)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.to_ascii_lowercase().contains("attachment"));

    let accept_ranges = status == reqwest::StatusCode::PARTIAL_CONTENT
        || headers
            .get(reqwest::header::ACCEPT_RANGES)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.to_ascii_lowercase().contains("bytes"));

    let content_length = if status == reqwest::StatusCode::PARTIAL_CONTENT {
        headers
            .get(reqwest::header::CONTENT_RANGE)
            .and_then(|v| v.to_str().ok())
            .and_then(parse_content_range_total)
    } else {
        headers
            .get(reqwest::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse().ok())
    };

    let response_filename = crate::filename::extract_filename(&final_url, Some(&headers));

    if looks_like_html_landing_page(content_type.as_deref(), has_attachment, content_length) {
        return Err(ShioError::NotADirectFile {
            content_type: content_type.unwrap_or_else(|| "unknown".to_string()),
        });
    }

    Ok(ServerProbe {
        content_length,
        accept_ranges,
        response_filename,
        final_url,
        content_type,
        has_attachment,
    })
}

pub(crate) fn apply_custom_headers(
    mut request: reqwest::RequestBuilder,
    headers: &[(String, String)],
) -> Result<reqwest::RequestBuilder> {
    for (key, value) in headers {
        let name = reqwest::header::HeaderName::from_bytes(key.as_bytes())
            .map_err(|e| ShioError::Config(format!("invalid header {key}: {e}")))?;
        let value = reqwest::header::HeaderValue::from_str(value)
            .map_err(|e| ShioError::Config(format!("invalid header {key}: {e}")))?;
        request = request.header(name, value);
    }
    Ok(request)
}

fn parse_content_range_total(value: &str) -> Option<u64> {
    value.rsplit('/').next().and_then(|s| s.trim().parse().ok())
}

fn looks_like_html_landing_page(
    content_type: Option<&str>,
    has_attachment: bool,
    content_length: Option<u64>,
) -> bool {
    if has_attachment {
        return false;
    }
    let Some(ct) = content_type else { return false };
    if !(ct.starts_with("text/html") || ct.starts_with("application/xhtml")) {
        return false;
    }
    content_length.is_none_or(|n| n < 64 * 1024 * 1024)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_range_total_parses_standard_header() {
        assert_eq!(parse_content_range_total("bytes 0-0/12345"), Some(12345));
        assert_eq!(parse_content_range_total("bytes 100-200/1000"), Some(1000));
    }

    #[test]
    fn content_range_total_handles_unknown_total() {
        assert_eq!(parse_content_range_total("bytes 0-0/*"), None);
    }

    #[test]
    fn html_landing_page_detection() {
        assert!(looks_like_html_landing_page(
            Some("text/html"),
            false,
            Some(5_000_000),
        ));
        assert!(looks_like_html_landing_page(Some("text/html"), false, None,));
        assert!(looks_like_html_landing_page(
            Some("application/xhtml+xml"),
            false,
            Some(20_000),
        ));
    }

    #[test]
    fn html_with_attachment_is_not_landing_page() {
        assert!(!looks_like_html_landing_page(
            Some("text/html"),
            true,
            Some(5_000_000),
        ));
    }

    #[test]
    fn binary_content_type_is_not_landing_page() {
        assert!(!looks_like_html_landing_page(
            Some("application/octet-stream"),
            false,
            Some(5_000_000),
        ));
        assert!(!looks_like_html_landing_page(
            Some("video/mp4"),
            false,
            None,
        ));
    }

    #[test]
    fn huge_html_is_not_treated_as_landing_page() {
        assert!(!looks_like_html_landing_page(
            Some("text/html"),
            false,
            Some(100 * 1024 * 1024),
        ));
    }

    #[tokio::test]
    async fn invalid_custom_header_returns_input_error() {
        let client = reqwest::Client::new();
        let result = probe_server(
            &client,
            "http://127.0.0.1:1/file.bin",
            &[("bad header".to_string(), "value".to_string())],
        )
        .await;

        assert!(
            matches!(result, Err(ShioError::Config(message)) if message.starts_with("invalid header "))
        );
    }
}
