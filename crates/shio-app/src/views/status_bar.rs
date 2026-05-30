use crate::message::Message;
use crate::style;
use iced::widget::{Space, container, row, text};
use iced::{Element, Length};
use shio_core::{AppConfig, Download, DownloadStatus};

pub(crate) fn view<'a>(
    downloads: &[Download],
    config: &AppConfig,
    p: &'a crate::style::Palette,
) -> Element<'a, Message> {
    let segments = status_segments(downloads, config);
    let Some((clipboard_label, main_segments)) = segments.split_last() else {
        return Space::new().into();
    };

    let mut content = row![Space::new().width(16)];
    for (index, segment) in main_segments.iter().enumerate() {
        if index > 0 {
            content = content
                .push(Space::new().width(16))
                .push(text("\u{00B7}").size(11).color(p.text_ghost))
                .push(Space::new().width(16));
        }
        content = content.push(text(segment.clone()).size(11).color(p.text_ghost));
    }

    let content = content
        .push(Space::new().width(Length::Fill))
        .push(text(clipboard_label.clone()).size(11).color(p.text_ghost))
        .push(Space::new().width(16))
        .align_y(iced::Alignment::Center)
        .height(24);

    container(content)
        .style(style::status_bar(p))
        .width(Length::Fill)
        .into()
}

fn status_segments(downloads: &[Download], config: &AppConfig) -> Vec<String> {
    let total = downloads.len();
    let transferring = downloads
        .iter()
        .filter(|d| {
            matches!(
                d.status,
                DownloadStatus::Downloading | DownloadStatus::Seeding
            )
        })
        .count();
    let preparing = downloads
        .iter()
        .filter(|d| {
            matches!(
                d.status,
                DownloadStatus::Starting
                    | DownloadStatus::Extracting
                    | DownloadStatus::FetchingMetadata
            )
        })
        .count();
    let seeding = downloads
        .iter()
        .filter(|d| d.status == DownloadStatus::Seeding)
        .count();
    let has_torrents = downloads.iter().any(|d| d.kind.is_torrent());
    let max = config.max_concurrent;

    let download_speed: u64 = downloads
        .iter()
        .filter(|d| d.status == DownloadStatus::Downloading)
        .map(|d| {
            if d.avg_speed > 0 {
                d.avg_speed
            } else {
                d.speed
            }
        })
        .sum();
    let upload_speed: u64 = downloads
        .iter()
        .filter(|d| d.status == DownloadStatus::Seeding)
        .map(|d| d.torrent().map_or(d.speed, |torrent| torrent.upload_speed))
        .sum();

    let speed = match (download_speed, upload_speed) {
        (0, 0) => String::new(),
        (down, 0) => format!("down {}", shio_core::format_speed(down)),
        (0, up) => format!("up {}", shio_core::format_speed(up)),
        (down, up) => format!(
            "down {} · up {}",
            shio_core::format_speed(down),
            shio_core::format_speed(up)
        ),
    };

    let clipboard_label = if config.clipboard_monitor {
        "clipboard: on"
    } else {
        "clipboard: off"
    };

    let mut segments = vec![
        format!("{total} downloads"),
        format!("{transferring}/{max} transferring"),
    ];

    if preparing > 0 {
        segments.push(format!("{preparing} preparing"));
    }

    if has_torrents {
        segments.push(format!("{seeding} seeding"));
    }

    if !speed.is_empty() {
        segments.push(speed);
    }

    segments.push(clipboard_label.to_string());
    segments
}

#[cfg(test)]
mod tests {
    use super::*;
    use shio_core::TorrentSource;
    use std::path::PathBuf;

    #[test]
    fn status_segments_include_torrent_summary_once() {
        let mut download = Download::try_from_torrent(
            TorrentSource::Magnet(
                "magnet:?xt=urn:btih:cab507494d02ebb1178b38f2e9d7be299c86b862".into(),
            ),
            PathBuf::from("/tmp"),
        )
        .expect("valid magnet");
        download.status = DownloadStatus::Seeding;

        assert_eq!(
            status_segments(&[download], &AppConfig::default()),
            vec![
                "1 downloads".to_string(),
                "1/3 transferring".to_string(),
                "1 seeding".to_string(),
                "clipboard: on".to_string(),
            ]
        );
    }

    #[test]
    fn status_segments_put_clipboard_last() {
        let config = AppConfig {
            clipboard_monitor: false,
            ..AppConfig::default()
        };

        assert_eq!(
            status_segments(&[], &config),
            vec![
                "0 downloads".to_string(),
                "0/3 transferring".to_string(),
                "clipboard: off".to_string(),
            ]
        );
    }
}
