pub(crate) fn is_magnet(text: &str) -> bool {
    text.get(..7)
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("magnet:"))
}

pub(crate) fn is_link(text: &str) -> bool {
    let Ok(url) = url::Url::parse(text) else {
        return false;
    };
    match url.scheme() {
        "http" | "https" => url.has_host(),
        "magnet" => url
            .query_pairs()
            .any(|(name, value)| name == "xt" && is_btih(&value)),
        _ => false,
    }
}

fn is_btih(value: &str) -> bool {
    let Some(hash) = value.strip_prefix("urn:btih:") else {
        return false;
    };
    match hash.len() {
        40 => hash.bytes().all(|byte| byte.is_ascii_hexdigit()),
        32 => hash
            .bytes()
            .all(|byte| byte.is_ascii_alphabetic() || (b'2'..=b'7').contains(&byte)),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_fetchable_links_are_accepted() {
        let cases = [
            ("https://example.com/file.zip", true),
            ("http://mirror.example.org/linux.iso", true),
            ("https://example.com/?token=abc#file.zip", true),
            (
                "magnet:?xt=urn:btih:cab507494d02ebb1178b38f2e9d7be299c86b862&dn=example",
                true,
            ),
            ("MAGNET:?xt=urn:btih:MFRGGZDFMZTWQ2LKNNWG23TPOBYXE43U", true),
            ("not a url", false),
            ("https://", false),
            ("ftp://example.com/file.zip", false),
            ("file:///etc/passwd", false),
            ("magnet:?dn=missing-info-hash", false),
            ("magnet:?xt=urn:btih:too-short", false),
            ("magnet:not-a-query", false),
        ];
        for (text, expected) in cases {
            assert_eq!(is_link(text), expected, "{text}");
        }
    }
}
