use std::sync::OnceLock;

use regex::Regex;

const RESERVED_DEVICE_NAMES: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchivePart {
    pub base_name: String,
    pub part_number: u32,
}

pub fn parse_archive_part(filename: &str) -> Option<ArchivePart> {
    let lower = filename.to_ascii_lowercase();
    let marker = ".part";
    let suffix = ".rar";
    if !lower.ends_with(suffix) {
        return None;
    }
    let suffix_start = lower.len().checked_sub(suffix.len())?;
    let before_suffix = &lower[..suffix_start];
    let marker_start = before_suffix.rfind(marker)?;
    let digits = &before_suffix[marker_start + marker.len()..];
    if digits.len() < 2 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let part_number = digits.parse().ok()?;
    if part_number == 0 {
        return None;
    }
    let base = filename.get(..marker_start)?;
    let base_name = sanitize_filename(base);
    if base_name.is_empty() || base_name == "download" {
        return None;
    }
    Some(ArchivePart {
        base_name,
        part_number,
    })
}

fn multipart_patterns() -> &'static [Regex] {
    static PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        vec![
            Regex::new(r"(?i)\.part\d+\.rar$").unwrap(),
            Regex::new(r"(?i)\.7z\.\d+$").unwrap(),
            Regex::new(r"(?i)\.zip\.\d+$").unwrap(),
        ]
    })
}

pub fn extract_filename(url: &str, headers: Option<&reqwest::header::HeaderMap>) -> String {
    if let Some(name) = headers
        .and_then(|headers| headers.get(reqwest::header::CONTENT_DISPOSITION))
        .and_then(|cd| cd.to_str().ok())
        .and_then(parse_content_disposition)
    {
        return sanitize_filename(&name);
    }

    if let Ok(parsed) = url::Url::parse(url) {
        let path_name = parsed
            .path_segments()
            .and_then(|mut s| s.next_back())
            .map(percent_decode)
            .filter(|s| !s.is_empty() && s != "/");

        let fragment_name = parsed
            .fragment()
            .map(percent_decode)
            .filter(|s| !s.is_empty());

        let prefer_fragment = match (&path_name, &fragment_name) {
            (Some(p), Some(f)) => !p.contains('.') && f.contains('.'),
            (None, Some(f)) => f.contains('.'),
            _ => false,
        };

        if prefer_fragment && let Some(name) = fragment_name {
            return sanitize_filename(&name);
        }

        if let Some(name) = path_name {
            return sanitize_filename(&name);
        }
    }

    "download".to_string()
}

pub fn sanitize_filename(name: &str) -> String {
    let cleaned: String = name.replace(['/', '\\'], "_").replace("..", "_");

    let mut result: String = cleaned
        .chars()
        .map(|c| {
            if c.is_control() || "<>:\"|?*".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();

    while result.ends_with('.') || result.ends_with(' ') {
        result.pop();
    }
    let result_trim_start = result.trim_start();
    if result_trim_start.len() != result.len() {
        result = result_trim_start.to_string();
    }

    let stem = result.split('.').next().unwrap_or("").to_uppercase();
    if RESERVED_DEVICE_NAMES.contains(&stem.as_str()) {
        result = format!("_{result}");
    }

    if result.is_empty() {
        return "download".to_string();
    }

    if result.chars().count() > 240 {
        let ext = result.rsplit_once('.').map_or("", |(_, e)| e);
        if !ext.is_empty() && ext.len() < 16 {
            let keep = 240usize.saturating_sub(ext.len() + 1);
            let head: String = result.chars().take(keep).collect();
            result = format!("{head}.{ext}");
        } else {
            result = result.chars().take(240).collect();
        }
    }

    result
}

pub fn subfolder_value(create_subfolder: bool, subfolder_name: &str) -> Option<String> {
    if !create_subfolder {
        return None;
    }

    let trimmed = subfolder_name.trim();
    if trimmed.is_empty() {
        return None;
    }

    let sanitized = sanitize_filename(trimmed);
    (!sanitized.is_empty()).then_some(sanitized)
}

pub fn suggest_folder_name(filenames: &[&str]) -> Option<String> {
    if filenames.is_empty() {
        return None;
    }

    let stems: Vec<String> = filenames
        .iter()
        .map(|name| strip_multipart_and_extension(name))
        .collect();

    let prefix = longest_common_prefix(&stems);
    let trimmed =
        prefix.trim_end_matches(|c: char| c == '.' || c == '_' || c == '-' || c.is_whitespace());

    if trimmed.len() < 3 {
        return None;
    }
    if trimmed.eq_ignore_ascii_case("download") {
        return None;
    }
    if trimmed.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }

    let sanitized = sanitize_filename(trimmed);
    if sanitized.len() < 3 || sanitized == "download" {
        return None;
    }
    Some(sanitized)
}

pub fn is_archive_filename(name: &str) -> bool {
    multipart_patterns()
        .iter()
        .any(|pattern| pattern.is_match(name))
        || crate::extract::Format::of(std::path::Path::new(name)).is_some()
}

pub(crate) fn has_unsupported_archive_extension(name: &str) -> bool {
    let lower = name.to_lowercase();
    if lower.ends_with(".tar.gz") || lower.ends_with(".tar.zst") {
        return false;
    }
    if lower.ends_with(".tar.bz2") || lower.ends_with(".tar.xz") {
        return true;
    }
    let ext = lower.rsplit('.').next().unwrap_or("");
    matches!(ext, "gz" | "bz2" | "xz" | "zst" | "lz4")
}

fn strip_multipart_and_extension(name: &str) -> String {
    let mut stripped: String = name.to_string();
    for pat in multipart_patterns() {
        if let Some(m) = pat.find(&stripped) {
            stripped.truncate(m.start());
            break;
        }
    }
    if let Some(dot) = stripped.rfind('.') {
        stripped.truncate(dot);
    }
    stripped
}

fn longest_common_prefix(strings: &[String]) -> String {
    let Some(first) = strings.first() else {
        return String::new();
    };
    let mut end = first.chars().count();
    for s in &strings[1..] {
        let common = first
            .chars()
            .zip(s.chars())
            .take_while(|(a, b)| a == b)
            .count();
        end = end.min(common);
        if end == 0 {
            break;
        }
    }
    first.chars().take(end).collect()
}

fn parse_content_disposition(value: &str) -> Option<String> {
    if let Some(pos) = value.find("filename*=") {
        let rest = &value[pos + 10..];
        if let Some(quote_pos) = rest.find('\'') {
            let after_quotes = &rest[quote_pos + 1..];
            if let Some(quote_pos2) = after_quotes.find('\'') {
                let encoded = after_quotes[quote_pos2 + 1..]
                    .trim_end_matches(';')
                    .trim()
                    .trim_matches('"');
                return Some(percent_decode(encoded));
            }
        }
    }

    if let Some(pos) = value.find("filename=") {
        let rest = &value[pos + 9..];
        let name = rest
            .trim()
            .trim_start_matches('"')
            .split('"')
            .next()
            .unwrap_or_else(|| rest.trim())
            .trim_end_matches(';')
            .trim();
        if !name.is_empty() {
            return Some(name.to_string());
        }
    }

    None
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if let [b'%', hi, lo, ..] = bytes[i..]
            && let (Some(h), Some(l)) = (hex_digit(hi), hex_digit(lo))
        {
            out.push(h << 4 | l);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

const fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::{CONTENT_DISPOSITION, HeaderMap, HeaderValue};

    #[test]
    fn sanitize_filename_handles_safety_and_preservation_cases() {
        for input in ["../etc/passwd", "..\\windows\\system32"] {
            let out = sanitize_filename(input);
            assert!(!out.contains(".."), "got {out}");
            assert!(!out.contains('/'), "got {out}");
            assert!(!out.contains('\\'), "got {out}");
        }

        let control = sanitize_filename("a\0b\tc");
        assert!(!control.contains('\0'));
        assert!(!control.contains('\t'));

        for reserved in ["CON", "NUL", "PRN", "AUX", "COM1", "LPT1", "con.txt"] {
            let out = sanitize_filename(reserved);
            assert!(out.starts_with('_'), "reserved name leaked: {out}");
        }

        for (input, expected) in [
            ("ubuntu-24.04.iso", "ubuntu-24.04.iso"),
            ("a file.txt", "a file.txt"),
            ("", "download"),
        ] {
            assert_eq!(sanitize_filename(input), expected);
        }

        let invalid = sanitize_filename(r#"bad<>:"|?*.txt"#);
        for c in ['<', '>', ':', '"', '|', '?', '*'] {
            assert!(!invalid.contains(c), "char {c} leaked into {invalid}");
        }

        let trimmed = sanitize_filename("name.  ");
        assert!(!trimmed.ends_with(' '));
        assert!(!trimmed.ends_with('.'));
    }

    #[test]
    fn subfolder_value_handles_enabled_disabled_and_blank_names() {
        assert_eq!(subfolder_value(false, "archive"), None);
        assert_eq!(subfolder_value(true, "   "), None);
        assert_eq!(
            subfolder_value(true, "  bad/name  ").as_deref(),
            Some("bad_name")
        );
    }

    #[test]
    fn sanitize_filename_caps_length_and_preserves_extension() {
        let long = "a".repeat(500);
        let out = sanitize_filename(&long);
        assert!(
            out.chars().count() <= 240,
            "got {} chars",
            out.chars().count()
        );

        let long = format!("{}.pdf", "a".repeat(500));
        let out = sanitize_filename(&long);
        assert!(out.chars().count() <= 240);
        assert!(
            std::path::Path::new(&out)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("pdf")),
            "extension lost: {out}"
        );
    }

    #[test]
    fn extract_filename_uses_headers_url_fragments_and_fallbacks() {
        for header in [
            r#"attachment; filename="report.pdf""#,
            "attachment; filename=report.pdf",
            "attachment; filename*=UTF-8''report.pdf",
        ] {
            let mut headers = HeaderMap::new();
            headers.insert(CONTENT_DISPOSITION, HeaderValue::from_static(header));
            assert_eq!(
                extract_filename("https://example.com/", Some(&headers)),
                "report.pdf"
            );
        }

        for (url, expected) in [
            ("https://example.com/path/file.zip", "file.zip"),
            ("https://example.com/path/my%20file.zip", "my file.zip"),
            (
                "https://example.com/abc123#archive.part01.rar",
                "archive.part01.rar",
            ),
            ("https://example.com/file.zip#anchor", "file.zip"),
            ("https://example.com/", "download"),
            ("not a url", "download"),
        ] {
            assert_eq!(extract_filename(url, None), expected);
        }
    }

    #[test]
    fn parse_archive_part_accepts_numbered_rar_parts_only() {
        assert_eq!(
            parse_archive_part("Game.part01.rar"),
            Some(ArchivePart {
                base_name: "Game".to_string(),
                part_number: 1,
            })
        );
        assert_eq!(
            parse_archive_part("Game.part001.rar"),
            Some(ArchivePart {
                base_name: "Game".to_string(),
                part_number: 1,
            })
        );
        assert_eq!(parse_archive_part("Game.rar"), None);
        assert_eq!(parse_archive_part("Game.part01.zip"), None);
        assert_eq!(parse_archive_part("Game.part00.rar"), None);
    }

    #[test]
    fn suggest_folder_name_handles_common_archive_sets_and_fallbacks() {
        for (names, expected) in [
            (&["foo.part01.rar", "foo.part02.rar"][..], Some("foo")),
            (&["foo.part07.rar", "foo.part08.rar"][..], Some("foo")),
            (&["archive.7z.001", "archive.7z.002"][..], Some("archive")),
            (&["data.r00", "data.r01", "data.rar"][..], Some("data")),
            (&["movie.mp4"][..], Some("movie")),
            (&["a.txt"][..], None),
            (&["alpha.zip", "beta.zip"][..], None),
            (&["foo_-_.part01.rar", "foo_-_.part02.rar"][..], Some("foo")),
        ] {
            assert_eq!(
                suggest_folder_name(names),
                expected.map(str::to_string),
                "names: {names:?}"
            );
        }
    }

    #[test]
    fn archive_filename_detection_handles_known_archive_extensions() {
        for name in ["foo.zip", "foo.rar", "foo.7z", "foo.tar.gz", "foo.tar.zst"] {
            assert!(is_archive_filename(name), "{name}");
        }
        for name in [
            "video.mp4",
            "doc.pdf",
            "song.mp3",
            "foo.tar.bz2",
            "foo.tar.xz",
            "foo.gz",
            "foo.lz4",
        ] {
            assert!(!is_archive_filename(name), "{name}");
        }
    }
}
