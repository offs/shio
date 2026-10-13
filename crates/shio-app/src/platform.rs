mod associations;
#[cfg(target_os = "linux")]
pub(crate) mod noctalia;
#[cfg(windows)]
mod registry;

use std::fs::{File, OpenOptions};
use std::io;
use std::path::Path;

pub(crate) use associations::{
    ASSOCIATION_SETUP_SUPPORTED, ASSOCIATION_SUCCESS_MESSAGE, AssociationLaunch, UNREGISTER_FLAG,
    association_launch_from_args, register_associations, unregister_associations,
};

pub(crate) const APP_ID: &str = "com.shio.DownloadManager";
const ICON_BYTES: &[u8] = include_bytes!("../../../assets/icons/icon-256.png");

pub(crate) fn window_icon() -> Option<iced::window::Icon> {
    let image = image::load_from_memory(ICON_BYTES).ok()?.to_rgba8();
    let (width, height) = image.dimensions();
    iced::window::icon::from_rgba(image.into_raw(), width, height).ok()
}

pub(crate) fn create_private_dir(path: &Path) -> io::Result<()> {
    std::fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata = std::fs::symlink_metadata(path)?;
        if !metadata.is_dir() {
            return Err(io::Error::other(format!(
                "{} is not a directory",
                path.display()
            )));
        }
        if metadata.permissions().mode() & 0o777 != 0o700 {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
        }
    }
    Ok(())
}

pub(crate) fn open_private(path: &Path, options: &OpenOptions) -> io::Result<File> {
    reject_non_file(path)?;
    #[cfg(unix)]
    let options = &{
        use std::os::unix::fs::OpenOptionsExt as _;
        let mut options = options.clone();
        options.mode(0o600);
        options
    };
    options.open(path)
}

fn reject_non_file(path: &Path) -> io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.is_file() => Err(io::Error::other(format!(
            "{} is not a regular file",
            path.display()
        ))),
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

pub(crate) fn remove_if_exists(path: &Path) -> io::Result<()> {
    match std::fs::remove_file(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

#[cfg(windows)]
pub(crate) fn register_app_id() {
    #![allow(unsafe_code)]
    let id = registry::wide(APP_ID);
    // SAFETY: `id` is NUL-terminated and outlives the call.
    let hr = unsafe {
        windows_sys::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID(id.as_ptr())
    };
    if hr < 0 {
        tracing::warn!("SetCurrentProcessExplicitAppUserModelID failed: 0x{hr:08x}");
    }
    let mut values = vec![("DisplayName", "shio")];
    let icon = icon_file();
    if let Some(icon) = icon.as_deref().and_then(Path::to_str) {
        values.push(("IconUri", icon));
    }
    if let Err(error) = registry::write_key(&app_id_key(), &values) {
        tracing::warn!("{error}");
    }
}

#[cfg(windows)]
fn app_id_key() -> String {
    format!("Software\\Classes\\AppUserModelId\\{APP_ID}")
}

#[cfg(windows)]
fn icon_path() -> Option<std::path::PathBuf> {
    Some(std::path::PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("shio/icon.png"))
}

#[cfg(windows)]
fn icon_file() -> Option<std::path::PathBuf> {
    let path = icon_path()?;
    if std::fs::read(&path).ok().as_deref() != Some(ICON_BYTES) {
        std::fs::create_dir_all(path.parent()?).ok()?;
        std::fs::write(&path, ICON_BYTES).ok()?;
    }
    Some(path)
}
