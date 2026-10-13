mod add_model;
mod blur;
mod notification;
mod sort;
mod state;
mod subscription;
mod update;
mod view;
#[cfg(target_os = "linux")]
mod wayland;

pub(crate) use add_model::{AddDialog, AddValidation, Link, LinkState, TorrentPick};
pub(crate) use notification::show as show_notification;
pub(crate) use state::{SettingsDraft, Shio, ToastKind, Totals};

const MAX_SEARCH_TEXT_BYTES: usize = 512;
const MAX_NAME_TEXT_BYTES: usize = 255;
const MAX_PATH_TEXT_BYTES: usize = 32 * 1024;
const MAX_PASSWORD_TEXT_BYTES: usize = 1024;
const MAX_NUMERIC_TEXT_BYTES: usize = 32;

fn truncate_text_bytes(mut value: String, max_bytes: usize) -> String {
    if value.len() > max_bytes {
        let boundary = value.floor_char_boundary(max_bytes);
        value.truncate(boundary);
    }
    value
}

/// Folder pickers must not silently alter non-UTF-8 paths that a text field cannot hold.
fn path_text(path: &std::path::Path) -> Result<String, &'static str> {
    path.to_str()
        .map(str::to_owned)
        .ok_or("folders with non-UTF-8 names are not supported")
}

async fn read_bounded_file(path: &std::path::Path, max_bytes: u64) -> Result<Vec<u8>, String> {
    use tokio::io::AsyncReadExt as _;

    let file = tokio::fs::File::open(path)
        .await
        .map_err(|error| format!("open file: {error}"))?;
    let metadata = file
        .metadata()
        .await
        .map_err(|error| format!("inspect file: {error}"))?;
    if !metadata.is_file() {
        return Err("not a regular file".to_owned());
    }
    if metadata.len() > max_bytes {
        return Err(format!("file exceeds {max_bytes} byte limit"));
    }
    let mut bytes = Vec::new();
    file.take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .await
        .map_err(|error| format!("read file: {error}"))?;
    if bytes.len() as u64 > max_bytes {
        return Err(format!("file exceeds {max_bytes} byte limit"));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use shio_core::{Download, DownloadId, DownloadKind, DownloadStatus, HttpState};

    pub(super) fn id(n: usize) -> DownloadId {
        DownloadId(
            format!("{n:032x}")
                .as_str()
                .try_into()
                .expect("32 hex digits form a uuid"),
        )
    }

    pub(super) fn download(n: usize, status: DownloadStatus) -> Download {
        Download {
            id: id(n),
            filename: format!("file-{n}.bin"),
            save_path: "/downloads".into(),
            total_size: Some(100),
            downloaded: 0,
            status,
            speed: 0,
            error_message: None,
            created_at: std::time::SystemTime::UNIX_EPOCH.into(),
            started_at: None,
            completed_at: None,
            retry_count: 0,
            max_retries: 3,
            pinned: false,
            kind: DownloadKind::Http(HttpState {
                url: format!("https://example.com/file-{n}.bin"),
                headers: Vec::new(),
                segments: 8,
                subfolder: None,
                auto_extract: false,
            }),
        }
    }

    #[test]
    fn text_cap_keeps_a_valid_multibyte_boundary() {
        assert_eq!(truncate_text_bytes("ab雪cd".to_owned(), 4), "ab");
        assert_eq!(truncate_text_bytes("ab雪cd".to_owned(), 5), "ab雪");
    }

    #[cfg(unix)]
    #[test]
    fn path_text_rejects_non_utf8_instead_of_changing_it() {
        use std::os::unix::ffi::OsStringExt;
        let path = std::path::PathBuf::from(std::ffi::OsString::from_vec(vec![0xff]));
        assert!(path_text(&path).is_err());
    }

    #[tokio::test]
    async fn oversized_files_are_rejected_before_reading() {
        let path = std::env::temp_dir().join(format!("shio-large-{}.torrent", std::process::id()));
        std::fs::File::create(&path)
            .and_then(|file| file.set_len(1025))
            .expect("sparse file");
        let result = read_bounded_file(&path, 1024).await;
        std::fs::remove_file(&path).expect("cleanup");
        assert!(result.is_err());
    }
}
