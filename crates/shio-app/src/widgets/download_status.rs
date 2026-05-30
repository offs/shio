use crate::style::Palette;
use shio_core::{Download, DownloadStatus};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RowTone {
    Active,
    Uploading,
    Complete,
    Warning,
    Error,
    Muted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DownloadStatusPresentation {
    pub(crate) dot_tone: RowTone,
    pub(crate) bar_tone: RowTone,
    pub(crate) detail: String,
    pub(crate) status: String,
}

impl RowTone {
    pub(crate) const fn color(self, p: &Palette) -> iced::Color {
        match self {
            Self::Active => p.accent,
            Self::Uploading | Self::Complete => p.success,
            Self::Warning => p.warning,
            Self::Error => p.error,
            Self::Muted => p.text_tertiary,
        }
    }
}

pub(crate) fn present(download: &Download) -> DownloadStatusPresentation {
    let tone = status_tone(download.status);
    DownloadStatusPresentation {
        dot_tone: tone,
        bar_tone: tone,
        detail: detail_label(download),
        status: status_label(download),
    }
}

const fn status_tone(status: DownloadStatus) -> RowTone {
    match status {
        DownloadStatus::Downloading
        | DownloadStatus::Starting
        | DownloadStatus::FetchingMetadata => RowTone::Active,
        DownloadStatus::Seeding => RowTone::Uploading,
        DownloadStatus::Completed => RowTone::Complete,
        DownloadStatus::Extracting | DownloadStatus::Paused | DownloadStatus::PasswordRequired => {
            RowTone::Warning
        },
        DownloadStatus::Error | DownloadStatus::ExtractError => RowTone::Error,
        DownloadStatus::Pending | DownloadStatus::Queued | DownloadStatus::Cancelled => {
            RowTone::Muted
        },
    }
}

fn detail_label(download: &Download) -> String {
    match download.status {
        DownloadStatus::Downloading => download_detail(download),
        DownloadStatus::FetchingMetadata => metadata_detail(download),
        DownloadStatus::Seeding => seeding_detail(download),
        DownloadStatus::Extracting => "extracting".to_string(),
        DownloadStatus::PasswordRequired => "password required".to_string(),
        DownloadStatus::Error => download
            .error_message
            .clone()
            .unwrap_or_else(|| "failed".to_string()),
        DownloadStatus::ExtractError => download
            .error_message
            .clone()
            .unwrap_or_else(|| "extract failed".to_string()),
        DownloadStatus::Pending
        | DownloadStatus::Queued
        | DownloadStatus::Starting
        | DownloadStatus::Paused
        | DownloadStatus::Completed
        | DownloadStatus::Cancelled => String::new(),
    }
}

fn download_detail(download: &Download) -> String {
    if download.speed > 0 {
        return shio_core::format_speed(download.speed);
    }

    let Some(torrent) = download.torrent() else {
        return String::new();
    };
    if torrent.peers_connected == 0 {
        return "waiting for peers".to_string();
    }
    "connected, no data".to_string()
}

fn metadata_detail(download: &Download) -> String {
    let Some(torrent) = download.torrent() else {
        return "getting metadata".to_string();
    };

    if torrent.metadata_wait_secs >= 30 {
        return "still looking for metadata".to_string();
    }
    if torrent.peers_connected > 0 {
        return format!("getting metadata · {} peers", torrent.peers_connected);
    }
    "getting metadata".to_string()
}

fn seeding_detail(download: &Download) -> String {
    let Some(torrent) = download.torrent() else {
        return "seeding idle".to_string();
    };

    if torrent.upload_speed > 0 {
        return format!("up {}", shio_core::format_speed(torrent.upload_speed));
    }
    if torrent.peers_connected > 0 {
        return format!(
            "seeding · {} {}",
            torrent.peers_connected,
            peer_label(torrent.peers_connected)
        );
    }
    "seeding idle".to_string()
}

fn status_label(download: &Download) -> String {
    match download.status {
        DownloadStatus::Pending => "pending".to_string(),
        DownloadStatus::Queued => "queued".to_string(),
        DownloadStatus::Starting => "starting".to_string(),
        DownloadStatus::Downloading => download_status(download),
        DownloadStatus::FetchingMetadata => "metadata".to_string(),
        DownloadStatus::Extracting => "extracting".to_string(),
        DownloadStatus::Paused => "paused".to_string(),
        DownloadStatus::Seeding => seeding_status(download),
        DownloadStatus::Completed => "finished".to_string(),
        DownloadStatus::Error => "failed".to_string(),
        DownloadStatus::ExtractError => "extract failed".to_string(),
        DownloadStatus::PasswordRequired => "needs password".to_string(),
        DownloadStatus::Cancelled => "cancelled".to_string(),
    }
}

fn download_status(download: &Download) -> String {
    let eta = shio_core::format_eta(download.downloaded, download.total_size, download.avg_speed);
    if eta.is_empty() {
        "waiting".to_string()
    } else {
        eta
    }
}

fn seeding_status(download: &Download) -> String {
    download.torrent().map_or_else(
        || "seeding".to_string(),
        |torrent| {
            format!(
                "ratio {:.2} · {}",
                torrent.ratio,
                format_seed_time(torrent.seed_elapsed_secs)
            )
        },
    )
}

fn format_seed_time(seconds: u64) -> String {
    let minutes = seconds / 60;
    let hours = minutes / 60;
    if hours > 0 {
        format!("{hours}h {:02}m", minutes % 60)
    } else {
        format!("{minutes}m")
    }
}

const fn peer_label(count: u32) -> &'static str {
    if count == 1 { "peer" } else { "peers" }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shio_core::{Download, DownloadStatus, TorrentSource};
    use std::path::PathBuf;

    #[test]
    fn metadata_copy_escalates_after_waiting_and_with_peers() {
        let mut download = torrent_download();
        download.status = DownloadStatus::FetchingMetadata;

        assert_eq!(present(&download).detail, "getting metadata");

        download.torrent_mut().expect("torrent").peers_connected = 3;
        assert_eq!(present(&download).detail, "getting metadata · 3 peers");

        let torrent = download.torrent_mut().expect("torrent");
        torrent.peers_connected = 0;
        torrent.metadata_wait_secs = 30;
        assert_eq!(present(&download).detail, "still looking for metadata");
    }

    #[test]
    fn torrent_peer_and_seeding_copy_stays_human() {
        let mut download = torrent_download();
        download.status = DownloadStatus::Downloading;

        assert_eq!(present(&download).detail, "waiting for peers");

        download.torrent_mut().expect("torrent").peers_connected = 2;
        assert_eq!(present(&download).detail, "connected, no data");

        download.status = DownloadStatus::Seeding;
        let torrent = download.torrent_mut().expect("torrent");
        torrent.peers_connected = 1;
        torrent.upload_speed = 1024;
        torrent.ratio = 1.25;
        torrent.seed_elapsed_secs = 3720;

        let presentation = present(&download);
        assert_eq!(presentation.detail, "up 1.0 KiB/s");
        assert_eq!(presentation.status, "ratio 1.25 · 1h 02m");

        let torrent = download.torrent_mut().expect("torrent");
        torrent.upload_speed = 0;
        torrent.peers_connected = 2;
        assert_eq!(present(&download).detail, "seeding · 2 peers");

        download.torrent_mut().expect("torrent").peers_connected = 0;
        assert_eq!(present(&download).detail, "seeding idle");
    }

    #[test]
    fn tone_mapping_for_starting_seeding_paused_and_error() {
        let mut download = torrent_download();
        download.status = DownloadStatus::Starting;
        assert_eq!(present(&download).dot_tone, RowTone::Active);
        assert_eq!(present(&download).bar_tone, RowTone::Active);

        download.status = DownloadStatus::Seeding;
        assert_eq!(present(&download).dot_tone, RowTone::Uploading);
        assert_eq!(present(&download).bar_tone, RowTone::Uploading);

        download.status = DownloadStatus::Paused;
        assert_eq!(present(&download).dot_tone, RowTone::Warning);
        assert_eq!(present(&download).bar_tone, RowTone::Warning);

        download.status = DownloadStatus::Error;
        assert_eq!(present(&download).dot_tone, RowTone::Error);
        assert_eq!(present(&download).bar_tone, RowTone::Error);
    }

    fn torrent_download() -> Download {
        Download::try_from_torrent(
            TorrentSource::Magnet(
                "magnet:?xt=urn:btih:cab507494d02ebb1178b38f2e9d7be299c86b862".into(),
            ),
            PathBuf::from("/tmp"),
        )
        .expect("valid magnet")
    }
}
