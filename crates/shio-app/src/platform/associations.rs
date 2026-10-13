use crate::link::is_magnet;
use std::ffi::OsString;
#[cfg(not(any(windows, target_os = "linux")))]
use std::io;
use std::path::{Path, PathBuf};

pub(crate) const UNREGISTER_FLAG: &str = "--unregister-associations";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct AssociationLaunch {
    pub(crate) magnets: Vec<String>,
    pub(crate) torrent_files: Vec<PathBuf>,
}

pub(crate) fn association_launch_from_args(
    args: impl IntoIterator<Item = OsString>,
) -> AssociationLaunch {
    let mut launch = AssociationLaunch::default();
    for arg in args.into_iter().skip(1) {
        if let Some(value) = arg.to_str().map(str::trim) {
            if is_magnet(value) {
                launch.magnets.push(value.to_owned());
                continue;
            }
            if !is_windows_drive_path(value)
                && let Ok(url) = url::Url::parse(value)
            {
                // `file:name` parses as `/name`, and a host would be a remote (UNC) share.
                if value
                    .get(..7)
                    .is_some_and(|scheme| scheme.eq_ignore_ascii_case("file://"))
                    && url.host().is_none()
                    && let Ok(path) = url.to_file_path()
                    && is_torrent_path(&path)
                {
                    launch.torrent_files.push(path);
                }
                continue;
            }
        }
        let path = PathBuf::from(arg);
        if is_torrent_path(&path)
            && let Ok(path) = std::path::absolute(path)
        {
            launch.torrent_files.push(path);
        }
    }
    launch
}

const fn is_windows_drive_path(value: &str) -> bool {
    matches!(value.as_bytes(), [drive, b':', b'\\' | b'/', ..] if drive.is_ascii_alphabetic())
}

fn is_torrent_path(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("torrent"))
}

pub(crate) const ASSOCIATION_SETUP_SUPPORTED: bool = cfg!(any(windows, target_os = "linux"));

pub(crate) const ASSOCIATION_SUCCESS_MESSAGE: &str = if cfg!(windows) {
    "registered shio; choose it for .torrent and magnet in default apps"
} else {
    "shio now opens .torrent files and magnet links"
};

#[cfg(windows)]
pub(crate) use windows::{
    register as register_associations, unregister as unregister_associations,
};

#[cfg(target_os = "linux")]
pub(crate) use linux::{register as register_associations, unregister as unregister_associations};

#[cfg(not(any(windows, target_os = "linux")))]
pub(crate) fn register_associations() -> io::Result<()> {
    Err(io::Error::from(io::ErrorKind::Unsupported))
}

#[cfg(not(any(windows, target_os = "linux")))]
pub(crate) fn unregister_associations() -> io::Result<()> {
    Err(io::Error::from(io::ErrorKind::Unsupported))
}

#[cfg(target_os = "linux")]
mod linux {
    use crate::platform::{APP_ID, ICON_BYTES, reject_non_file, remove_if_exists};
    use std::ffi::OsString;
    use std::io::{self, Write};
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    use std::path::{Path, PathBuf};
    use std::process::{Command, Output};

    const MIME_TYPES: [&str; 2] = ["application/x-bittorrent", "x-scheme-handler/magnet"];
    const PACKAGED_ENTRY: &str =
        include_str!("../../../../packaging/linux/com.shio.DownloadManager.desktop");
    // The icon theme spec asks for at least a 48px hicolor icon.
    const ICONS: [(&str, &[u8]); 2] = [
        (
            "48x48",
            include_bytes!("../../../../assets/icons/icon-48.png"),
        ),
        ("256x256", ICON_BYTES),
    ];

    pub(crate) fn register() -> io::Result<()> {
        let entry = desktop_entry(&stable_executable()?)?;
        let data = data_home()?;
        for (size, bytes) in ICONS {
            write_atomic(&icon_path(&data, size), bytes)?;
        }
        let applications = data.join("applications");
        let desktop_id = desktop_id();
        write_atomic(&applications.join(&desktop_id), entry.as_bytes())?;
        update_desktop_database(&applications);
        for mime in MIME_TYPES {
            let set = xdg_mime(&["default", &desktop_id, mime])?;
            if !set.status.success() {
                return Err(io::Error::other(format!(
                    "xdg-mime default {mime} failed with {}",
                    set.status
                )));
            }
            let query = xdg_mime(&["query", "default", mime])?;
            if !String::from_utf8_lossy(&query.stdout)
                .lines()
                .any(|line| line.trim() == desktop_id)
            {
                return Err(io::Error::other(format!(
                    "the desktop did not make shio the default for {mime}"
                )));
            }
        }
        Ok(())
    }

    pub(crate) fn unregister() -> io::Result<()> {
        let data = data_home()?;
        let applications = data.join("applications");
        for (size, _) in ICONS {
            remove_if_exists(&icon_path(&data, size))?;
        }
        remove_if_exists(&applications.join(desktop_id()))?;
        update_desktop_database(&applications);
        Ok(())
    }

    fn desktop_id() -> String {
        format!("{APP_ID}.desktop")
    }

    fn icon_path(data: &Path, size: &str) -> PathBuf {
        data.join(format!("icons/hicolor/{size}/apps/{APP_ID}.png"))
    }

    fn data_home() -> io::Result<PathBuf> {
        if let Some(data) = std::env::var_os("XDG_DATA_HOME").filter(|d| Path::new(d).is_absolute())
        {
            return Ok(data.into());
        }
        let home = std::env::var_os("HOME").ok_or_else(|| io::Error::other("HOME is not set"))?;
        Ok(PathBuf::from(home).join(".local/share"))
    }

    fn xdg_mime(args: &[&str]) -> io::Result<Output> {
        Command::new("xdg-mime")
            .args(args)
            .output()
            .map_err(|error| {
                if error.kind() == io::ErrorKind::NotFound {
                    io::Error::new(error.kind(), "xdg-mime not found; install xdg-utils")
                } else {
                    error
                }
            })
    }

    fn update_desktop_database(applications: &Path) {
        match Command::new("update-desktop-database")
            .arg(applications)
            .status()
        {
            Ok(status) if status.success() => {},
            Ok(status) => tracing::warn!("update-desktop-database failed with {status}"),
            Err(error) => tracing::warn!("update-desktop-database: {error}"),
        }
    }

    fn stable_executable() -> io::Result<PathBuf> {
        let path = select_executable(
            std::env::var_os("APPIMAGE"),
            std::env::var_os("APPDIR"),
            std::env::current_exe()?,
        )?;
        let metadata = std::fs::symlink_metadata(&path)?;
        if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
            return Err(io::Error::other(format!(
                "{} is not a regular executable file",
                path.display()
            )));
        }
        Ok(path)
    }

    // APPIMAGE is inherited by everything started from inside an image, so it names shio
    // only while this executable lives in that image's own APPDIR mount.
    fn select_executable(
        appimage: Option<OsString>,
        appdir: Option<OsString>,
        current: PathBuf,
    ) -> io::Result<PathBuf> {
        let inside = appdir.is_some_and(|appdir| current.starts_with(appdir));
        if let Some(appimage) = appimage.filter(|_| inside).map(PathBuf::from) {
            return if appimage.is_absolute() {
                Ok(appimage)
            } else {
                Err(io::Error::other("APPIMAGE is not an absolute path"))
            };
        }
        let transient = current
            .components()
            .any(|part| part.as_os_str().to_string_lossy().starts_with(".mount_"));
        if transient {
            return Err(io::Error::other(
                "cannot register a transient AppImage mount path",
            ));
        }
        Ok(current)
    }

    fn desktop_entry(exe: &Path) -> io::Result<String> {
        let exe = exe
            .to_str()
            .filter(|exe| !exe.chars().any(char::is_control))
            .ok_or_else(|| io::Error::other("the shio executable path is not plain UTF-8"))?;
        let launch = format!(
            "TryExec={}\nExec={} %U",
            exe.replace('\\', "\\\\"),
            exec_quote(exe)
        );
        Ok(PACKAGED_ENTRY.replacen("Exec=shio %U", &launch, 1))
    }

    // Exec values are unescaped as desktop strings before being unquoted, so quoted-argument
    // escapes need their backslash doubled.
    fn exec_quote(value: &str) -> String {
        let mut quoted = String::with_capacity(value.len() + 2);
        quoted.push('"');
        for c in value.chars() {
            match c {
                '"' | '`' | '$' => {
                    quoted.push_str("\\\\");
                    quoted.push(c);
                },
                '\\' => quoted.push_str("\\\\\\\\"),
                '%' => quoted.push_str("%%"),
                _ => quoted.push(c),
            }
        }
        quoted.push('"');
        quoted
    }

    fn write_atomic(path: &Path, contents: &[u8]) -> io::Result<()> {
        let dir = path
            .parent()
            .ok_or_else(|| io::Error::other("path has no parent"))?;
        std::fs::create_dir_all(dir)?;
        reject_non_file(path)?;
        let temporary = dir.join(format!(".{APP_ID}.{}.tmp", std::process::id()));
        let result = (|| {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o644)
                .open(&temporary)?;
            file.write_all(contents)?;
            file.sync_all()?;
            std::fs::rename(&temporary, path)?;
            std::fs::File::open(dir)?.sync_all()
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        result
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn exec_escaping_follows_the_desktop_entry_spec() {
            let entry = desktop_entry(Path::new("/opt/Shio $beta`\"\\%/shio")).unwrap();

            assert!(entry.contains(r#"Exec="/opt/Shio \\$beta\\`\\"\\\\%%/shio" %U"#));
            assert!(entry.contains("TryExec=/opt/Shio $beta`\"\\\\%/shio\n"));
            assert!(desktop_entry(Path::new("/opt/shio\n/shio")).is_err());
        }

        #[test]
        fn only_our_own_appimage_is_registered() {
            let mounted = PathBuf::from("/tmp/.mount_shio/usr/bin/shio");
            let ours = || Some("/tmp/.mount_shio".into());

            assert_eq!(
                select_executable(Some("/opt/Shio.AppImage".into()), ours(), mounted.clone())
                    .unwrap(),
                PathBuf::from("/opt/Shio.AppImage")
            );
            assert!(select_executable(None, None, mounted.clone()).is_err());
            assert!(select_executable(Some("Shio.AppImage".into()), ours(), mounted).is_err());
            assert_eq!(
                select_executable(
                    Some("/opt/Other.AppImage".into()),
                    Some("/tmp/.mount_other".into()),
                    PathBuf::from("/usr/bin/shio"),
                )
                .unwrap(),
                PathBuf::from("/usr/bin/shio"),
                "an APPIMAGE inherited from another image is ignored"
            );
        }

        #[test]
        fn atomic_writer_refuses_a_symbolic_link_target() {
            let root = std::env::temp_dir().join(format!("shio-desktop-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&root).unwrap();
            let target = root.join("unrelated.desktop");
            std::fs::write(&target, b"keep me").unwrap();
            let link = root.join("shio.desktop");
            std::os::unix::fs::symlink(&target, &link).unwrap();

            assert!(write_atomic(&link, b"replace me").is_err());
            assert_eq!(std::fs::read(&target).unwrap(), b"keep me");
            std::fs::remove_dir_all(root).unwrap();
        }
    }
}

#[cfg(windows)]
mod windows {
    use crate::platform::registry::{delete_tree, delete_value, read_default, write_key};
    use crate::platform::{app_id_key, icon_path, remove_if_exists};
    use std::io;
    use std::path::Path;

    const CAPABILITIES: &str = "Software\\Shio\\Capabilities";
    const TORRENT_PROG_ID: &str = "Software\\Classes\\Shio.Torrent";
    const MAGNET_PROG_ID: &str = "Software\\Classes\\Shio.Magnet";
    const APPLICATION: &str = "Software\\Classes\\Applications\\shio.exe";
    const OPEN_WITH: &str = "Software\\Classes\\.torrent\\OpenWithProgids";
    const MAGNET: &str = "Software\\Classes\\magnet";
    const MAGNET_COMMAND: &str = "Software\\Classes\\magnet\\shell\\open\\command";

    pub(crate) fn register() -> io::Result<()> {
        let exe = std::env::current_exe()?;
        let command = open_command(&exe);
        let icon = format!("{},0", exe.display());
        write_key(
            CAPABILITIES,
            &[
                ("ApplicationName", "Shio"),
                (
                    "ApplicationDescription",
                    "download manager for links, torrents and magnets",
                ),
                ("ApplicationIcon", &icon),
            ],
        )?;
        write_key(
            &format!("{CAPABILITIES}\\FileAssociations"),
            &[(".torrent", "Shio.Torrent")],
        )?;
        write_key(
            &format!("{CAPABILITIES}\\MIMEAssociations"),
            &[("application/x-bittorrent", "Shio.Torrent")],
        )?;
        write_key(
            &format!("{CAPABILITIES}\\UrlAssociations"),
            &[("magnet", "Shio.Magnet")],
        )?;
        write_key(
            "Software\\RegisteredApplications",
            &[("Shio", CAPABILITIES)],
        )?;
        for (prog_id, name, extra) in [
            (TORRENT_PROG_ID, "torrent file", None),
            (MAGNET_PROG_ID, "magnet link", Some(("URL Protocol", ""))),
        ] {
            let mut values = vec![("", name)];
            values.extend(extra);
            write_key(prog_id, &values)?;
            write_key(&format!("{prog_id}\\DefaultIcon"), &[("", &icon)])?;
            write_key(
                &format!("{prog_id}\\shell\\open\\command"),
                &[("", &command)],
            )?;
        }
        write_key(OPEN_WITH, &[("Shio.Torrent", "")])?;
        write_key(APPLICATION, &[("FriendlyAppName", "Shio")])?;
        write_key(
            &format!("{APPLICATION}\\SupportedTypes"),
            &[(".torrent", "")],
        )?;
        write_key(
            &format!("{APPLICATION}\\shell\\open\\command"),
            &[("", &command)],
        )?;
        // Without any magnet handler Windows offers no choice for magnet links; never take
        // over one that belongs to another client.
        if read_default(MAGNET_COMMAND)?.is_none_or(|existing| existing == command) {
            write_key(MAGNET, &[("", "URL:magnet link"), ("URL Protocol", "")])?;
            write_key(MAGNET_COMMAND, &[("", &command)])?;
        }
        notify_changed();
        Ok(())
    }

    pub(crate) fn unregister() -> io::Result<()> {
        let command = open_command(&std::env::current_exe()?);
        if read_default(MAGNET_COMMAND)?.is_some_and(|existing| existing == command) {
            delete_tree(MAGNET)?;
        }
        for key in [
            "Software\\Shio",
            TORRENT_PROG_ID,
            MAGNET_PROG_ID,
            APPLICATION,
            &app_id_key(),
        ] {
            delete_tree(key)?;
        }
        delete_value("Software\\RegisteredApplications", "Shio")?;
        delete_value(OPEN_WITH, "Shio.Torrent")?;
        if let Some(icon) = icon_path() {
            remove_if_exists(&icon)?;
        }
        notify_changed();
        Ok(())
    }

    fn open_command(exe: &Path) -> String {
        format!("\"{}\" \"%1\"", exe.display())
    }

    #[allow(unsafe_code)]
    fn notify_changed() {
        use windows_sys::Win32::UI::Shell::{SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify};
        // SAFETY: SHCNE_ASSOCCHANGED takes no item pointers.
        unsafe {
            SHChangeNotify(
                SHCNE_ASSOCCHANGED.cast_signed(),
                SHCNF_IDLIST,
                std::ptr::null(),
                std::ptr::null(),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_args_keep_only_magnets_and_local_torrents() {
        let magnet = "magnet:?xt=urn:btih:cab507494d02ebb1178b38f2e9d7be299c86b862";
        let local = std::env::temp_dir().join("Shio files/雪.torrent");
        let url = url::Url::from_file_path(&local).unwrap();
        let localhost = url.as_str().replacen("file://", "file://localhost", 1);
        let launch = association_launch_from_args(
            [
                "shio",
                magnet,
                "relative/a.torrent",
                "--flag",
                "notes.txt",
                "https://example.com/release.torrent",
                "file:not-an-absolute-path.torrent",
                "file://remote.invalid/release.torrent",
                url.as_str(),
                &localhost,
            ]
            .map(OsString::from),
        );

        assert_eq!(launch.magnets, [magnet]);
        assert_eq!(
            launch.torrent_files,
            [
                std::env::current_dir().unwrap().join("relative/a.torrent"),
                local.clone(),
                local,
            ]
        );
    }
}
