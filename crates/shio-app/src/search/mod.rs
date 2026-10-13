use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Matcher, Utf32Str};
use shio_core::TorrentFile;

const KB: u64 = 1_000;
const MB: u64 = 1_000_000;
const GB: u64 = 1_000_000_000;

#[derive(Default)]
pub(crate) struct Query {
    pattern: Option<Pattern>,
    kind: TypeFilter,
    size: SizeFilter,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum SizeFilter {
    #[default]
    Any,
    LessThan(u64),
    Between(u64, u64),
    GreaterThan(u64),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum TypeFilter {
    #[default]
    Any,
    Archives,
    Audio,
    Documents,
    Images,
    Software,
    Torrents,
    Videos,
}

impl Query {
    pub(crate) fn parse(input: &str) -> Self {
        let mut text = Vec::new();
        let mut query = Self::default();
        for token in input.split_whitespace() {
            let lower = token.to_lowercase();
            if let Some(value) = lower.strip_prefix("type:")
                && let Some(kind) = parse_type(value)
            {
                query.kind = kind;
            } else if let Some(value) = lower.strip_prefix("size:")
                && let Some(size) = parse_size(value)
            {
                query.size = size;
            } else if lower != "type:" && lower != "size:" {
                text.push(token);
            }
        }
        if !text.is_empty() {
            query.pattern = Some(Pattern::parse(
                &text.join(" "),
                CaseMatching::Ignore,
                Normalization::Smart,
            ));
        }
        query
    }

    pub(crate) fn matches(
        &self,
        matcher: &mut Matcher,
        name: &str,
        url: Option<&str>,
        size: Option<u64>,
        files: &[TorrentFile],
    ) -> bool {
        let kind = self.kind;
        let kind_matches = kind.matches(name)
            || files.iter().any(|file| {
                file.selected
                    && file
                        .path
                        .file_name()
                        .and_then(std::ffi::OsStr::to_str)
                        .is_some_and(|name| kind.matches(name))
            });
        if !kind_matches || !self.size.matches(size) {
            return false;
        }
        let Some(pattern) = &self.pattern else {
            return true;
        };
        let mut buffer = Vec::new();
        [Some(name), url].into_iter().flatten().any(|haystack| {
            pattern
                .score(Utf32Str::new(haystack, &mut buffer), matcher)
                .is_some()
        })
    }
}

impl SizeFilter {
    fn matches(self, size: Option<u64>) -> bool {
        match self {
            Self::Any => true,
            Self::LessThan(max) => size.is_some_and(|s| s < max),
            Self::Between(min, max) => size.is_some_and(|s| s >= min && s < max),
            Self::GreaterThan(min) => size.is_some_and(|s| s > min),
        }
    }
}

impl TypeFilter {
    fn matches(self, filename: &str) -> bool {
        let extensions: &[&str] = match self {
            Self::Any => return true,
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
        };
        filename.rsplit_once('.').is_some_and(|(_, extension)| {
            extensions
                .iter()
                .any(|known| extension.eq_ignore_ascii_case(known))
        })
    }
}

fn parse_type(value: &str) -> Option<TypeFilter> {
    Some(match value {
        "any" | "all" => TypeFilter::Any,
        "archive" | "archives" => TypeFilter::Archives,
        "audio" | "music" => TypeFilter::Audio,
        "document" | "documents" | "docs" => TypeFilter::Documents,
        "image" | "images" => TypeFilter::Images,
        "software" | "program" | "programs" | "app" | "apps" => TypeFilter::Software,
        "torrent" | "torrents" => TypeFilter::Torrents,
        "video" | "videos" => TypeFilter::Videos,
        _ => return None,
    })
}

fn parse_size(value: &str) -> Option<SizeFilter> {
    if value == "any" {
        return Some(SizeFilter::Any);
    }
    if let Some(rest) = value.strip_prefix('<') {
        return parse_amount(rest).map(SizeFilter::LessThan);
    }
    if let Some(rest) = value.strip_prefix('>') {
        return parse_amount(rest).map(SizeFilter::GreaterThan);
    }
    let (low, high) = value.split_once('-')?;
    Some(SizeFilter::Between(parse_amount(low)?, parse_amount(high)?))
}

fn parse_amount(text: &str) -> Option<u64> {
    let (number, unit) = [("gb", GB), ("mb", MB), ("kb", KB), ("b", 1)]
        .into_iter()
        .find_map(|(suffix, unit)| text.strip_suffix(suffix).map(|number| (number, unit)))?;
    number.parse::<u64>().ok()?.checked_mul(unit)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Suggestion {
    pub(crate) label: &'static str,
    pub(crate) value: &'static str,
    pub(crate) hint: &'static str,
}

const KEYS: [Suggestion; 2] = [
    Suggestion {
        label: "type:",
        value: "type:",
        hint: "archives, videos…",
    },
    Suggestion {
        label: "size:",
        value: "size:",
        hint: ">1gb, <100mb…",
    },
];

const TYPES: [&str; 8] = [
    "type:archives",
    "type:audio",
    "type:documents",
    "type:images",
    "type:software",
    "type:torrents",
    "type:videos",
    "type:any",
];

const SIZES: [&str; 7] = [
    "size:<10mb",
    "size:<100mb",
    "size:10mb-100mb",
    "size:100mb-1gb",
    "size:>100mb",
    "size:>1gb",
    "size:any",
];

pub(crate) fn completions(input: &str) -> Vec<Suggestion> {
    if input.trim().is_empty() {
        return KEYS.to_vec();
    }
    let Some(token) = input
        .split_whitespace()
        .last()
        .filter(|_| !input.ends_with(char::is_whitespace))
    else {
        return Vec::new();
    };
    let token = token.to_lowercase();
    let values = |options: &[&'static str], partial: &str| {
        options
            .iter()
            .filter_map(|&value| {
                let (_, label) = value.split_once(':')?;
                label.starts_with(partial).then_some(Suggestion {
                    label,
                    value,
                    hint: "",
                })
            })
            .collect()
    };
    match token.split_once(':') {
        None => KEYS
            .into_iter()
            .filter(|key| key.value.starts_with(&token))
            .collect(),
        Some(("type", partial)) if parse_type(partial).is_none() => values(&TYPES, partial),
        Some(("size", partial)) if parse_size(partial).is_none() => values(&SIZES, partial),
        Some(_) => Vec::new(),
    }
}

pub(crate) fn apply(input: &str, value: &str) -> String {
    let kept = input.trim_end_matches(|c: char| !c.is_whitespace());
    let space = if value.ends_with(':') { "" } else { " " };
    format!("{kept}{value}{space}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_syntax_parses_filters_and_keeps_other_tokens_as_text() {
        let cases = [
            ("", TypeFilter::Any, SizeFilter::Any, false),
            ("ubuntu iso", TypeFilter::Any, SizeFilter::Any, true),
            ("Size:", TypeFilter::Any, SizeFilter::Any, false),
            (
                "TYPE:Archives",
                TypeFilter::Archives,
                SizeFilter::Any,
                false,
            ),
            (
                "type:videos type:any",
                TypeFilter::Any,
                SizeFilter::Any,
                false,
            ),
            (
                "size:<10mb",
                TypeFilter::Any,
                SizeFilter::LessThan(10 * MB),
                false,
            ),
            (
                "size:>1gb",
                TypeFilter::Any,
                SizeFilter::GreaterThan(GB),
                false,
            ),
            (
                "size:10mb-100mb",
                TypeFilter::Any,
                SizeFilter::Between(10 * MB, 100 * MB),
                false,
            ),
            ("type:banana", TypeFilter::Any, SizeFilter::Any, true),
            (
                "size:>99999999999gb",
                TypeFilter::Any,
                SizeFilter::Any,
                true,
            ),
            (
                "size:18446744073709551616b",
                TypeFilter::Any,
                SizeFilter::Any,
                true,
            ),
        ];
        for (input, kind, size, has_text) in cases {
            let query = Query::parse(input);
            assert_eq!(query.kind, kind, "{input}");
            assert_eq!(query.size, size, "{input}");
            assert_eq!(query.pattern.is_some(), has_text, "{input}");
        }
    }

    #[test]
    fn size_filter_bounds_are_half_open_and_unknown_sizes_only_match_any() {
        let cases = [
            (SizeFilter::Any, None, true),
            (SizeFilter::LessThan(10 * MB), None, false),
            (SizeFilter::LessThan(10 * MB), Some(10 * MB), false),
            (SizeFilter::Between(10 * MB, 100 * MB), Some(10 * MB), true),
            (
                SizeFilter::Between(10 * MB, 100 * MB),
                Some(100 * MB),
                false,
            ),
            (SizeFilter::GreaterThan(GB), Some(GB), false),
            (SizeFilter::GreaterThan(GB), Some(GB + 1), true),
        ];
        for (filter, size, expected) in cases {
            assert_eq!(filter.matches(size), expected, "{filter:?} {size:?}");
        }
    }

    #[test]
    fn query_matches_name_url_and_selected_torrent_files() {
        let mut matcher = Matcher::default();
        let files = [TorrentFile {
            path: "release/movie.mkv".into(),
            size: 1,
            selected: true,
        }];

        assert!(Query::parse("ubu").matches(&mut matcher, "ubuntu.iso", None, None, &[]));
        assert!(Query::parse("mirror").matches(
            &mut matcher,
            "file.bin",
            Some("https://mirror.example/file.bin"),
            None,
            &[]
        ));
        assert!(!Query::parse("xyzzy").matches(&mut matcher, "movie.mp4", None, None, &[]));
        assert!(Query::parse("type:video").matches(&mut matcher, "release", None, None, &files));
        assert!(!Query::parse("type:video").matches(&mut matcher, "notes.txt", None, None, &[]));
    }

    #[test]
    fn completions_follow_the_last_token_and_only_offer_filters_the_parser_accepts() {
        let values = |input: &str| -> Vec<&str> {
            completions(input)
                .into_iter()
                .map(|suggestion| suggestion.value)
                .collect()
        };
        assert_eq!(values(""), ["type:", "size:"]);
        assert_eq!(values("ubuntu s"), ["size:"]);
        assert_eq!(
            values("type:a"),
            ["type:archives", "type:audio", "type:any"]
        );
        assert_eq!(values("SIZE:<1"), ["size:<10mb", "size:<100mb"]);
        for done in ["ubuntu", "ubuntu ", "type:videos", "size:>1gb", "name:x"] {
            assert!(values(done).is_empty(), "{done}");
        }
        for value in values("type:").into_iter().chain(values("size:")) {
            let query = Query::parse(value);
            assert!(query.pattern.is_none(), "{value} is not a filter");
        }
        assert_eq!(apply("ubuntu ty", "type:"), "ubuntu type:");
        assert_eq!(
            apply("ubuntu type:vi", "type:videos"),
            "ubuntu type:videos "
        );
    }
}
