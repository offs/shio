use super::download_status::{self, DownloadStatusPresentation, RowTone};
use crate::message::{DownloadColumnWidths, Message};
use crate::style::{self, Palette};
use iced::widget::text::Wrapping;
use iced::widget::{
    Space, button, column, container, progress_bar, rich_text, row, span, stack, text,
};
use iced::{Border, Element, Length, Theme};
use iced_aw::ContextMenu;
use shio_core::{Download, DownloadId, DownloadStatus, TorrentSource};

const MAX_FILENAME_CHARS: usize = 34;
const FILENAME_TAIL_CHARS: usize = 10;
const ROW_HEIGHT: f32 = 48.0;
const ACTION_BUTTON_SIZE: f32 = 32.0;
const ACTION_COLUMN_WIDTH: f32 = 76.0;
const COLUMN_GAP_WIDTH: f32 = 8.0;
const ROW_SPACING: f32 = 4.0;
const APPROX_FILENAME_CHAR_WIDTH: f32 = 7.2;

#[derive(Clone, Copy)]
pub(crate) struct NameDisplayOptions {
    pub(crate) carousel: bool,
    pub(crate) carousel_offset: usize,
    pub(crate) name_width: f32,
}

pub(crate) fn view<'a>(
    download: &'a Download,
    selected: bool,
    match_indices: Option<&[u32]>,
    drop_side: Option<crate::app::DropSide>,
    name_options: NameDisplayOptions,
    widths: DownloadColumnWidths,
    p: &'a Palette,
) -> Element<'a, Message> {
    let id = download.id;
    let presentation = download_status::present(download);
    let progress_pct = download.progress_percent() / 100.0;

    let content = row![
        status_dot(&presentation, p),
        name_cell(download, match_indices, name_options, p),
        column_gap(),
        size_cell(download, widths.size, p),
        column_gap(),
        progress_cell(&presentation, progress_pct, widths.progress, p),
        column_gap(),
        speed_cell(&presentation, widths.speed, p),
        column_gap(),
        eta_cell(&presentation, widths.eta, p),
        column_gap(),
        action_cell(download, p),
    ]
    .spacing(ROW_SPACING)
    .align_y(iced::Alignment::Center)
    .padding([10, 16])
    .height(ROW_HEIGHT);

    let wrapped: Element<'_, Message> = if let Some(side) = drop_side {
        stack![
            if selected {
                container(content)
                    .width(Length::Fill)
                    .style(style::row_selected(p))
            } else {
                container(content)
                    .width(Length::Fill)
                    .style(style::download_row_container)
            },
            drop_indicator(side, p),
        ]
        .width(Length::Fill)
        .into()
    } else if selected {
        container(content)
            .width(Length::Fill)
            .style(style::row_selected(p))
            .into()
    } else {
        container(content)
            .width(Length::Fill)
            .style(style::download_row_container)
            .into()
    };

    let status = download.status;
    let pinned = download.pinned;
    let is_torrent = download.kind.is_torrent();
    let can_copy_url = copyable_source(download).is_some();
    let menu_text_primary = p.text_primary;
    let menu_text_tertiary = p.text_tertiary;
    let menu_bg_elevated = p.bg_elevated;
    let menu_border = p.border_default;
    let menu_bg_hover = p.bg_hover;
    let menu_text_secondary = p.text_secondary;
    ContextMenu::new(wrapped, move || {
        context_menu_items(
            id,
            status,
            pinned,
            is_torrent,
            can_copy_url,
            MenuColors {
                text_primary: menu_text_primary,
                text_secondary: menu_text_secondary,
                text_tertiary: menu_text_tertiary,
                bg_elevated: menu_bg_elevated,
                border: menu_border,
                bg_hover: menu_bg_hover,
            },
        )
    })
    .into()
}

fn column_gap<'a>() -> Element<'a, Message> {
    Space::new().width(COLUMN_GAP_WIDTH).into()
}

fn status_dot<'a>(presentation: &DownloadStatusPresentation, p: &Palette) -> Element<'a, Message> {
    let status_color = presentation.dot_tone.color(p);
    container(
        container(Space::new().width(0).height(0))
            .width(8)
            .height(8)
            .style(move |_: &Theme| container::Style {
                background: Some(status_color.into()),
                border: Border {
                    radius: 100.0.into(),
                    ..Border::default()
                },
                ..container::Style::default()
            }),
    )
    .width(28)
    .center_y(Length::Shrink)
    .into()
}

fn name_cell<'a>(
    download: &'a Download,
    match_indices: Option<&[u32]>,
    options: NameDisplayOptions,
    p: &'a Palette,
) -> Element<'a, Message> {
    let display_filename = display_filename(&download.filename, options);
    let filename_el: Element<'_, Message> = match match_indices {
        Some(indices) if !indices.is_empty() && display_filename == download.filename => {
            highlighted_filename(&download.filename, indices, p)
        },
        _ => text(display_filename)
            .size(13)
            .color(p.text_primary)
            .width(Length::Fill)
            .wrapping(Wrapping::None)
            .into(),
    };

    let mut name_row = row![]
        .spacing(6)
        .align_y(iced::Alignment::Center)
        .width(Length::Fill)
        .clip(true);
    if download.pinned {
        name_row = name_row.push(
            iced_fonts::bootstrap::pin_angle_fill()
                .size(11)
                .color(p.text_tertiary),
        );
    }
    name_row = name_row.push(filename_el);

    container(name_row)
        .width(Length::Fixed(options.name_width))
        .clip(true)
        .into()
}

fn size_cell<'a>(download: &'a Download, width: f32, p: &Palette) -> Element<'a, Message> {
    let size_str = match download.total_size {
        Some(total) => format_size_pair(download.downloaded, total),
        None => format_size_compact(download.downloaded),
    };
    text(size_str)
        .size(12)
        .color(p.text_secondary)
        .wrapping(Wrapping::None)
        .width(Length::Fixed(width))
        .into()
}

fn truncated_filename(filename: &str, max_chars: usize) -> String {
    if filename.chars().count() <= max_chars {
        return filename.to_string();
    }

    let head_len = max_chars.saturating_sub(FILENAME_TAIL_CHARS + 3).max(1);
    let head: String = filename.chars().take(head_len).collect();
    let mut tail: Vec<char> = filename.chars().rev().take(FILENAME_TAIL_CHARS).collect();
    tail.reverse();
    format!("{head}...{}", tail.into_iter().collect::<String>())
}

fn display_filename(filename: &str, options: NameDisplayOptions) -> String {
    let max_chars = max_filename_chars(options.name_width);
    if options.carousel {
        return carousel_filename(filename, options.carousel_offset, max_chars);
    }
    truncated_filename(filename, max_chars)
}

fn carousel_filename(filename: &str, offset: usize, max_chars: usize) -> String {
    let chars: Vec<char> = filename.chars().collect();
    if chars.len() <= max_chars {
        return filename.to_string();
    }

    let head_len = max_chars.saturating_sub(FILENAME_TAIL_CHARS + 3).max(1);
    let scrollable_len = chars.len() - FILENAME_TAIL_CHARS;
    let start = offset % scrollable_len;
    let visible: String = (0..head_len)
        .map(|i| chars[(start + i) % scrollable_len])
        .collect();
    let visible = visible.trim_end_matches('.');
    let tail: String = chars[chars.len() - FILENAME_TAIL_CHARS..].iter().collect();
    format!("{visible}...{tail}")
}

fn max_filename_chars(width: f32) -> usize {
    ((width / APPROX_FILENAME_CHAR_WIDTH).floor() as usize).max(MAX_FILENAME_CHARS)
}

fn format_size_compact(bytes: u64) -> String {
    bytesize::ByteSize(bytes).to_string()
}

fn split_size_unit(value: &str) -> Option<(&str, &str)> {
    value
        .rsplit_once(' ')
        .filter(|(amount, unit)| !amount.is_empty() && !unit.is_empty())
}

fn format_size_pair(downloaded: u64, total: u64) -> String {
    let downloaded = format_size_compact(downloaded);
    let total = format_size_compact(total);
    match (split_size_unit(&downloaded), split_size_unit(&total)) {
        (Some((downloaded_amount, downloaded_unit)), Some((_, total_unit)))
            if downloaded_unit == total_unit =>
        {
            format!("{downloaded_amount} / {total}")
        },
        _ => format!("{downloaded} / {total}"),
    }
}

fn progress_cell<'a>(
    presentation: &DownloadStatusPresentation,
    pct: f32,
    width: f32,
    p: &Palette,
) -> Element<'a, Message> {
    let pct = pct.clamp(0.0, 1.0);
    let bar_color = presentation.bar_tone.color(p);
    let bg = p.progress_bg;
    container(progress_bar(0.0..=1.0, pct).style(move |_: &Theme| {
        iced::widget::progress_bar::Style {
            background: bg.into(),
            bar: bar_color.into(),
            border: Border {
                radius: 2.0.into(),
                ..Border::default()
            },
        }
    }))
    .width(Length::Fixed(width))
    .height(4)
    .into()
}

fn speed_cell<'a>(
    presentation: &DownloadStatusPresentation,
    width: f32,
    p: &Palette,
) -> Element<'a, Message> {
    text(presentation.detail.clone())
        .size(12)
        .color(detail_color(presentation, p))
        .wrapping(Wrapping::None)
        .width(Length::Fixed(width))
        .into()
}

const fn detail_color(presentation: &DownloadStatusPresentation, p: &Palette) -> iced::Color {
    match presentation.dot_tone {
        RowTone::Warning => p.warning,
        RowTone::Error => p.error,
        RowTone::Active | RowTone::Uploading | RowTone::Complete | RowTone::Muted => {
            p.text_secondary
        },
    }
}

fn eta_cell<'a>(
    presentation: &DownloadStatusPresentation,
    width: f32,
    p: &Palette,
) -> Element<'a, Message> {
    text(presentation.status.clone())
        .size(12)
        .color(p.text_tertiary)
        .width(Length::Fixed(width))
        .into()
}

fn action_cell<'a>(download: &'a Download, p: &'a Palette) -> Element<'a, Message> {
    let id = download.id;
    row![
        action_button(download, p),
        button(iced_fonts::bootstrap::x_lg().size(12))
            .style(style::btn_icon(p))
            .on_press(Message::RemoveDownload(id))
            .padding(0)
            .width(Length::Fixed(ACTION_BUTTON_SIZE))
            .height(Length::Fixed(ACTION_BUTTON_SIZE)),
    ]
    .spacing(4)
    .align_y(iced::Alignment::Center)
    .width(Length::Fixed(ACTION_COLUMN_WIDTH))
    .into()
}

fn action_button<'a>(download: &'a Download, p: &'a Palette) -> Element<'a, Message> {
    let id = download.id;
    match download.status {
        DownloadStatus::Downloading
        | DownloadStatus::Starting
        | DownloadStatus::FetchingMetadata => button(iced_fonts::bootstrap::pause_fill().size(12))
            .style(style::btn_ghost(p))
            .on_press(Message::PauseDownload(id))
            .padding(0)
            .width(Length::Fixed(ACTION_BUTTON_SIZE))
            .height(Length::Fixed(ACTION_BUTTON_SIZE))
            .into(),
        DownloadStatus::Seeding => button(iced_fonts::bootstrap::stop_fill().size(12))
            .style(style::btn_ghost(p))
            .on_press(Message::StopSeeding(id))
            .padding(0)
            .width(Length::Fixed(ACTION_BUTTON_SIZE))
            .height(Length::Fixed(ACTION_BUTTON_SIZE))
            .into(),
        DownloadStatus::Paused => button(iced_fonts::bootstrap::play_fill().size(12))
            .style(style::btn_ghost(p))
            .on_press(Message::ResumeDownload(id))
            .padding(0)
            .width(Length::Fixed(ACTION_BUTTON_SIZE))
            .height(Length::Fixed(ACTION_BUTTON_SIZE))
            .into(),
        DownloadStatus::Error => button(iced_fonts::bootstrap::arrow_clockwise().size(12))
            .style(style::btn_ghost(p))
            .on_press(Message::RetryDownload(id))
            .padding(0)
            .width(Length::Fixed(ACTION_BUTTON_SIZE))
            .height(Length::Fixed(ACTION_BUTTON_SIZE))
            .into(),
        DownloadStatus::ExtractError => button(iced_fonts::bootstrap::arrow_clockwise().size(12))
            .style(style::btn_ghost(p))
            .on_press(Message::RetryExtract(id))
            .padding(0)
            .width(Length::Fixed(ACTION_BUTTON_SIZE))
            .height(Length::Fixed(ACTION_BUTTON_SIZE))
            .into(),
        DownloadStatus::PasswordRequired => button(iced_fonts::bootstrap::key().size(12))
            .style(style::btn_ghost(p))
            .on_press(Message::RequestPassword(id))
            .padding(0)
            .width(Length::Fixed(ACTION_BUTTON_SIZE))
            .height(Length::Fixed(ACTION_BUTTON_SIZE))
            .into(),
        DownloadStatus::Completed => button(iced_fonts::bootstrap::foldertwo_open().size(12))
            .style(style::btn_ghost(p))
            .on_press(Message::OpenFolder(id))
            .padding(0)
            .width(Length::Fixed(ACTION_BUTTON_SIZE))
            .height(Length::Fixed(ACTION_BUTTON_SIZE))
            .into(),
        _ => Space::new().width(ACTION_BUTTON_SIZE).into(),
    }
}

fn drop_indicator<'a>(side: crate::app::DropSide, p: &Palette) -> Element<'a, Message> {
    let accent = p.accent;
    let line: Element<'_, Message> = container(Space::new().height(2))
        .width(Length::Fill)
        .style(move |_: &Theme| container::Style {
            background: Some(accent.into()),
            ..container::Style::default()
        })
        .into();
    let align = match side {
        crate::app::DropSide::Below => iced::Alignment::End,
        crate::app::DropSide::Above => iced::Alignment::Start,
    };
    container(line)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_y(align)
        .into()
}

#[derive(Clone, Copy)]
struct MenuColors {
    text_primary: iced::Color,
    text_secondary: iced::Color,
    text_tertiary: iced::Color,
    bg_elevated: iced::Color,
    border: iced::Color,
    bg_hover: iced::Color,
}

fn context_menu_items(
    id: DownloadId,
    status: DownloadStatus,
    pinned: bool,
    is_torrent: bool,
    can_copy_url: bool,
    colors: MenuColors,
) -> Element<'static, Message> {
    let menu_btn =
        |label: &str, shortcut: Option<&str>, msg: Message| -> Element<'static, Message> {
            let label_text = text(label.to_string()).size(12).color(colors.text_primary);
            let mut content = row![label_text, Space::new().width(Length::Fill)]
                .spacing(12)
                .align_y(iced::Alignment::Center);
            if let Some(hint) = shortcut {
                content = content.push(text(hint.to_string()).size(11).color(colors.text_tertiary));
            }
            let bg_hover = colors.bg_hover;
            let text_primary = colors.text_primary;
            let text_secondary = colors.text_secondary;
            button(content)
                .style(move |_theme: &Theme, status: button::Status| {
                    let base = button::Style {
                        background: None,
                        text_color: text_secondary,
                        border: Border {
                            radius: 4.0.into(),
                            ..Border::default()
                        },
                        ..button::Style::default()
                    };
                    match status {
                        button::Status::Hovered => button::Style {
                            background: Some(bg_hover.into()),
                            text_color: text_primary,
                            ..base
                        },
                        _ => base,
                    }
                })
                .on_press(msg)
                .padding([6, 16])
                .width(Length::Fill)
                .into()
        };

    let mut items: Vec<Element<'static, Message>> = Vec::new();

    items.push(menu_btn(
        if pinned { "unpin" } else { "pin to top" },
        Some("P"),
        Message::TogglePin(id),
    ));

    match status {
        DownloadStatus::Downloading
        | DownloadStatus::Starting
        | DownloadStatus::FetchingMetadata => {
            items.push(menu_btn("pause", Some("Space"), Message::PauseDownload(id)));
        },
        DownloadStatus::Seeding => {
            items.push(menu_btn("stop seeding", None, Message::StopSeeding(id)));
        },
        DownloadStatus::Paused => {
            items.push(menu_btn(
                "resume",
                Some("Space"),
                Message::ResumeDownload(id),
            ));
        },
        DownloadStatus::Error => {
            items.push(menu_btn("retry", Some("R"), Message::RetryDownload(id)));
        },
        DownloadStatus::ExtractError => {
            items.push(menu_btn(
                "retry extract",
                Some("R"),
                Message::RetryExtract(id),
            ));
        },
        DownloadStatus::PasswordRequired => {
            items.push(menu_btn(
                "enter password…",
                Some("R"),
                Message::RequestPassword(id),
            ));
        },
        _ => {},
    }

    if is_torrent
        && matches!(
            status,
            DownloadStatus::Paused
                | DownloadStatus::Completed
                | DownloadStatus::Seeding
                | DownloadStatus::Error
        )
    {
        items.push(menu_btn("force recheck", None, Message::ForceRecheck(id)));
    }

    items.push(menu_btn("cancel", Some("C"), Message::CancelDownload(id)));
    if can_copy_url {
        items.push(menu_btn("copy url", Some("U"), Message::CopyUrl(id)));
    }
    items.push(menu_btn("edit", Some("E"), Message::RequestEdit(id)));

    if can_open_file_from_menu(status) {
        items.push(menu_btn("open file", Some("Enter"), Message::OpenFile(id)));
    }
    if can_open_folder_from_menu(status) || (is_torrent && torrent_can_open_folder(status)) {
        items.push(menu_btn("open folder", Some("F"), Message::OpenFolder(id)));
    }

    items.push(menu_btn(
        "delete",
        Some("Del"),
        Message::RequestDeleteWithFiles(id),
    ));

    let bg_elevated = colors.bg_elevated;
    let border = colors.border;
    container(column(items).width(160))
        .style(move |_: &Theme| container::Style {
            background: Some(bg_elevated.into()),
            border: Border {
                radius: 6.0.into(),
                width: 1.0,
                color: border,
            },
            ..container::Style::default()
        })
        .padding(4)
        .into()
}

const fn can_open_file_from_menu(status: DownloadStatus) -> bool {
    matches!(
        status,
        DownloadStatus::Completed | DownloadStatus::ExtractError | DownloadStatus::PasswordRequired
    )
}

const fn can_open_folder_from_menu(status: DownloadStatus) -> bool {
    can_open_file_from_menu(status)
}

const fn torrent_can_open_folder(status: DownloadStatus) -> bool {
    matches!(status, DownloadStatus::Seeding)
}

pub(crate) fn copyable_source(download: &Download) -> Option<&str> {
    if let Some(url) = download.url() {
        return Some(url);
    }
    let source = &download.torrent()?.source;
    match source {
        TorrentSource::Magnet(magnet) => Some(magnet.as_str()),
        TorrentSource::File(_) => None,
    }
}

fn highlighted_filename<'a>(filename: &str, indices: &[u32], p: &Palette) -> Element<'a, Message> {
    let chars: Vec<char> = filename.chars().collect();
    let max = indices.last().copied().unwrap_or(0) as usize;
    let mask_len = chars.len().max(max + 1);
    let mut mask = vec![false; mask_len];
    for &idx in indices {
        let i = idx as usize;
        if i < mask.len() {
            mask[i] = true;
        }
    }

    let mut spans = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let is_match = mask.get(i).copied().unwrap_or(false);
        let start = i;
        while i < chars.len() && mask.get(i).copied().unwrap_or(false) == is_match {
            i += 1;
        }
        let chunk: String = chars[start..i].iter().collect();
        let s: iced::widget::text::Span<'_> = if is_match {
            span(chunk).color(p.accent)
        } else {
            span(chunk).color(p.text_primary)
        };
        spans.push(s);
    }

    rich_text(spans)
        .size(13)
        .width(Length::Fill)
        .wrapping(Wrapping::WordOrGlyph)
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn sample_torrent_bytes() -> Vec<u8> {
        let pieces = b"abcdefghijklmnopqrst";
        let mut bytes = b"d8:announce35:udp://tracker.example:1337/announce4:infod5:filesld6:lengthi123e4:pathl7:foo.txteed6:lengthi456e4:pathl3:bar7:baz.mkveee4:name7:release12:piece lengthi16384e6:pieces20:".to_vec();
        bytes.extend_from_slice(pieces);
        bytes.extend_from_slice(b"ee");
        bytes
    }

    #[test]
    fn size_pair_uses_one_shared_unit() {
        assert_eq!(
            format_size_pair(263_600_000, 263_600_000),
            "251.4 / 251.4 MiB"
        );
    }

    #[test]
    fn long_filename_is_truncated_before_layout() {
        assert_eq!(
            truncated_filename(
                "shio-portable-update-1.0.2-to-1.0.3-windows-x64.rar",
                MAX_FILENAME_CHARS
            ),
            "shio-portable-update-...ws-x64.rar"
        );
    }

    #[test]
    fn display_filename_uses_more_text_when_name_column_is_wider() {
        let filename = "shio-portable-update-1.0.2-to-1.0.3-windows-x64.rar";

        assert_eq!(
            display_filename(
                filename,
                NameDisplayOptions {
                    carousel: false,
                    carousel_offset: 0,
                    name_width: 520.0,
                },
            ),
            filename
        );
    }

    #[test]
    fn display_filename_scrolls_collapsed_long_names_when_enabled() {
        let filename = "shio-portable-update-1.0.2-to-1.0.3-windows-x64.rar";

        assert_eq!(
            display_filename(
                filename,
                NameDisplayOptions {
                    carousel: true,
                    carousel_offset: 8,
                    name_width: 280.0,
                },
            ),
            "table-update-1.0.2-to-1.0...ws-x64.rar"
        );
    }

    #[test]
    fn copyable_source_handles_torrent_sources() {
        let download = Download::try_from_torrent(
            TorrentSource::File(sample_torrent_bytes()),
            PathBuf::from("/tmp"),
        )
        .expect("torrent");
        assert_eq!(copyable_source(&download), None);

        let magnet = "magnet:?xt=urn:btih:cab507494d02ebb1178b38f2e9d7be299c86b862";
        let download =
            Download::try_from_torrent(TorrentSource::Magnet(magnet.into()), PathBuf::from("/tmp"))
                .expect("valid magnet");
        assert_eq!(copyable_source(&download), Some(magnet));
    }

    #[test]
    fn seeding_torrents_can_open_folder_from_menu() {
        assert!(torrent_can_open_folder(DownloadStatus::Seeding));
    }
}
