use crate::path::{MAX_COMPONENT_BYTES, is_forbidden_char, is_reserved_name};
use std::borrow::Cow;

pub(crate) const FALLBACK_FILENAME: &str = "download";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchivePart {
    pub base_name: String,
    pub part_number: u32,
}

pub fn parse_archive_part(filename: &str) -> Option<ArchivePart> {
    let (base, digits) = rar_part(filename)?;
    let part_number = digits.parse().ok().filter(|&number| number > 0)?;
    let base_name = sanitize_filename(base);
    (base_name != FALLBACK_FILENAME).then_some(ArchivePart {
        base_name,
        part_number,
    })
}

/// `base.partNN.rar` → (`base`, `NN`).
pub(crate) fn rar_part(name: &str) -> Option<(&str, &str)> {
    let stem = strip_suffix_ignore_ascii_case(name, ".rar")?;
    let (base, part) = stem.rsplit_once('.')?;
    let digits = part
        .get(..4)
        .filter(|marker| marker.eq_ignore_ascii_case("part"))
        .and_then(|_| part.get(4..))?;
    is_number(digits).then_some((base, digits))
}

/// `base.7z.NNN` → (`base`, `NNN`).
pub(crate) fn sevenz_volume(name: &str) -> Option<(&str, &str)> {
    let (stem, digits) = name.rsplit_once('.')?;
    let base = strip_suffix_ignore_ascii_case(stem, ".7z")?;
    is_number(digits).then_some((base, digits))
}

pub(crate) fn strip_suffix_ignore_ascii_case<'a>(value: &'a str, suffix: &str) -> Option<&'a str> {
    let split = value.len().checked_sub(suffix.len())?;
    value
        .get(split..)?
        .eq_ignore_ascii_case(suffix)
        .then(|| &value[..split])
}

fn is_number(digits: &str) -> bool {
    !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
}

/// Direct multi-file hosts put the real name in a `#name.ext` fragment when the path has none.
pub fn extract_filename(url: &str) -> String {
    let name = url::Url::parse(url).ok().and_then(|url| {
        let path = path_filename(&url);
        let fragment = url
            .fragment()
            .map(decode_lossy)
            .filter(|fragment| fragment.contains('.'));
        match (path, fragment) {
            (Some(path), Some(fragment)) if !path.contains('.') => Some(fragment),
            (None, fragment) => fragment,
            (path, _) => path,
        }
    });
    name.map_or_else(
        || FALLBACK_FILENAME.to_owned(),
        |name| sanitize_filename(&name),
    )
}

pub(crate) fn path_filename(url: &url::Url) -> Option<String> {
    url.path_segments()?
        .next_back()
        .map(decode_lossy)
        .filter(|name| !name.is_empty())
}

pub(crate) fn fragment_name(url: &url::Url) -> Option<String> {
    let fragment = decode_lossy(url.fragment()?);
    let fragment = fragment.trim();
    (!fragment.is_empty()).then(|| sanitize_filename(fragment))
}

pub fn sanitize_filename(name: &str) -> String {
    let mapped: String = name
        .replace("..", "_")
        .chars()
        .map(|c| {
            if c == '/' || is_forbidden_char(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let mut name = truncate_keeping_extension(trim(&mapped));
    if name.is_empty() {
        return FALLBACK_FILENAME.to_owned();
    }
    if is_reserved_name(&name) {
        name.insert(0, '_');
        name = trim(truncate_utf8(&name, MAX_COMPONENT_BYTES)).to_owned();
    }
    name
}

fn trim(name: &str) -> &str {
    name.trim_start().trim_end_matches(['.', ' '])
}

fn truncate_keeping_extension(name: &str) -> String {
    if name.len() <= MAX_COMPONENT_BYTES {
        return name.to_owned();
    }
    match name.rsplit_once('.') {
        Some((head, ext)) if !ext.is_empty() && ext.len() < 16 => {
            let head = truncate_utf8(head, MAX_COMPONENT_BYTES - ext.len() - 1);
            format!("{}.{ext}", head.trim_end_matches('.'))
        },
        _ => trim(truncate_utf8(name, MAX_COMPONENT_BYTES)).to_owned(),
    }
}

fn truncate_utf8(value: &str, max_bytes: usize) -> &str {
    let mut end = value.len().min(max_bytes);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

pub fn subfolder_value(create_subfolder: bool, subfolder_name: &str) -> Option<String> {
    let trimmed = subfolder_name.trim();
    (create_subfolder && !trimmed.is_empty()).then(|| sanitize_filename(trimmed))
}

pub fn suggest_folder_name(filenames: &[&str]) -> Option<String> {
    let (first, rest) = filenames.split_first()?;
    let first = archive_stem(first);
    let common = rest.iter().fold(first.len(), |end, name| {
        first[..end]
            .char_indices()
            .zip(archive_stem(name).chars())
            .find(|((_, a), b)| a != b)
            .map_or_else(
                || end.min(archive_stem(name).len()),
                |((index, _), _)| index,
            )
    });
    let prefix = release_title(trim_release_suffix(&first[..common]));
    if prefix.len() < 3 || prefix.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let name = sanitize_filename(&prefix);
    (name.len() >= 3 && !name.eq_ignore_ascii_case(FALLBACK_FILENAME)).then_some(name)
}

pub(crate) fn release_name(stem: &str) -> String {
    sanitize_filename(&release_title(trim_release_suffix(stem)))
}

fn trim_release_suffix(name: &str) -> &str {
    name.trim_end_matches(|c: char| matches!(c, '.' | '_' | '-') || c.is_whitespace())
}

fn release_title(prefix: &str) -> Cow<'_, str> {
    let mut dropped_tag = false;
    let mut segments = Vec::new();
    for (index, segment) in prefix.split("--").enumerate() {
        let segment =
            segment.trim_matches(|c: char| matches!(c, '_' | '-' | '.') || c.is_whitespace());
        if segment.is_empty() {
            continue;
        }
        if index > 0 && is_site_tag(segment) {
            dropped_tag = true;
        } else {
            segments.push(segment);
        }
    }
    if segments.len() == 1 && !dropped_tag {
        return Cow::Borrowed(prefix);
    }
    let mut title = segments.join(" - ");
    if dropped_tag && !title.contains(' ') {
        title = title.replace('_', " ");
    }
    Cow::Owned(title.split_whitespace().collect::<Vec<_>>().join(" "))
}

fn is_site_tag(segment: &str) -> bool {
    segment.rsplit_once('.').is_some_and(|(head, tld)| {
        !head.is_empty()
            && (2..=24).contains(&tld.len())
            && tld.bytes().all(|b| b.is_ascii_lowercase())
    }) && segment
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.'))
}

fn archive_stem(name: &str) -> &str {
    rar_part(name).or_else(|| sevenz_volume(name)).map_or_else(
        || name.rsplit_once('.').map_or(name, |(stem, _)| stem),
        |(base, _)| base,
    )
}

pub(crate) struct ContentDisposition {
    pub(crate) attachment: bool,
    pub(crate) filename: Option<String>,
}

/// RFC 6266 `Content-Disposition`; `filename*` (RFC 8187) wins over `filename`.
pub(crate) fn parse_content_disposition(value: &str) -> ContentDisposition {
    let parts = split_parameters(value);
    let attachment = parts
        .first()
        .is_some_and(|kind| kind.trim().eq_ignore_ascii_case("attachment"));
    let mut plain = None;
    let mut extended = None;
    for (name, value) in parts.iter().skip(1).filter_map(|part| part.split_once('=')) {
        let (name, value) = (name.trim(), value.trim());
        if name.eq_ignore_ascii_case("filename*") {
            extended = extended.or_else(|| decode_extended_value(value.trim_matches('"')));
        } else if name.eq_ignore_ascii_case("filename") {
            plain = plain.or_else(|| Some(unquote(value)));
        }
    }
    ContentDisposition {
        attachment,
        filename: extended
            .or(plain)
            .filter(|name| !name.trim().is_empty())
            .map(|name| sanitize_filename(&name)),
    }
}

fn split_parameters(value: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let (mut start, mut quoted, mut escaped) = (0, false, false);
    for (index, c) in value.char_indices() {
        match c {
            _ if escaped => escaped = false,
            '\\' if quoted => escaped = true,
            '"' => quoted = !quoted,
            ';' if !quoted => {
                parts.push(&value[start..index]);
                start = index + 1;
            },
            _ => {},
        }
    }
    parts.push(&value[start..]);
    parts
}

fn unquote(value: &str) -> String {
    let Some(quoted) = value.strip_prefix('"') else {
        return value.to_owned();
    };
    let mut out = String::with_capacity(quoted.len());
    let mut chars = quoted.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => break,
            '\\' => out.extend(chars.next()),
            c => out.push(c),
        }
    }
    out
}

fn decode_extended_value(value: &str) -> Option<String> {
    let mut fields = value.splitn(3, '\'');
    let (charset, _language, encoded) = (fields.next()?, fields.next()?, fields.next()?);
    let bytes = percent_decode(encoded);
    if charset.eq_ignore_ascii_case("utf-8") {
        String::from_utf8(bytes).ok()
    } else if charset.eq_ignore_ascii_case("iso-8859-1") {
        Some(bytes.into_iter().map(char::from).collect())
    } else {
        None
    }
}

fn decode_lossy(input: &str) -> String {
    String::from_utf8_lossy(&percent_decode(input)).into_owned()
}

fn percent_decode(input: &str) -> Vec<u8> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if let [b'%', hi, lo, ..] = bytes[i..]
            && let (Some(hi), Some(lo)) = (hex_digit(hi), hex_digit(lo))
        {
            out.push((hi << 4) | lo);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    out
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

    #[test]
    fn sanitize_filename_neutralizes_hostile_names_idempotently() {
        for (input, expected) in [
            ("../etc/passwd", "__etc_passwd"),
            ("..\\windows\\system32", "__windows_system32"),
            ("a\0b\tc", "a_b_c"),
            (r#"bad<>:"|?*.txt"#, "bad_______.txt"),
            ("invoice\u{202E}fdp.exe", "invoice_fdp.exe"),
            ("zero\u{200B}width", "zero_width"),
            ("CON", "_CON"),
            ("con.txt", "_con.txt"),
            ("CON .txt", "_CON .txt"),
            ("COM0", "_COM0"),
            ("lpt³.log", "_lpt³.log"),
            ("CONIN$", "_CONIN$"),
            ("COM10", "COM10"),
            ("name.  ", "name"),
            ("  .", "download"),
            ("", "download"),
            ("ubuntu-24.04.iso", "ubuntu-24.04.iso"),
            ("a\u{200D}b.png", "a\u{200D}b.png"),
        ] {
            let once = sanitize_filename(input);
            assert_eq!(once, expected, "{input:?}");
            assert_eq!(sanitize_filename(&once), once, "{input:?}");
        }
    }

    #[test]
    fn sanitize_filename_caps_length_idempotently() {
        for input in [
            "a".repeat(500),
            format!("{}.pdf", "a".repeat(500)),
            format!("{}.zip", "💾".repeat(240)),
            format!("{}{}", "a".repeat(239), ". b"),
            format!("{}.{}", "a".repeat(300), ".".repeat(5)),
            format!("CON.{}", "x".repeat(300)),
        ] {
            let once = sanitize_filename(&input);
            assert!(once.len() <= MAX_COMPONENT_BYTES, "{} bytes", once.len());
            assert_eq!(sanitize_filename(&once), once);
            crate::path::validate_leaf_filename(&once).expect("sanitized name is valid");
        }
        let long = sanitize_filename(&format!("{}.pdf", "a".repeat(500)));
        assert_eq!(
            std::path::Path::new(&long).extension(),
            Some("pdf".as_ref())
        );
    }

    #[test]
    fn content_disposition_parses_rfc6266_shapes() {
        for (header, attachment, filename) in [
            (
                r#"attachment; filename="report.pdf""#,
                true,
                Some("report.pdf"),
            ),
            (
                "attachment; filename=report.pdf; size=3",
                true,
                Some("report.pdf"),
            ),
            (
                r#"attachment; filename*=UTF-8''r%C3%A9sum%C3%A9.pdf; filename="resume.pdf""#,
                true,
                Some("résumé.pdf"),
            ),
            (
                r#"attachment; filename="fallback.pdf"; filename*=utf-8''real.pdf"#,
                true,
                Some("real.pdf"),
            ),
            (
                "Attachment; FILENAME=\"Upper.pdf\"",
                true,
                Some("Upper.pdf"),
            ),
            (
                "attachment; filename*=iso-8859-1'en'caf%E9.txt",
                true,
                Some("café.txt"),
            ),
            (
                r#"attachment; filename="a\"b;c.txt"; x=1"#,
                true,
                Some("a_b;c.txt"),
            ),
            (
                r#"inline; filename="attachment.html""#,
                false,
                Some("attachment.html"),
            ),
            ("attachment; x-filename=evil.exe", true, None),
            ("attachment; filename*=koi8-r''x.txt", true, None),
            (
                r#"attachment; filename="../../.bashrc""#,
                true,
                Some("____.bashrc"),
            ),
            ("attachment", true, None),
        ] {
            let parsed = parse_content_disposition(header);
            assert_eq!(parsed.attachment, attachment, "{header}");
            assert_eq!(parsed.filename.as_deref(), filename, "{header}");
        }
    }

    #[test]
    fn extract_filename_uses_url_path_and_fragments() {
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
            assert_eq!(extract_filename(url), expected, "{url}");
        }
    }

    #[test]
    fn parse_archive_part_accepts_numbered_rar_parts_only() {
        let part = |base: &str, part_number| {
            Some(ArchivePart {
                base_name: base.into(),
                part_number,
            })
        };
        assert_eq!(parse_archive_part("Game.part01.rar"), part("Game", 1));
        assert_eq!(parse_archive_part("Game.PART002.RAR"), part("Game", 2));
        assert_eq!(parse_archive_part("Game.part1.rar"), part("Game", 1));
        for name in [
            "Game.rar",
            "Game.part01.zip",
            "Game.part00.rar",
            "Game.part0.rar",
        ] {
            assert_eq!(parse_archive_part(name), None, "{name}");
        }
    }

    #[test]
    fn suggest_folder_name_handles_archive_sets_and_fallbacks() {
        for (names, expected) in [
            (&["foo.part01.rar", "foo.part02.rar"][..], Some("foo")),
            (&["archive.7z.001", "archive.7z.002"][..], Some("archive")),
            (&["data.r00", "data.r01", "data.rar"][..], Some("data")),
            (&["movie.mp4"][..], Some("movie")),
            (&["a.txt"][..], None),
            (&["alpha.zip", "beta.zip"][..], None),
            (&["foo_-_.part01.rar", "foo_-_.part02.rar"][..], Some("foo")),
            (&["2024.zip", "2024.rar"][..], None),
            (
                &[
                    "Some_Game_--_uploader.example_--_.part1.rar",
                    "Some_Game_--_uploader.example_--_.part2.rar",
                ][..],
                Some("Some Game"),
            ),
            (
                &["Some_Game_--_uploader.example_--_.part1.rar"][..],
                Some("Some Game"),
            ),
            (&["Some.Movie--Extras.mkv"][..], Some("Some.Movie - Extras")),
            (&["Dune--Part.Two.mkv"][..], Some("Dune - Part.Two")),
            (&["ubuntu-24.04.iso"][..], Some("ubuntu-24.04")),
            (&[][..], None),
        ] {
            assert_eq!(suggest_folder_name(names).as_deref(), expected, "{names:?}");
        }
    }
}
