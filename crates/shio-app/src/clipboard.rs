pub(crate) fn is_downloadable_url(text: &str) -> bool {
    let trimmed = text.trim();

    if !trimmed.starts_with("http://") && !trimmed.starts_with("https://") {
        return false;
    }

    let Ok(parsed) = url::Url::parse(trimmed) else {
        return false;
    };

    if parsed.host_str().is_none() {
        return false;
    }

    let path = parsed.path();
    if path.is_empty() || path == "/" {
        return false;
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_urls() {
        assert!(is_downloadable_url("https://example.com/file.zip"));
        assert!(is_downloadable_url("http://mirror.example.org/linux.iso"));
        assert!(is_downloadable_url(
            "https://cdn.example.com/path/to/video.mp4?token=abc"
        ));
    }

    #[test]
    fn test_invalid_urls() {
        assert!(!is_downloadable_url("not a url"));
        assert!(!is_downloadable_url("ftp://example.com/file.zip"));
        assert!(!is_downloadable_url("https://example.com/"));
        assert!(!is_downloadable_url("https://example.com"));
    }
}
