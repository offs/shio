use std::path::PathBuf;

#[cfg(windows)]
const REGISTERED_APP_NAME: &str = "Shio";
#[cfg(windows)]
const TORRENT_PROG_ID: &str = "Shio.Torrent";
#[cfg(windows)]
const MAGNET_PROG_ID: &str = "Shio.Magnet";

#[cfg(any(windows, target_os = "linux"))]
const TORRENT_MIME: &str = "application/x-bittorrent";
#[cfg(target_os = "linux")]
const MAGNET_MIME: &str = "x-scheme-handler/magnet";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct AssociationLaunch {
    pub(crate) magnets: Vec<String>,
    pub(crate) torrent_files: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AssociationError {
    message: String,
}

impl AssociationError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for AssociationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for AssociationError {}

pub(crate) fn association_launch_from_args<I>(args: I) -> AssociationLaunch
where
    I: IntoIterator<Item = std::ffi::OsString>,
{
    let mut launch = AssociationLaunch::default();

    for arg in args.into_iter().skip(1) {
        if let Some(value) = arg.to_str()
            && value
                .trim_start()
                .to_ascii_lowercase()
                .starts_with("magnet:")
        {
            launch.magnets.push(value.trim().to_string());
            continue;
        }

        let path = PathBuf::from(arg);
        if path
            .extension()
            .and_then(std::ffi::OsStr::to_str)
            .is_some_and(|ext| ext.eq_ignore_ascii_case("torrent"))
        {
            launch.torrent_files.push(path);
        }
    }

    launch
}

pub(crate) const fn successful_association_message() -> &'static str {
    if default_app_settings_available() {
        "registered shio; choose shio for .torrent and magnet in Windows Default Apps"
    } else {
        "file associations registered"
    }
}

pub(crate) fn complete_download_association_setup() -> Result<(), AssociationError> {
    register_download_associations()?;
    if default_app_settings_available() {
        open_default_app_settings()?;
    }
    Ok(())
}

fn register_download_associations() -> Result<(), AssociationError> {
    #[cfg(windows)]
    {
        windows::register_download_associations()
    }

    #[cfg(target_os = "linux")]
    {
        linux::register_download_associations()
    }

    #[cfg(all(not(windows), not(target_os = "linux")))]
    {
        Err(AssociationError::new(
            "file association setup is only implemented on Windows and Linux",
        ))
    }
}

const fn default_app_settings_available() -> bool {
    cfg!(windows)
}

fn open_default_app_settings() -> Result<(), AssociationError> {
    #[cfg(windows)]
    {
        open::that("ms-settings:defaultapps")
            .map_err(|e| AssociationError::new(format!("open Windows default apps: {e}")))?;
        Ok(())
    }

    #[cfg(not(windows))]
    {
        Err(AssociationError::new(
            "default app settings are only implemented on Windows",
        ))
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::{AssociationError, MAGNET_MIME, TORRENT_MIME, default_handler_matches};
    use crate::platform::APP_USER_MODEL_ID;
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    pub(super) fn register_download_associations() -> Result<(), AssociationError> {
        let exe = std::env::current_exe()
            .map_err(|e| AssociationError::new(format!("resolve shio executable: {e}")))?;
        let applications_dir = applications_dir()?;
        std::fs::create_dir_all(&applications_dir).map_err(|e| {
            AssociationError::new(format!("create {}: {e}", applications_dir.display()))
        })?;

        let desktop_id = format!("{APP_USER_MODEL_ID}.desktop");
        let desktop_path = applications_dir.join(&desktop_id);
        let mut file = std::fs::File::create(&desktop_path).map_err(|e| {
            AssociationError::new(format!("create {}: {e}", desktop_path.display()))
        })?;
        file.write_all(desktop_entry(&exe).as_bytes())
            .map_err(|e| AssociationError::new(format!("write {}: {e}", desktop_path.display())))?;

        match Command::new("update-desktop-database")
            .arg(&applications_dir)
            .status()
        {
            Ok(status) if status.success() => {},
            Ok(status) => tracing::warn!("update-desktop-database failed with {status}"),
            Err(e) => tracing::warn!("update-desktop-database could not run: {e}"),
        }
        set_xdg_mime_default(&desktop_id, TORRENT_MIME)?;
        set_xdg_mime_default(&desktop_id, MAGNET_MIME)?;
        verify_xdg_mime_default(&desktop_id, TORRENT_MIME)?;
        verify_xdg_mime_default(&desktop_id, MAGNET_MIME)
    }

    fn applications_dir() -> Result<PathBuf, AssociationError> {
        if let Some(data_home) = std::env::var_os("XDG_DATA_HOME") {
            return Ok(PathBuf::from(data_home).join("applications"));
        }
        let home =
            std::env::var_os("HOME").ok_or_else(|| AssociationError::new("HOME is not set"))?;
        Ok(PathBuf::from(home).join(".local/share/applications"))
    }

    fn desktop_entry(exe: &Path) -> String {
        format!(
            "[Desktop Entry]\nType=Application\nName=Shio\nExec={} %U\nTerminal=false\nMimeType={TORRENT_MIME};{MAGNET_MIME};\nCategories=Network;FileTransfer;\n",
            shell_quote(&exe.display().to_string())
        )
    }

    fn shell_quote(value: &str) -> String {
        format!("'{}'", value.replace('\'', "'\\''"))
    }

    fn set_xdg_mime_default(desktop_id: &str, mime: &str) -> Result<(), AssociationError> {
        let status = Command::new("xdg-mime")
            .arg("default")
            .arg(desktop_id)
            .arg(mime)
            .status()
            .map_err(|e| AssociationError::new(format!("run xdg-mime: {e}")))?;
        if status.success() {
            return Ok(());
        }
        Err(AssociationError::new(format!(
            "xdg-mime default {desktop_id} {mime} failed with {status}"
        )))
    }

    fn verify_xdg_mime_default(desktop_id: &str, mime: &str) -> Result<(), AssociationError> {
        let output = Command::new("xdg-mime")
            .arg("query")
            .arg("default")
            .arg(mime)
            .output()
            .map_err(|e| AssociationError::new(format!("query xdg-mime default {mime}: {e}")))?;
        if !output.status.success() {
            return Err(AssociationError::new(format!(
                "xdg-mime query default {mime} failed with {}",
                output.status
            )));
        }

        let actual = String::from_utf8_lossy(&output.stdout);
        if default_handler_matches(desktop_id, &actual) {
            return Ok(());
        }
        Err(AssociationError::new(format!(
            "desktop did not make shio the default for {mime}"
        )))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn desktop_entry_declares_torrent_and_magnet_handlers() {
            let entry = desktop_entry(Path::new("/opt/shio/shio"));

            assert!(entry.contains("application/x-bittorrent;x-scheme-handler/magnet;"));
        }
    }
}

#[cfg(any(target_os = "linux", test))]
fn default_handler_matches(desktop_id: &str, output: &str) -> bool {
    output.lines().any(|line| line.trim() == desktop_id)
}

#[cfg(windows)]
const fn current_exe_filename() -> &'static str {
    "shio.exe"
}

#[cfg(windows)]
const fn torrent_content_type() -> &'static str {
    TORRENT_MIME
}

#[cfg(windows)]
const fn magnet_content_type() -> &'static str {
    "application/x-magnet"
}

#[cfg(windows)]
const fn torrent_perceived_type() -> &'static str {
    "application"
}

#[cfg(windows)]
const fn torrent_friendly_type_name() -> &'static str {
    "Torrent metadata file"
}

#[cfg(windows)]
const fn magnet_friendly_type_name() -> &'static str {
    "Magnet URI"
}

#[cfg(windows)]
const fn application_description() -> &'static str {
    "Download manager for direct links, .torrent files, and magnet links"
}

#[cfg(windows)]
const fn application_name() -> &'static str {
    REGISTERED_APP_NAME
}

#[cfg(windows)]
const fn torrent_prog_id() -> &'static str {
    TORRENT_PROG_ID
}

#[cfg(windows)]
const fn magnet_prog_id() -> &'static str {
    MAGNET_PROG_ID
}

#[cfg(windows)]
const fn application_capabilities_path() -> &'static str {
    "Software\\Shio\\Capabilities"
}

#[cfg(windows)]
const fn registered_app_name() -> &'static str {
    REGISTERED_APP_NAME
}

#[cfg(windows)]
const fn registered_app_path() -> &'static str {
    "Software\\Shio\\Capabilities"
}

#[cfg(windows)]
fn shell_open_command(exe: &std::path::Path) -> String {
    format!("\"{}\" \"%1\"", exe.display())
}

#[cfg(windows)]
fn icon_reference(exe: &std::path::Path) -> String {
    format!("{},0", exe.display())
}

#[cfg(windows)]
fn install_location(exe: &std::path::Path) -> Option<String> {
    exe.parent().map(|path| path.display().to_string())
}

#[cfg(windows)]
fn application_exe_registry_key() -> String {
    format!(
        "Software\\Classes\\Applications\\{}",
        current_exe_filename()
    )
}

#[cfg(windows)]
fn torrent_prog_id_key() -> String {
    format!("Software\\Classes\\{}", torrent_prog_id())
}

#[cfg(windows)]
fn magnet_prog_id_key() -> String {
    format!("Software\\Classes\\{}", magnet_prog_id())
}

#[cfg(windows)]
fn register_download_associations_windows(exe: &std::path::Path) -> Result<(), AssociationError> {
    let command = shell_open_command(exe);
    let icon = icon_reference(exe);

    if let Some(install_location) = install_location(exe) {
        windows::write_key("Software\\Shio", &[("InstallLocation", &install_location)])?;
    }

    windows::write_key(
        application_capabilities_path(),
        &[
            ("ApplicationDescription", application_description()),
            ("ApplicationIcon", &icon),
            ("ApplicationName", application_name()),
        ],
    )?;
    windows::write_key(
        "Software\\Shio\\Capabilities\\FileAssociations",
        &[(".torrent", torrent_prog_id())],
    )?;
    windows::write_key(
        "Software\\Shio\\Capabilities\\MIMEAssociations",
        &[(torrent_content_type(), torrent_prog_id())],
    )?;
    windows::write_key(
        "Software\\Shio\\Capabilities\\UrlAssociations",
        &[("magnet", magnet_prog_id())],
    )?;
    windows::write_key(
        "Software\\RegisteredApplications",
        &[(registered_app_name(), registered_app_path())],
    )?;

    windows::write_key(
        &application_exe_registry_key(),
        &[("FriendlyAppName", "Shio")],
    )?;
    windows::write_key(
        &format!("{}\\SupportedTypes", application_exe_registry_key()),
        &[(".torrent", "")],
    )?;
    windows::write_key(
        &format!("{}\\shell\\open\\command", application_exe_registry_key()),
        &[("", &command)],
    )?;

    windows::write_key(
        &torrent_prog_id_key(),
        &[
            ("", "Torrent file"),
            ("FriendlyTypeName", torrent_friendly_type_name()),
        ],
    )?;
    windows::write_key(
        &format!("{}\\DefaultIcon", torrent_prog_id_key()),
        &[("", &icon)],
    )?;
    windows::write_key(
        &format!("{}\\shell\\open\\command", torrent_prog_id_key()),
        &[("", &command)],
    )?;
    windows::write_key(
        "Software\\Classes\\.torrent",
        &[
            ("Content Type", torrent_content_type()),
            ("PerceivedType", torrent_perceived_type()),
        ],
    )?;
    windows::write_key(
        "Software\\Classes\\.torrent\\OpenWithProgids",
        &[(torrent_prog_id(), "")],
    )?;
    windows::write_key(
        &format!(
            "Software\\Classes\\.torrent\\OpenWithList\\{}",
            current_exe_filename()
        ),
        &[("", "")],
    )?;
    windows::write_key(
        "Software\\Classes\\MIME\\Database\\Content Type\\application/x-bittorrent",
        &[("Extension", ".torrent")],
    )?;

    windows::write_key(
        &magnet_prog_id_key(),
        &[
            ("", "Magnet link"),
            ("FriendlyTypeName", magnet_friendly_type_name()),
            ("URL Protocol", ""),
        ],
    )?;
    windows::write_key(
        &format!("{}\\DefaultIcon", magnet_prog_id_key()),
        &[("", &icon)],
    )?;
    windows::write_key(
        &format!("{}\\shell\\open\\command", magnet_prog_id_key()),
        &[("", &command)],
    )?;
    windows::write_key(
        "Software\\Classes\\magnet",
        &[
            ("", "URL:Magnet URI"),
            ("Content Type", magnet_content_type()),
            ("URL Protocol", ""),
        ],
    )?;
    windows::write_key(
        &format!(
            "Software\\Classes\\magnet\\OpenWithList\\{}",
            current_exe_filename()
        ),
        &[("", "")],
    )?;
    windows::write_key(
        "Software\\Classes\\magnet\\shell\\open\\command",
        &[("", &command)],
    )?;

    windows::notify_associations_changed();
    Ok(())
}

#[cfg(windows)]
mod windows {
    use super::AssociationError;
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey,
        RegCreateKeyExW, RegSetValueExW,
    };
    use windows_sys::Win32::UI::Shell::{SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify};

    pub(super) fn register_download_associations() -> Result<(), AssociationError> {
        let exe = std::env::current_exe()
            .map_err(|e| AssociationError::new(format!("resolve shio executable: {e}")))?;
        super::register_download_associations_windows(&exe)
    }

    pub(super) fn notify_associations_changed() {
        #[allow(unsafe_code)]
        unsafe {
            SHChangeNotify(
                i32::try_from(SHCNE_ASSOCCHANGED).unwrap_or(i32::MAX),
                SHCNF_IDLIST,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            );
        }
    }

    pub(super) fn write_key(path: &str, values: &[(&str, &str)]) -> Result<(), AssociationError> {
        #[allow(unsafe_code)]
        unsafe {
            let mut hkey: HKEY = std::ptr::null_mut();
            let path_w = wide_z(path);
            let status = RegCreateKeyExW(
                HKEY_CURRENT_USER,
                path_w.as_ptr(),
                0,
                std::ptr::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_WRITE,
                std::ptr::null(),
                &raw mut hkey,
                std::ptr::null_mut(),
            );
            if status != ERROR_SUCCESS {
                return Err(AssociationError::new(format!(
                    "create HKCU\\{path}: {status}"
                )));
            }
            for (name, value) in values {
                set_string_result(hkey, name, value)
                    .map_err(|e| AssociationError::new(format!("write HKCU\\{path}: {e}")))?;
            }
            RegCloseKey(hkey);
        }

        Ok(())
    }

    #[allow(unsafe_code)]
    /// # Safety
    /// `hkey` must be a valid open registry key handle with write access.
    unsafe fn set_string_result(
        hkey: HKEY,
        name: &str,
        value: &str,
    ) -> Result<(), AssociationError> {
        let name_w = wide_z(name);
        let value_w: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
        let byte_len = u32::try_from(value_w.len() * 2).unwrap_or(u32::MAX);
        let bytes = value_w.as_ptr().cast::<u8>();
        let status = unsafe { RegSetValueExW(hkey, name_w.as_ptr(), 0, REG_SZ, bytes, byte_len) };
        if status != ERROR_SUCCESS {
            return Err(AssociationError::new(format!(
                "RegSetValueExW({name}) failed: {status}"
            )));
        }
        Ok(())
    }

    fn wide_z(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    #[test]
    fn association_launch_parser_collects_magnets_and_torrents() {
        let args = [
            "shio",
            "magnet:?xt=urn:btih:cab507494d02ebb1178b38f2e9d7be299c86b862",
            "C:\\downloads\\release.torrent",
            "https://example.com/file.zip",
        ]
        .into_iter()
        .map(OsString::from);

        let launch = association_launch_from_args(args);

        assert_eq!(launch.magnets.len(), 1);
        assert_eq!(
            launch.torrent_files,
            vec![PathBuf::from("C:\\downloads\\release.torrent")]
        );
    }

    #[test]
    fn association_launch_parser_ignores_non_association_args() {
        let args = ["shio", "--flag", "C:\\downloads\\release.txt"]
            .into_iter()
            .map(OsString::from);

        assert_eq!(
            association_launch_from_args(args),
            AssociationLaunch::default()
        );
    }

    #[test]
    fn xdg_default_verification_accepts_exact_desktop_id() {
        assert!(default_handler_matches(
            "com.shio.DownloadManager.desktop",
            "com.shio.DownloadManager.desktop\n"
        ));
    }

    #[test]
    fn xdg_default_verification_rejects_other_handler() {
        assert!(!default_handler_matches(
            "com.shio.DownloadManager.desktop",
            "transmission-qt.desktop\n"
        ));
    }

    #[cfg(windows)]
    #[test]
    fn windows_command_quotes_exe_and_argument() {
        let command = shell_open_command(std::path::Path::new("C:\\Program Files\\Shio\\shio.exe"));

        assert_eq!(command, "\"C:\\Program Files\\Shio\\shio.exe\" \"%1\"");
    }

    #[cfg(windows)]
    #[test]
    fn windows_registration_uses_specific_prog_ids() {
        assert_eq!(torrent_prog_id(), "Shio.Torrent");
        assert_eq!(magnet_prog_id(), "Shio.Magnet");
        assert_eq!(torrent_content_type(), "application/x-bittorrent");
        assert_eq!(magnet_content_type(), "application/x-magnet");
    }

    #[cfg(windows)]
    #[test]
    fn windows_success_message_keeps_user_choice_explicit() {
        assert_eq!(
            successful_association_message(),
            "registered shio; choose shio for .torrent and magnet in Windows Default Apps"
        );
    }
}
