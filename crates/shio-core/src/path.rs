use std::ffi::OsStr;
use std::path::{Component, Path};

use crate::error::{Result, ShioError};

pub(crate) fn path_to_utf8<'a>(field: &'static str, path: &'a Path) -> Result<&'a str> {
    path.to_str().ok_or_else(|| ShioError::DatabaseValue {
        field,
        value: "non-utf8 path".to_string(),
    })
}

pub(crate) fn validate_leaf_filename(name: &str) -> Result<()> {
    let path = Path::new(name);
    if name.is_empty() || path.components().count() != 1 || name != crate::sanitize_filename(name) {
        return Err(ShioError::Config(format!("invalid filename: {name}")));
    }
    validate_platform_component(OsStr::new(name))
}

pub(crate) fn validate_relative_path(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty() || path.is_absolute() {
        return Err(ShioError::Config(format!(
            "path must be relative: {}",
            path.display()
        )));
    }

    let mut saw_normal = false;
    for component in path.components() {
        match component {
            Component::Normal(part) => {
                saw_normal = true;
                validate_platform_component(part)?;
            },
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

    if !saw_normal {
        return Err(ShioError::Config("path must contain a filename".into()));
    }
    Ok(())
}

fn validate_platform_component(name: &OsStr) -> Result<()> {
    let Some(name) = name.to_str() else {
        return Ok(());
    };
    let stem = name.split('.').next().unwrap_or(name);
    if name.contains(':') || is_reserved_device_name(stem) {
        return Err(ShioError::Config(format!("invalid path component: {name}")));
    }
    Ok(())
}

fn is_reserved_device_name(name: &str) -> bool {
    const RESERVED: &[&str] = &[
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];

    RESERVED
        .iter()
        .any(|reserved| name.eq_ignore_ascii_case(reserved))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_path_rejects_escape_and_reserved_components() {
        assert!(validate_relative_path(Path::new("../file.bin")).is_err());
        assert!(validate_relative_path(Path::new("/tmp/file.bin")).is_err());
        assert!(validate_relative_path(Path::new("release/NUL.txt")).is_err());
        assert!(validate_relative_path(Path::new("release/file.txt:stream")).is_err());
    }

    #[test]
    fn relative_path_accepts_nested_normal_components() {
        assert!(validate_relative_path(Path::new("release/cd1/file.bin")).is_ok());
    }
}
