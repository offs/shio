use std::fmt;

const KB: u64 = 1_000;
const MB: u64 = 1_000_000;
const GB: u64 = 1_000_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SizeFilter {
    Any,
    LessThan(u64),
    Between(u64, u64),
    GreaterThan(u64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TypeFilter {
    Archives,
    Audio,
    Documents,
    Images,
    Software,
    Torrents,
    Videos,
}

impl TypeFilter {
    const fn extensions(self) -> &'static [&'static str] {
        match self {
            Self::Archives => &["zip", "rar", "7z", "tar", "gz", "bz2", "xz", "zst", "lz4"],
            Self::Audio => &["mp3", "flac", "wav", "aac", "ogg", "wma", "m4a", "opus"],
            Self::Documents => &[
                "pdf", "doc", "docx", "txt", "xlsx", "pptx", "odt", "rtf", "csv",
            ],
            Self::Images => &["png", "jpg", "jpeg", "gif", "webp", "bmp", "ico", "tiff"],
            Self::Software => &[
                "exe", "msi", "dmg", "deb", "rpm", "appimage", "pkg", "snap", "flatpak",
            ],
            Self::Torrents => &["torrent"],
            Self::Videos => &[
                "mp4", "mkv", "avi", "mov", "wmv", "flv", "webm", "m4v", "ts",
            ],
        }
    }

    pub(crate) fn matches_download(self, download: &shio_core::Download) -> bool {
        filename_matches_type(&download.filename, self)
            || download.torrent().is_some_and(|torrent| {
                torrent.files.iter().any(|file| {
                    file.selected
                        && file
                            .path
                            .file_name()
                            .and_then(std::ffi::OsStr::to_str)
                            .is_some_and(|name| filename_matches_type(name, self))
                })
            })
    }
}

fn filename_matches_type(filename: &str, filter: TypeFilter) -> bool {
    filename
        .rsplit('.')
        .next()
        .map(str::to_ascii_lowercase)
        .is_some_and(|ext| filter.extensions().contains(&ext.as_str()))
}

impl SizeFilter {
    pub(crate) fn matches(self, size: Option<u64>) -> bool {
        match self {
            Self::Any => true,
            Self::LessThan(max) => size.is_some_and(|s| s < max),
            Self::Between(min, max) => size.is_some_and(|s| s >= min && s < max),
            Self::GreaterThan(min) => size.is_some_and(|s| s > min),
        }
    }
}

impl fmt::Display for SizeFilter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Any => write!(f, "any"),
            Self::LessThan(n) if n == 10 * MB => write!(f, "< 10 MB"),
            Self::LessThan(n) if n == 100 * MB => write!(f, "< 100 MB"),
            Self::LessThan(n) if n == GB => write!(f, "< 1 GB"),
            Self::LessThan(n) => write!(f, "< {}", format_bytes(n)),
            Self::Between(a, b) if a == 10 * MB && b == 100 * MB => write!(f, "10 – 100 MB"),
            Self::Between(a, b) if a == 100 * MB && b == GB => write!(f, "100 MB – 1 GB"),
            Self::Between(a, b) => write!(f, "{} – {}", format_bytes(a), format_bytes(b)),
            Self::GreaterThan(n) if n == 10 * MB => write!(f, "> 10 MB"),
            Self::GreaterThan(n) if n == 100 * MB => write!(f, "> 100 MB"),
            Self::GreaterThan(n) if n == GB => write!(f, "> 1 GB"),
            Self::GreaterThan(n) => write!(f, "> {}", format_bytes(n)),
        }
    }
}

fn format_bytes(n: u64) -> String {
    if n >= GB {
        format!("{} GB", n / GB)
    } else if n >= MB {
        format!("{} MB", n / MB)
    } else if n >= KB {
        format!("{} KB", n / KB)
    } else {
        format!("{n} B")
    }
}

use nucleo_matcher::{
    Config, Matcher,
    pattern::{CaseMatching, Normalization, Pattern},
};

#[derive(Debug, Clone)]
pub(crate) struct MatchResult {
    pub(crate) filename_indices: Vec<u32>,
}

pub(crate) fn make_matcher() -> Matcher {
    Matcher::new(Config::DEFAULT)
}

pub(crate) fn match_download(
    matcher: &mut Matcher,
    dl: &shio_core::Download,
    pattern_str: &str,
) -> Option<MatchResult> {
    if pattern_str.is_empty() {
        return Some(MatchResult {
            filename_indices: Vec::new(),
        });
    }

    let pattern = Pattern::parse(pattern_str, CaseMatching::Ignore, Normalization::Smart);

    let mut fn_buf = Vec::new();
    let fn_hay = nucleo_matcher::Utf32Str::new(&dl.filename, &mut fn_buf);
    let mut fn_indices = Vec::new();
    let fn_score = pattern.indices(fn_hay, matcher, &mut fn_indices);

    if fn_score.is_some() {
        return Some(MatchResult {
            filename_indices: fn_indices,
        });
    }

    let mut url_buf = Vec::new();
    let url_str = dl.url().unwrap_or("");
    let url_hay = nucleo_matcher::Utf32Str::new(url_str, &mut url_buf);
    let url_score = pattern.score(url_hay, matcher);

    url_score.map(|_| MatchResult {
        filename_indices: Vec::new(),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SearchQuery {
    pub(crate) text: String,
    pub(crate) type_filter: Option<TypeFilter>,
    pub(crate) size_filter: SizeFilter,
}

impl SearchQuery {
    pub(crate) fn parse(input: &str) -> Self {
        let mut text_tokens: Vec<&str> = Vec::new();
        let mut type_filter = None;
        let mut size_filter = SizeFilter::Any;

        for token in input.split_whitespace() {
            let lower = token.to_lowercase();
            if let Some(rest) = lower.strip_prefix("type:") {
                if let Some(parsed) = parse_type_value(rest) {
                    type_filter = parsed;
                    continue;
                }
            } else if let Some(rest) = lower.strip_prefix("size:")
                && let Some(sf) = parse_size_value(rest)
            {
                size_filter = sf;
                continue;
            }
            text_tokens.push(token);
        }

        Self {
            text: text_tokens.join(" "),
            type_filter,
            size_filter,
        }
    }
}

#[expect(
    clippy::option_option,
    reason = "nested option distinguishes an explicit any type filter from an unrecognized token"
)]
fn parse_type_value(value: &str) -> Option<Option<TypeFilter>> {
    match value {
        "any" | "all" => Some(None),
        "archive" | "archives" => Some(Some(TypeFilter::Archives)),
        "audio" | "music" => Some(Some(TypeFilter::Audio)),
        "document" | "documents" | "docs" => Some(Some(TypeFilter::Documents)),
        "image" | "images" => Some(Some(TypeFilter::Images)),
        "software" | "program" | "programs" | "app" | "apps" => Some(Some(TypeFilter::Software)),
        "torrent" | "torrents" => Some(Some(TypeFilter::Torrents)),
        "video" | "videos" => Some(Some(TypeFilter::Videos)),
        _ => None,
    }
}

fn parse_size_value(value: &str) -> Option<SizeFilter> {
    if value == "any" {
        return Some(SizeFilter::Any);
    }
    if let Some(rest) = value.strip_prefix('<') {
        return parse_size_amount(rest).map(SizeFilter::LessThan);
    }
    if let Some(rest) = value.strip_prefix('>') {
        return parse_size_amount(rest).map(SizeFilter::GreaterThan);
    }
    if let Some((lo, hi)) = value.split_once('-')
        && let (Some(a), Some(b)) = (parse_size_amount(lo), parse_size_amount(hi))
    {
        return Some(SizeFilter::Between(a, b));
    }
    None
}

fn parse_size_amount(s: &str) -> Option<u64> {
    let s = s.trim();
    let (num_str, unit) = if let Some(n) = s.strip_suffix("gb") {
        (n, GB)
    } else if let Some(n) = s.strip_suffix("mb") {
        (n, MB)
    } else if let Some(n) = s.strip_suffix("kb") {
        (n, KB)
    } else if let Some(n) = s.strip_suffix('b') {
        (n, 1)
    } else {
        return None;
    };
    num_str.parse::<u64>().ok().map(|n| n * unit)
}

#[derive(Debug, Clone)]
pub(crate) struct Suggestion {
    pub(crate) label: &'static str,
    pub(crate) value: &'static str,
}

const TYPE_OPTIONS: &[(&str, &str)] = &[
    ("archives", "type:archives"),
    ("audio", "type:audio"),
    ("documents", "type:documents"),
    ("images", "type:images"),
    ("software", "type:software"),
    ("torrents", "type:torrents"),
    ("videos", "type:videos"),
    ("any", "type:any"),
];

const SIZE_OPTIONS: &[(&str, &str)] = &[
    ("<10mb", "size:<10mb"),
    ("<100mb", "size:<100mb"),
    ("10mb-100mb", "size:10mb-100mb"),
    ("100mb-1gb", "size:100mb-1gb"),
    (">100mb", "size:>100mb"),
    (">1gb", "size:>1gb"),
    ("any", "size:any"),
];

pub(crate) fn completions(input: &str) -> Vec<Suggestion> {
    let last_token = match input.split_whitespace().last() {
        Some(t) => t.to_lowercase(),
        None => return Vec::new(),
    };

    if !last_token.contains(':') {
        let mut out = Vec::new();
        if "type:".starts_with(&last_token) {
            out.push(Suggestion {
                label: "type:",
                value: "type:",
            });
        }
        if "size:".starts_with(&last_token) {
            out.push(Suggestion {
                label: "size:",
                value: "size:",
            });
        }
        return out;
    }

    if let Some(partial) = last_token.strip_prefix("type:") {
        if parse_type_value(partial).is_some() {
            return Vec::new();
        }
        return TYPE_OPTIONS
            .iter()
            .filter(|(label, _)| partial.is_empty() || label.starts_with(partial))
            .map(|(label, value)| Suggestion { label, value })
            .collect();
    }

    if let Some(partial) = last_token.strip_prefix("size:") {
        if parse_size_value(partial).is_some() {
            return Vec::new();
        }
        return SIZE_OPTIONS
            .iter()
            .filter(|(label, _)| partial.is_empty() || label.starts_with(partial))
            .map(|(label, value)| Suggestion { label, value })
            .collect();
    }

    Vec::new()
}

pub(crate) fn apply_suggestion(input: &str, suggestion_value: &str) -> String {
    let trimmed = input.trim_end();
    let prefix = match trimmed.rfind(char::is_whitespace) {
        Some(pos) => &trimmed[..=pos],
        None => "",
    };
    format!("{prefix}{suggestion_value} ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_filter_matches_expected_boundaries() {
        let cases = [
            (SizeFilter::Any, None, true),
            (SizeFilter::Any, Some(10 * GB), true),
            (SizeFilter::LessThan(10 * MB), None, false),
            (SizeFilter::LessThan(10 * MB), Some(5 * MB), true),
            (SizeFilter::LessThan(10 * MB), Some(10 * MB), false),
            (SizeFilter::Between(10 * MB, 100 * MB), Some(9 * MB), false),
            (SizeFilter::Between(10 * MB, 100 * MB), Some(10 * MB), true),
            (
                SizeFilter::Between(10 * MB, 100 * MB),
                Some(100 * MB),
                false,
            ),
            (SizeFilter::GreaterThan(GB), Some(GB), false),
            (SizeFilter::GreaterThan(GB), Some(GB + 1), true),
        ];

        for (filter, value, expected) in cases {
            assert_eq!(filter.matches(value), expected, "{filter:?} {value:?}");
        }
    }

    #[test]
    fn search_query_parse_handles_text_type_size_and_unknown_tokens() {
        let cases = [
            ("", "", None, SizeFilter::Any),
            ("ubuntu iso", "ubuntu iso", None, SizeFilter::Any),
            (
                "type:archives",
                "",
                Some(TypeFilter::Archives),
                SizeFilter::Any,
            ),
            ("size:<10mb", "", None, SizeFilter::LessThan(10 * MB)),
            ("size:>1gb", "", None, SizeFilter::GreaterThan(GB)),
            (
                "size:10mb-100mb",
                "",
                None,
                SizeFilter::Between(10 * MB, 100 * MB),
            ),
            (
                "type:banana extra",
                "type:banana extra",
                None,
                SizeFilter::Any,
            ),
        ];

        for (input, text, type_filter, size_filter) in cases {
            let q = SearchQuery::parse(input);
            assert_eq!(q.text, text, "{input}");
            assert_eq!(q.type_filter, type_filter, "{input}");
            assert_eq!(q.size_filter, size_filter, "{input}");
        }
    }

    #[test]
    fn fuzzy_match_subsequence_scores_and_indices() {
        use nucleo_matcher::{
            Matcher,
            pattern::{CaseMatching, Normalization, Pattern},
        };

        let mut matcher = Matcher::new(nucleo_matcher::Config::DEFAULT);
        let pattern = Pattern::parse("ubu", CaseMatching::Ignore, Normalization::Smart);
        let mut buf = Vec::new();
        let haystack = nucleo_matcher::Utf32Str::new("ubuntu-24.04.iso", &mut buf);
        let mut indices = Vec::new();
        let score = pattern.indices(haystack, &mut matcher, &mut indices);
        assert!(score.is_some(), "should match");
        assert!(!indices.is_empty(), "should have indices");
        assert_eq!(&indices[..3], &[0, 1, 2], "should match leading 'ubu'");
    }

    #[test]
    fn query_matches_filename_returns_indices() {
        use shio_core::Download;
        use std::path::PathBuf;

        let mut dl = Download::new("https://example.com/x".to_string(), PathBuf::from("/tmp"));
        dl.filename = "ubuntu-24.04.iso".to_string();

        let mut matcher = make_matcher();
        let result = match_download(&mut matcher, &dl, "ubu");
        assert!(result.is_some(), "should match");
        let res = result.unwrap();
        assert_eq!(&res.filename_indices[..3], &[0, 1, 2]);
    }

    #[test]
    fn query_no_match_returns_none() {
        use shio_core::Download;
        use std::path::PathBuf;

        let mut dl = Download::new("https://example.com/x".to_string(), PathBuf::from("/tmp"));
        dl.filename = "movie.mp4".to_string();

        let mut matcher = make_matcher();
        let result = match_download(&mut matcher, &dl, "xyzzy");
        assert!(result.is_none());
    }
}
