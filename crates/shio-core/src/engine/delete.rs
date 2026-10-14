// Only payloads Shio published are deleted, through a journaled quarantine that a crash rolls
// back (row still present) or completes (row gone). Everything but `targets` blocks.

use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

use crate::db::{Database, DeleteJournal};
use crate::error::{Result, ShioError};
use crate::types::{Download, DownloadId, DownloadKind, TorrentFile};

pub(super) struct Targets {
    owner: DownloadId,
    root: PathBuf,
    files: Vec<PathBuf>,
    dirs: Vec<PathBuf>,
}

pub(super) struct Quarantine {
    owner: DownloadId,
    root: PathBuf,
    dir: Option<PathBuf>,
    moved: Vec<(PathBuf, PathBuf)>,
    dirs: Vec<PathBuf>,
}

pub(super) async fn targets(download: &Download) -> Result<Targets> {
    let mut root = download.save_path.clone();
    let mut dirs = Vec::new();
    let files = match &download.kind {
        DownloadKind::Http(http) => {
            dirs.extend(
                http.subfolder
                    .as_ref()
                    .map(|subfolder| root.join(subfolder)),
            );
            let row = download.clone();
            crate::worker::blocking(move || Ok(crate::worker::published_file(&row)))
                .await?
                .into_iter()
                .collect()
        },
        DownloadKind::Torrent(torrent) => {
            let files = crate::torrent::published_files(download).await?;
            for file in &files {
                let mut dir = file.clone();
                while dir.pop() && dir != root && dir.starts_with(&root) {
                    dirs.push(dir.clone());
                }
            }
            if is_generated_folder(download, &torrent.files)
                && let Some(parent) = root.parent()
            {
                dirs.push(root.clone());
                root = parent.to_path_buf();
            }
            files
        },
    };
    dirs.retain(|dir| dir != &root && dir.starts_with(&root));
    dirs.sort_unstable();
    dirs.dedup();
    dirs.sort_by_key(|dir| std::cmp::Reverse(dir.components().count()));
    Ok(Targets {
        owner: download.id,
        root,
        files,
        dirs,
    })
}

// The per-torrent folder the add dialog creates, named after the single selected file or the
// files' shared folder.
fn is_generated_folder(download: &Download, files: &[TorrentFile]) -> bool {
    let selected: Vec<&Path> = files
        .iter()
        .filter(|file| file.selected)
        .map(|file| file.path.as_path())
        .collect();
    let name = match selected.as_slice() {
        [] => None,
        [single] => single
            .file_stem()
            .and_then(|stem| stem.to_str())
            .map(str::to_owned),
        [first, rest @ ..] => {
            let shared = rest.iter().fold(first.components().count(), |len, path| {
                first
                    .components()
                    .zip(path.components())
                    .take(len)
                    .take_while(|(left, right)| left == right)
                    .count()
            });
            let name = first
                .components()
                .take(shared)
                .last()
                .and_then(|component| component.as_os_str().to_str())
                .map_or_else(|| download.filename.clone(), str::to_owned);
            Some(name)
        },
    };
    name.is_some_and(|name| {
        let dir = download.save_path.file_name().and_then(|dir| dir.to_str());
        [
            crate::filename::sanitize_filename(&name),
            crate::filename::release_name(&name),
        ]
        .iter()
        .any(|name| !name.is_empty() && dir == Some(name.as_str()))
    })
}

pub(super) fn quarantine(db: &Database, targets: &[Targets]) -> Result<Vec<Quarantine>> {
    for target in targets {
        preflight(target)?;
    }
    let mut done = Vec::with_capacity(targets.len());
    for target in targets {
        match quarantine_one(db, target) {
            Ok(quarantine) => done.push(quarantine),
            Err(error) => {
                rollback(db, done);
                return Err(error);
            },
        }
    }
    Ok(done)
}

fn preflight(target: &Targets) -> Result<()> {
    for file in &target.files {
        ensure_real_parents(&target.root, file)?;
        if std::fs::symlink_metadata(file).is_ok_and(|metadata| metadata.is_dir()) {
            return Err(ShioError::Other(format!(
                "refusing to remove a directory as a download file: {}",
                file.display()
            )));
        }
    }
    for dir in &target.dirs {
        ensure_real_parents(&target.root, dir)?;
    }
    Ok(())
}

fn quarantine_one(db: &Database, target: &Targets) -> Result<Quarantine> {
    let mut quarantine = Quarantine {
        owner: target.owner,
        root: target.root.clone(),
        dir: None,
        moved: Vec::new(),
        dirs: target.dirs.clone(),
    };
    let existing: Vec<&PathBuf> = target
        .files
        .iter()
        .filter(|file| std::fs::symlink_metadata(file).is_ok())
        .collect();
    if existing.is_empty() {
        return Ok(quarantine);
    }
    let dir = quarantine_dir(&target.root, target.owner);
    std::fs::create_dir(&dir).map_err(|error| {
        if error.kind() == ErrorKind::AlreadyExists {
            ShioError::Other("an earlier delete quarantine needs recovery".into())
        } else {
            error.into()
        }
    })?;
    quarantine.moved = existing
        .into_iter()
        .zip(0..)
        .map(|(file, index)| (file.clone(), dir.join(index.to_string())))
        .collect();
    quarantine.dir = Some(dir.clone());
    if let Err(error) = db.insert_delete_journal(&DeleteJournal {
        download_id: target.owner,
        protected_root: target.root.clone(),
        quarantine_root: dir.clone(),
        moved: quarantine.moved.clone(),
    }) {
        if let Err(cleanup) = std::fs::remove_dir(&dir) {
            tracing::warn!(path = %dir.display(), "failed to remove unused delete quarantine: {cleanup}");
        }
        return Err(error);
    }
    let mut renamed = 0;
    let moved = quarantine.moved.iter().try_for_each(|(file, staged)| {
        std::fs::rename(file, staged)?;
        renamed += 1;
        std::io::Result::Ok(())
    });
    if let Err(error) = moved {
        quarantine.moved.truncate(renamed);
        rollback(db, vec![quarantine]);
        return Err(error.into());
    }
    Ok(quarantine)
}

pub(super) fn rollback(db: &Database, quarantines: Vec<Quarantine>) {
    for quarantine in quarantines.into_iter().rev() {
        let mut restored = true;
        for (original, staged) in quarantine.moved.iter().rev() {
            let result = ensure_real_parents(&quarantine.root, original)
                .and_then(|()| std::fs::rename(staged, original).map_err(ShioError::from));
            if let Err(error) = result {
                tracing::error!(path = %original.display(), "delete rollback kept the file quarantined: {error}");
                restored = false;
            }
        }
        if let Some(dir) = quarantine.dir.as_ref()
            && let Err(error) = std::fs::remove_dir(dir)
        {
            tracing::warn!(path = %dir.display(), "failed to remove empty delete quarantine: {error}");
            restored = false;
        }
        if restored && let Err(error) = db.delete_delete_journal(quarantine.owner) {
            tracing::error!(download = %quarantine.owner, "failed to clear recovered delete journal: {error}");
        }
    }
}

pub(super) fn commit(db: &Database, quarantines: Vec<Quarantine>) {
    for quarantine in quarantines {
        let removed = quarantine.dir.as_ref().is_none_or(|dir| {
            ignore_not_found(std::fs::remove_dir_all(dir)).inspect_err(|error| {
                tracing::warn!(path = %dir.display(), "removed row left quarantined files behind: {error}");
            })
            .is_ok()
        });
        for dir in &quarantine.dirs {
            let result =
                ensure_real_parents(&quarantine.root, dir).and_then(
                    |()| match std::fs::remove_dir(dir) {
                        Err(error) if error.kind() == ErrorKind::DirectoryNotEmpty => Ok(()),
                        other => ignore_not_found(other),
                    },
                );
            if let Err(error) = result {
                tracing::warn!(path = %dir.display(), "failed to remove empty download directory: {error}");
            }
        }
        if removed && let Err(error) = db.delete_delete_journal(quarantine.owner) {
            tracing::warn!(download = %quarantine.owner, "failed to clear committed delete journal: {error}");
        }
    }
}

// A journal whose row still exists is rolled back, otherwise its quarantine is purged.
pub(super) fn recover(db: &Database) -> Result<()> {
    for journal in db.delete_journals()? {
        let expected = quarantine_dir(&journal.protected_root, journal.download_id);
        let paths_are_owned = journal.quarantine_root == expected
            && journal
                .moved
                .iter()
                .zip(0..)
                .all(|((original, staged), index)| {
                    original.starts_with(&journal.protected_root)
                        && *staged == expected.join(index.to_string())
                });
        if !paths_are_owned {
            return Err(ShioError::Other(
                "unsafe delete recovery journal paths".into(),
            ));
        }
        let dir_exists = match std::fs::symlink_metadata(&expected) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => true,
            Ok(_) => {
                return Err(ShioError::Other(
                    "delete recovery quarantine is not a real directory".into(),
                ));
            },
            Err(error) if error.kind() == ErrorKind::NotFound => false,
            Err(error) => return Err(error.into()),
        };
        if db.download_exists(journal.download_id)? {
            for (original, staged) in &journal.moved {
                if std::fs::symlink_metadata(staged).is_err() {
                    continue;
                }
                ensure_real_parents(&journal.protected_root, original)?;
                if std::fs::symlink_metadata(original).is_ok() {
                    return Err(ShioError::DestinationExists(original.clone()));
                }
                std::fs::rename(staged, original)?;
            }
            if dir_exists {
                std::fs::remove_dir(&expected)?;
            }
        } else if dir_exists {
            std::fs::remove_dir_all(&expected)?;
        }
        db.delete_delete_journal(journal.download_id)?;
    }
    Ok(())
}

pub(super) fn remove_http_staging(download: &Download) -> Result<()> {
    let staging = crate::worker::http_staging_root(download);
    ensure_real_parents(&download.save_path, &staging)?;
    match std::fs::symlink_metadata(&staging) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            ignore_not_found(std::fs::remove_dir_all(&staging))
        },
        Ok(_) => Err(ShioError::Other(
            "HTTP staging is not a real directory".into(),
        )),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn quarantine_dir(root: &Path, owner: DownloadId) -> PathBuf {
    root.join(format!(".shio-remove-{owner}"))
}

fn ignore_not_found(result: std::io::Result<()>) -> Result<()> {
    match result {
        Err(error) if error.kind() != ErrorKind::NotFound => Err(error.into()),
        _ => Ok(()),
    }
}

// Refuses to touch `target` through a symlinked or non-directory ancestor below `root`.
fn ensure_real_parents(root: &Path, target: &Path) -> Result<()> {
    let relative = target
        .strip_prefix(root)
        .map_err(|_| ShioError::Other("refusing to delete outside the save path".into()))?;
    let mut current = root.to_path_buf();
    let parents = relative.parent().into_iter().flat_map(Path::components);
    for component in std::iter::once(None).chain(parents.map(Some)) {
        if let Some(component) = component {
            let Component::Normal(component) = component else {
                return Err(ShioError::Other("unsafe delete path".into()));
            };
            current.push(component);
        }
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {},
            Ok(_) => {
                return Err(ShioError::Other(format!(
                    "refusing to delete through non-directory: {}",
                    current.display()
                )));
            },
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restart_rolls_back_a_quarantine_whose_row_survived() {
        let tmp = tempfile::tempdir().unwrap();
        let download = Download::new(
            "https://example.com/payload.bin".into(),
            tmp.path().to_path_buf(),
        );
        let original = tmp.path().join("payload.bin");
        std::fs::write(&original, b"payload").unwrap();
        let db = Database::in_memory().unwrap();
        db.insert_download(&download).unwrap();
        quarantine(
            &db,
            &[Targets {
                owner: download.id,
                root: tmp.path().to_path_buf(),
                files: vec![original.clone()],
                dirs: Vec::new(),
            }],
        )
        .unwrap();
        assert!(!original.exists());

        recover(&db).unwrap();

        assert_eq!(std::fs::read(original).unwrap(), b"payload");
        assert!(!quarantine_dir(tmp.path(), download.id).exists());
        assert!(db.delete_journals().unwrap().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn quarantine_refuses_symlinked_parents() {
        let tmp = tempfile::tempdir().unwrap();
        let outside = tmp.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("payload.bin"), b"keep").unwrap();
        let root = tmp.path().join("root");
        std::fs::create_dir(&root).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("sub")).unwrap();
        let db = Database::in_memory().unwrap();

        let result = quarantine(
            &db,
            &[Targets {
                owner: DownloadId::new(),
                root: root.clone(),
                files: vec![root.join("sub/payload.bin")],
                dirs: vec![root.join("sub")],
            }],
        );

        assert!(result.is_err());
        assert_eq!(std::fs::read(outside.join("payload.bin")).unwrap(), b"keep");
    }

    #[test]
    fn only_the_dialog_named_folder_counts_as_generated() {
        let file = |path: &str| TorrentFile {
            path: path.into(),
            size: 1,
            selected: true,
        };
        for (folder, files, generated) in [
            (
                "Some Game",
                vec![file("Some_Game_--_uploader.example_--_.iso")],
                true,
            ),
            (
                "Some_Game_--_uploader.example_--_",
                vec![file("Some_Game_--_uploader.example_--_.iso")],
                true,
            ),
            (
                "Downloads",
                vec![file("Some_Game_--_uploader.example_--_.iso")],
                false,
            ),
            ("Show", vec![file("Show/a.mkv"), file("Show/b.mkv")], true),
            ("Other", vec![file("Show/a.mkv"), file("Show/b.mkv")], false),
        ] {
            let mut download = Download::new(
                "https://example.com/x.torrent".into(),
                std::path::PathBuf::from("/data").join(folder),
            );
            download.filename = "x".into();
            assert_eq!(
                is_generated_folder(&download, &files),
                generated,
                "{folder}"
            );
        }
    }
}
