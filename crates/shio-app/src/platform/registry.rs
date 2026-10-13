#![allow(unsafe_code)]

use std::io;
use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, WIN32_ERROR};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_REG_SZ,
    RegCloseKey, RegCreateKeyExW, RegDeleteKeyValueW, RegDeleteTreeW, RegGetValueW, RegSetValueExW,
};

pub(super) fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn check(status: WIN32_ERROR, what: &str, path: &str) -> io::Result<()> {
    if status == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "{what} HKCU\\{path}: {}",
            io::Error::from_raw_os_error(status.cast_signed())
        )))
    }
}

pub(super) fn write_key(path: &str, values: &[(&str, &str)]) -> io::Result<()> {
    let path_w = wide(path);
    let mut key: HKEY = std::ptr::null_mut();
    // SAFETY: `path_w` is NUL-terminated and outlives the call; `key` receives the opened handle.
    let status = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            path_w.as_ptr(),
            0,
            std::ptr::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            std::ptr::null(),
            &raw mut key,
            std::ptr::null_mut(),
        )
    };
    check(status, "create", path)?;
    let result = values.iter().try_for_each(|(name, value)| {
        let name_w = wide(name);
        let value_w = wide(value);
        let len = u32::try_from(value_w.len() * 2)
            .map_err(|_| io::Error::other(format!("registry value {name} is too long")))?;
        // SAFETY: `key` is open with write access; both buffers are NUL-terminated and `len`
        // is exactly the byte length of `value_w`.
        let status = unsafe {
            RegSetValueExW(
                key,
                name_w.as_ptr(),
                0,
                REG_SZ,
                value_w.as_ptr().cast(),
                len,
            )
        };
        check(status, "write", path)
    });
    // SAFETY: `key` was opened above and is closed exactly once.
    unsafe { RegCloseKey(key) };
    result
}

pub(super) fn read_default(path: &str) -> io::Result<Option<String>> {
    let path_w = wide(path);
    let mut buffer = [0_u16; 1024];
    let mut len = u32::try_from(size_of_val(&buffer)).map_err(io::Error::other)?;
    // SAFETY: `path_w` is NUL-terminated; `buffer` holds `len` bytes, which the call updates.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            path_w.as_ptr(),
            std::ptr::null(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buffer.as_mut_ptr().cast(),
            &raw mut len,
        )
    };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    check(status, "read", path)?;
    let units = (len as usize / 2).saturating_sub(1);
    Ok(Some(String::from_utf16_lossy(&buffer[..units])))
}

pub(super) fn delete_tree(path: &str) -> io::Result<()> {
    let path_w = wide(path);
    // SAFETY: `path_w` is NUL-terminated and outlives the call.
    let status = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, path_w.as_ptr()) };
    absent_ok(status, path)
}

pub(super) fn delete_value(path: &str, name: &str) -> io::Result<()> {
    let (path_w, name_w) = (wide(path), wide(name));
    // SAFETY: both buffers are NUL-terminated and outlive the call.
    let status = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, path_w.as_ptr(), name_w.as_ptr()) };
    absent_ok(status, path)
}

fn absent_ok(status: WIN32_ERROR, path: &str) -> io::Result<()> {
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(());
    }
    check(status, "delete", path)
}
