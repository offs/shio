use std::collections::HashMap;
use std::path::PathBuf;

use crate::error::{Result, ShioError};
use crate::types::{TorrentFile, TorrentProgressSnapshot};

pub(crate) fn selected_file_indexes(files: &[TorrentFile]) -> Result<Option<Vec<usize>>> {
    if files.is_empty() {
        return Ok(None);
    }
    let selected = files
        .iter()
        .enumerate()
        .filter_map(|(index, file)| file.selected.then_some(index))
        .collect::<Vec<_>>();
    if selected.is_empty() {
        return Err(ShioError::Other(
            "torrent must have at least one selected file".into(),
        ));
    }
    if selected.len() == files.len() {
        Ok(None)
    } else {
        Ok(Some(selected))
    }
}

pub(crate) fn selected_size(files: &[TorrentFile]) -> u64 {
    selected_bytes(files)
}

pub(crate) fn selected_bytes(files: &[TorrentFile]) -> u64 {
    files
        .iter()
        .filter(|file| file.selected)
        .map(|file| file.size)
        .sum()
}

pub(crate) fn upload_ratio(uploaded_bytes: u64, selected_bytes: u64) -> f32 {
    if selected_bytes == 0 {
        return 0.0;
    }
    (uploaded_bytes as f64 / selected_bytes as f64) as f32
}

pub(crate) fn snapshot_with_selection(
    snapshot: &TorrentProgressSnapshot,
    existing_files: &[TorrentFile],
) -> TorrentProgressSnapshot {
    let existing_selection = existing_files
        .iter()
        .map(|file| (file.path.clone(), file.selected))
        .collect::<HashMap<PathBuf, bool>>();
    let files = snapshot
        .files
        .iter()
        .map(|file| {
            let mut file = file.clone();
            if let Some(selected) = existing_selection.get(&file.path) {
                file.selected = *selected;
            }
            file
        })
        .collect();

    TorrentProgressSnapshot {
        is_private: snapshot.is_private,
        files,
        trackers: snapshot.trackers.clone(),
    }
}
