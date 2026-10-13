use std::ffi::OsStr;
use std::path::{Component, Path};

use crate::error::{Result, ShioError};

pub(crate) const MAX_COMPONENT_BYTES: usize = 240;
const MAX_RELATIVE_PATH_BYTES: usize = 4_096;

pub(crate) fn path_to_utf8<'a>(field: &'static str, path: &'a Path) -> Result<&'a str> {
    path.to_str().ok_or_else(|| ShioError::DatabaseValue {
        field,
        value: "non-utf8 path".to_string(),
    })
}

pub(crate) fn validate_leaf_filename(name: &str) -> Result<()> {
    if name != crate::sanitize_filename(name) || Path::new(name).components().count() != 1 {
        return Err(ShioError::Config(format!("invalid filename: {name}")));
    }
    validate_component(OsStr::new(name))
}

pub(crate) fn validate_relative_path(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty() || path.is_absolute() {
        return Err(ShioError::Config(format!(
            "path must be relative: {}",
            path.display()
        )));
    }
    if path.as_os_str().len() > MAX_RELATIVE_PATH_BYTES {
        return Err(ShioError::Config("relative path is too long".into()));
    }
    for component in path.components() {
        match component {
            Component::Normal(part) => validate_component(part)?,
            Component::CurDir
            | Component::ParentDir
            | Component::RootDir
            | Component::Prefix(_) => {
                return Err(ShioError::Config(format!(
                    "path escapes destination: {}",
                    path.display()
                )));
            },
        }
    }
    Ok(())
}

fn validate_component(name: &OsStr) -> Result<()> {
    let Some(name) = name.to_str() else {
        return Err(ShioError::Config(
            "path component is not valid UTF-8".into(),
        ));
    };
    if name.len() > MAX_COMPONENT_BYTES {
        return Err(ShioError::Config("path component is too long".into()));
    }
    if name.chars().any(is_forbidden_char) || name.ends_with(['.', ' ']) || is_reserved_name(name) {
        return Err(ShioError::Config(format!("invalid path component: {name}")));
    }
    Ok(())
}

// Bidi/format characters spoof names (`invoice\u{202E}fdp.exe`); ZWJ/ZWNJ stay for scripts/emoji.
pub(crate) fn is_forbidden_char(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '<' | '>'
                | ':'
                | '"'
                | '|'
                | '?'
                | '*'
                | '\\'
                | '\u{AD}'
                | '\u{61C}'
                | '\u{180E}'
                | '\u{200B}'
                | '\u{200E}'
                | '\u{200F}'
                | '\u{202A}'..='\u{202E}'
                | '\u{2060}'..='\u{2064}'
                | '\u{2066}'..='\u{2069}'
                | '\u{FEFF}'
                | '\u{FFF9}'..='\u{FFFB}'
        )
}

// Device names stay reserved with any extension and trailing spaces (`con .txt`).
pub(crate) fn is_reserved_name(name: &str) -> bool {
    const DEVICES: &[&str] = &["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"];
    let stem = name.split('.').next().unwrap_or(name).trim_end_matches(' ');
    if DEVICES
        .iter()
        .any(|device| stem.eq_ignore_ascii_case(device))
    {
        return true;
    }
    let (Some(prefix), Some(rest)) = (stem.get(..3), stem.get(3..)) else {
        return false;
    };
    let mut rest = rest.chars();
    (prefix.eq_ignore_ascii_case("COM") || prefix.eq_ignore_ascii_case("LPT"))
        && rest
            .next()
            .is_some_and(|d| d.is_ascii_digit() || matches!(d, '¹' | '²' | '³'))
        && rest.next().is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths_reject_escapes_and_unportable_components() {
        for path in [
            "",
            "../file.bin",
            "/tmp/file.bin",
            "./file.bin",
            "release/../file.bin",
            "release/NUL.txt",
            "release/com0",
            "release/LPT¹.log",
            "release/conin$",
            "release/CON .txt",
            "release/file.txt:stream",
            "release/bad?.txt",
            "release/trailing. ",
            "release/invoice\u{202E}fdp.exe",
        ] {
            assert!(validate_relative_path(Path::new(path)).is_err(), "{path}");
        }
        if cfg!(unix) {
            assert!(validate_relative_path(Path::new("release\\windows.bin")).is_err());
        }
        for path in [
            "release/cd1/file.bin",
            "COM10.txt",
            "console.log",
            "a\u{200D}b",
        ] {
            assert!(validate_relative_path(Path::new(path)).is_ok(), "{path}");
        }
    }
}
