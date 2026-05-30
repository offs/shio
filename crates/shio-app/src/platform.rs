mod associations;

use iced::window::icon;

pub(crate) use associations::{
    AssociationLaunch, association_launch_from_args, complete_download_association_setup,
    successful_association_message,
};

const ICON_BYTES: &[u8] = include_bytes!("../../../assets/icons/icon-256.png");
const APP_USER_MODEL_ID: &str = "com.shio.DownloadManager";

#[cfg(windows)]
const DISPLAY_NAME: &str = "shio";

pub(crate) fn window_icon() -> Option<iced::window::Icon> {
    let img = image::load_from_memory(ICON_BYTES).ok()?.to_rgba8();
    let (w, h) = img.dimensions();
    icon::from_rgba(img.into_raw(), w, h).ok()
}

#[allow(clippy::missing_const_for_fn)]
pub(crate) fn register_app_id() {
    #[cfg(windows)]
    {
        windows::set_app_user_model_id();
        windows::register_aumid_in_registry();
    }
}

pub(crate) const fn app_id() -> &'static str {
    APP_USER_MODEL_ID
}

#[cfg(windows)]
mod windows {
    use super::{APP_USER_MODEL_ID, DISPLAY_NAME, ICON_BYTES};
    use std::io::Write;
    use std::path::PathBuf;
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey,
        RegCreateKeyExW, RegSetValueExW,
    };
    use windows_sys::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;

    pub(super) fn set_app_user_model_id() {
        let wide = wide_z(APP_USER_MODEL_ID);
        #[allow(unsafe_code)]
        unsafe {
            let hr = SetCurrentProcessExplicitAppUserModelID(wide.as_ptr());
            if hr < 0 {
                tracing::warn!("SetCurrentProcessExplicitAppUserModelID failed: 0x{hr:08x}");
            }
        }
    }

    pub(super) fn register_aumid_in_registry() {
        let subkey = format!("Software\\Classes\\AppUserModelId\\{APP_USER_MODEL_ID}");
        let icon_path = ensure_icon_on_disk();
        #[allow(unsafe_code)]
        unsafe {
            let mut hkey: HKEY = std::ptr::null_mut();
            let path = wide_z(&subkey);
            let status = RegCreateKeyExW(
                HKEY_CURRENT_USER,
                path.as_ptr(),
                0,
                std::ptr::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_WRITE,
                std::ptr::null(),
                &raw mut hkey,
                std::ptr::null_mut(),
            );
            if status != ERROR_SUCCESS {
                tracing::warn!("RegCreateKeyExW failed: {status}");
                return;
            }
            set_string(hkey, "DisplayName", DISPLAY_NAME);
            if let Some(path) = icon_path.and_then(|p| p.to_str().map(str::to_owned)) {
                set_string(hkey, "IconUri", &path);
            }
            RegCloseKey(hkey);
        }
    }

    #[allow(unsafe_code)]
    /// # Safety
    /// `hkey` must be a valid open registry key handle with write access.
    unsafe fn set_string(hkey: HKEY, name: &str, value: &str) {
        let name_w = wide_z(name);
        let value_w: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
        let byte_len = u32::try_from(value_w.len() * 2).unwrap_or(u32::MAX);
        let bytes = value_w.as_ptr().cast::<u8>();
        let status = unsafe { RegSetValueExW(hkey, name_w.as_ptr(), 0, REG_SZ, bytes, byte_len) };
        if status != ERROR_SUCCESS {
            tracing::warn!("RegSetValueExW({name}) failed: {status}");
        }
    }

    fn ensure_icon_on_disk() -> Option<PathBuf> {
        let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from)?;
        let dir = base.join("shio");
        std::fs::create_dir_all(&dir).ok()?;
        let path = dir.join("icon.png");
        let needs_write = std::fs::read(&path)
            .map(|existing| existing != ICON_BYTES)
            .unwrap_or(true);
        if needs_write {
            let mut f = std::fs::File::create(&path).ok()?;
            f.write_all(ICON_BYTES).ok()?;
        }
        Some(path)
    }

    fn wide_z(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }
}
