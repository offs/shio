use std::path::{Path, PathBuf};

use super::metainfo::{StagedFile, metadata_bytes, staged_files};
use crate::error::{Result, ShioError};
use crate::types::{Download, DownloadStatus, TorrentFile, TorrentState};

const PUBLISHED: &[u8] = b"published\n";

pub(crate) fn staging_root(download: &Download) -> PathBuf {
    download
        .save_path
        .join(format!(".shio-{}.torrent", download.id))
}

pub(super) fn payload_dir(download: &Download) -> PathBuf {
    staging_root(download).join("payload")
}

fn published_marker(download: &Download) -> PathBuf {
    staging_root(download).join("published")
}

fn owner_marker(download: &Download) -> PathBuf {
    staging_root(download).join("owner")
}

fn torrent(download: &Download) -> Result<&TorrentState> {
    download
        .torrent()
        .ok_or_else(|| ShioError::Other("expected a torrent download".into()))
}

fn selected<'a>(
    files: &'a [TorrentFile],
    staged: &'a [StagedFile],
) -> impl Iterator<Item = (&'a TorrentFile, &'a StagedFile)> {
    files.iter().zip(staged).filter(|(file, _)| file.selected)
}

pub(super) async fn ensure_owned_staging(download: &Download) -> Result<PathBuf> {
    tokio::fs::create_dir_all(&download.save_path).await?;
    let root = staging_root(download);
    match tokio::fs::create_dir(&root).await {
        Ok(()) => {
            #[cfg(unix)]
            tokio::fs::set_permissions(&root, std::os::unix::fs::PermissionsExt::from_mode(0o700))
                .await?;
            crate::worker::sync_parent_directory(&root).await?;
            if let Err(error) =
                write_marker(&owner_marker(download), download.id.to_string().as_bytes()).await
            {
                if let Err(cleanup_error) = tokio::fs::remove_dir(&root).await {
                    tracing::warn!(path = %root.display(), "failed to clean uninitialized torrent staging directory: {cleanup_error}");
                }
                return Err(error);
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            verify_owner(download).await?;
        },
        Err(error) => return Err(error.into()),
    }
    let payload = payload_dir(download);
    match tokio::fs::create_dir(&payload).await {
        Ok(()) => crate::worker::sync_parent_directory(&payload).await?,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let metadata = tokio::fs::symlink_metadata(&payload).await?;
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                return Err(ShioError::DestinationExists(payload));
            }
        },
        Err(error) => return Err(error.into()),
    }
    Ok(payload)
}

async fn write_marker(path: &Path, contents: &[u8]) -> Result<()> {
    use tokio::io::AsyncWriteExt as _;

    let mut marker = tokio::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .await?;
    let written = async {
        marker.write_all(contents).await?;
        marker.sync_all().await
    }
    .await;
    if let Err(error) = written {
        drop(marker);
        if let Err(cleanup_error) = tokio::fs::remove_file(path).await {
            tracing::warn!(path = %path.display(), "failed to clean incomplete marker: {cleanup_error}");
        }
        return Err(error.into());
    }
    crate::worker::sync_parent_directory(path).await
}

async fn verify_owner(download: &Download) -> Result<()> {
    let root = staging_root(download);
    let metadata = tokio::fs::symlink_metadata(&root).await?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || !marker_matches(&owner_marker(download), download.id.to_string().as_bytes()).await?
    {
        return Err(ShioError::DestinationExists(root));
    }
    Ok(())
}

async fn marker_matches(path: &Path, expected: &[u8]) -> Result<bool> {
    use tokio::io::AsyncReadExt as _;

    let metadata = match tokio::fs::symlink_metadata(path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Ok(false);
    }
    let mut contents = Vec::with_capacity(expected.len() + 1);
    tokio::fs::File::open(path)
        .await?
        .take(expected.len() as u64 + 1)
        .read_to_end(&mut contents)
        .await?;
    Ok(contents == expected)
}

async fn owned_staging_exists(download: &Download) -> Result<bool> {
    match tokio::fs::symlink_metadata(staging_root(download)).await {
        Ok(_) => verify_owner(download).await.map(|()| true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

pub(crate) async fn cleanup_staging(download: &Download) -> Result<()> {
    if !owned_staging_exists(download).await? {
        return Ok(());
    }
    match tokio::fs::remove_dir_all(staging_root(download)).await {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error.into()),
        _ => Ok(()),
    }
}

pub(super) async fn is_published(download: &Download) -> Result<bool> {
    marker_matches(&published_marker(download), PUBLISHED).await
}

/// The selected destination files once published; call it before cleaning the staging.
pub(crate) async fn published_files(download: &Download) -> Result<Vec<PathBuf>> {
    let published = if owned_staging_exists(download).await? {
        is_published(download).await?
    } else {
        download.status == DownloadStatus::Completed
    };
    if !published {
        return Ok(Vec::new());
    }
    Ok(torrent(download)?
        .files
        .iter()
        .filter(|file| file.selected)
        .map(|file| download.save_path.join(&file.path))
        .collect())
}

/// Rebuilds a completed torrent's staging from its published files so librqbit can seed again.
pub(crate) async fn prepare_completed_recheck(download: &Download) -> Result<()> {
    if owned_staging_exists(download).await? {
        return Ok(());
    }
    let state = torrent(download)?;
    let bytes = metadata_bytes(state)
        .ok_or_else(|| ShioError::Other("completed torrent has no metadata".into()))?;
    let staged = staged_files(state, bytes)?;
    let payload = ensure_owned_staging(download).await?;
    let linked = async {
        for (file, staged) in selected(&state.files, &staged) {
            let source = download.save_path.join(&file.path);
            let metadata = tokio::fs::symlink_metadata(&source).await?;
            if !metadata.is_file()
                || metadata.file_type().is_symlink()
                || metadata.len() != file.size
            {
                return Err(ShioError::Other(format!(
                    "torrent payload is missing or has the wrong size: {}",
                    source.display()
                )));
            }
            let target = payload.join(&staged.path);
            if let Some(parent) = target.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            tokio::fs::hard_link(&source, &target).await?;
        }
        write_marker(&published_marker(download), PUBLISHED).await
    }
    .await;
    if linked.is_err()
        && let Err(error) = cleanup_staging(download).await
    {
        tracing::warn!(download = %download.id, "failed to roll back recheck staging: {error}");
    }
    linked
}

pub(super) async fn check_destinations(download: &Download, staged: &[StagedFile]) -> Result<()> {
    let payload = payload_dir(download);
    for (file, staged) in selected(&torrent(download)?.files, staged) {
        let destination = download.save_path.join(&file.path);
        match tokio::fs::symlink_metadata(&destination).await {
            Ok(_) => {
                if !matches!(
                    crate::worker::same_file(&payload.join(&staged.path), &destination).await,
                    Ok(true)
                ) {
                    return Err(ShioError::DestinationExists(destination));
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

/// `may_move` lets a file leave staging when hard links are unsupported; rollback moves it back.
pub(super) async fn publish_payload(
    download: &Download,
    staged: &[StagedFile],
    may_move: bool,
) -> Result<()> {
    if is_published(download).await? {
        return Ok(());
    }
    let payload = payload_dir(download);
    let mut published = Vec::new();
    let result = async {
        for (file, staged) in selected(&torrent(download)?.files, staged) {
            ensure_destination_parent(&download.save_path, &file.path).await?;
            let source = payload.join(&staged.path);
            let destination = download.save_path.join(&file.path);
            if tokio::fs::symlink_metadata(&destination).await.is_ok() {
                if matches!(
                    crate::worker::same_file(&source, &destination).await,
                    Ok(true)
                ) {
                    continue;
                }
                return Err(ShioError::DestinationExists(destination));
            }
            let moved =
                crate::worker::publish_file_no_clobber(&source, &destination, may_move).await?;
            published.push((destination, moved.then_some(source)));
        }
        write_marker(&published_marker(download), PUBLISHED).await
    }
    .await;
    if result.is_err() {
        for (path, moved_from) in &published {
            let undone = match moved_from {
                Some(source) => crate::worker::rename_no_clobber(path, source),
                None => match tokio::fs::remove_file(path).await {
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                    undone => undone,
                },
            };
            if let Err(error) = undone {
                tracing::warn!(path = %path.display(), "torrent publish rollback failed: {error}");
            }
        }
    }
    result
}

async fn ensure_destination_parent(save_path: &Path, relative_file: &Path) -> Result<()> {
    let Some(parent) = relative_file.parent() else {
        return Ok(());
    };
    let mut current = save_path.to_path_buf();
    for component in parent.components() {
        current.push(component);
        match tokio::fs::create_dir(&current).await {
            Ok(()) => crate::worker::sync_parent_directory(&current).await?,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let metadata = tokio::fs::symlink_metadata(&current).await?;
                if !metadata.is_dir() || metadata.file_type().is_symlink() {
                    return Err(ShioError::DestinationExists(current));
                }
            },
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::torrent::metainfo::tests::{bencode_torrent, torrent_download};

    fn staged_download(root: &Path) -> (Download, Vec<StagedFile>) {
        let bytes = bencode_torrent(&[("", &["release", "file.bin"], 7)]);
        let download = torrent_download(bytes, root.to_path_buf());
        let state = download.torrent().expect("torrent");
        let staged = staged_files(state, metadata_bytes(state).expect("bytes")).expect("staged");
        (download, staged)
    }

    async fn stage_payload(download: &Download) -> PathBuf {
        let staged = ensure_owned_staging(download)
            .await
            .expect("owned stage")
            .join("release/file.bin");
        tokio::fs::create_dir_all(staged.parent().expect("parent"))
            .await
            .expect("staged dir");
        tokio::fs::write(&staged, b"torrent").await.expect("staged");
        staged
    }

    #[tokio::test]
    async fn publication_never_clobbers_a_foreign_file() {
        let temp = tempfile::tempdir().expect("tempdir");
        let (download, staged_layout) = staged_download(temp.path());
        let staged = stage_payload(&download).await;
        let destination = temp.path().join("release/file.bin");
        tokio::fs::create_dir(temp.path().join("release"))
            .await
            .expect("dir");
        tokio::fs::write(&destination, b"user data")
            .await
            .expect("destination");

        let result = publish_payload(&download, &staged_layout, true).await;

        assert!(matches!(result, Err(ShioError::DestinationExists(path)) if path == destination));
        assert_eq!(
            tokio::fs::read(&destination).await.expect("dest"),
            b"user data"
        );
        assert_eq!(tokio::fs::read(staged).await.expect("staged"), b"torrent");
        assert_eq!(
            published_files(&download).await.expect("owned"),
            Vec::<PathBuf>::new()
        );
    }

    #[tokio::test]
    async fn published_files_survive_staging_cleanup_and_stay_owned() {
        let temp = tempfile::tempdir().expect("tempdir");
        let (mut download, staged_layout) = staged_download(temp.path());
        stage_payload(&download).await;
        let destination = temp.path().join("release/file.bin");

        publish_payload(&download, &staged_layout, false)
            .await
            .expect("publish");

        assert_eq!(
            published_files(&download).await.expect("owned"),
            std::slice::from_ref(&destination)
        );
        cleanup_staging(&download).await.expect("cleanup");
        assert!(!staging_root(&download).exists());
        assert_eq!(
            tokio::fs::read(&destination).await.expect("dest"),
            b"torrent"
        );
        download.status = DownloadStatus::Completed;
        assert_eq!(
            published_files(&download).await.expect("owned"),
            [destination]
        );
    }

    #[tokio::test]
    async fn unowned_staging_directory_is_never_reused_or_removed() {
        let temp = tempfile::tempdir().expect("tempdir");
        let (download, _) = staged_download(temp.path());
        let root = staging_root(&download);
        tokio::fs::create_dir(&root).await.expect("foreign stage");
        tokio::fs::write(root.join("user-file"), b"keep")
            .await
            .expect("foreign");

        assert!(ensure_owned_staging(&download).await.is_err());
        assert!(cleanup_staging(&download).await.is_err());
        assert_eq!(
            tokio::fs::read(root.join("user-file")).await.expect("kept"),
            b"keep"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn publication_refuses_a_symlinked_destination_parent() {
        let temp = tempfile::tempdir().expect("tempdir");
        let outside = tempfile::tempdir().expect("outside");
        let (download, staged_layout) = staged_download(temp.path());
        stage_payload(&download).await;
        std::os::unix::fs::symlink(outside.path(), temp.path().join("release")).expect("symlink");

        let result = publish_payload(&download, &staged_layout, true).await;

        assert!(matches!(result, Err(ShioError::DestinationExists(_))));
        assert!(!outside.path().join("file.bin").exists());
    }
}
