use super::{hint_on, icon_button, pill};
use crate::message::Message;
use crate::style::{self, ICON_BUTTON, Palette, SPACE_M, SPACE_S, SPACE_XS};
use bytesize::ByteSize;
use iced::widget::text::Wrapping;
use iced::widget::{
    Space, column, container, hover, lazy, mouse_area, progress_bar, row, text, tooltip,
};
use iced::{Alignment, Color, Element, Length, Padding, Point};
use iced_fonts::bootstrap;
use shio_core::{DownloadId, DownloadStatus, TorrentStats};
use std::borrow::Cow;
use std::hash::{Hash, Hasher};

const PADDING: [f32; 2] = [7.0, SPACE_M];
const ACTIONS_WIDTH: f32 = 3.0 * ICON_BUTTON + 2.0 * SPACE_XS;
const DOT: f32 = 6.0;
const NAME_INSET: f32 = DOT + SPACE_S;
const NAME_CHAR_WIDTH: f32 = 7.2;
const ROW_CHROME: f32 = 2.0 * (SPACE_XS + SPACE_M) + NAME_INSET + SPACE_S + ACTIONS_WIDTH;

/// The `lazy` key: every field `build` reads must be hashed.
pub(crate) struct RowView<'a> {
    pub(crate) id: DownloadId,
    pub(crate) name: &'a str,
    pub(crate) error: Option<&'a str>,
    pub(crate) status: DownloadStatus,
    pub(crate) downloaded: u64,
    pub(crate) total: Option<u64>,
    pub(crate) speed: u64,
    pub(crate) torrent: Option<TorrentStats>,
    pub(crate) pinned: bool,
    pub(crate) selected: bool,
    pub(crate) name_budget: usize,
    pub(crate) palette: &'a Palette,
}

impl Hash for RowView<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
        self.name.hash(state);
        self.error.hash(state);
        std::mem::discriminant(&self.status).hash(state);
        self.downloaded.hash(state);
        self.total.hash(state);
        self.speed.hash(state);
        if let Some(t) = self.torrent {
            (
                t.peers,
                t.upload_speed,
                t.ratio.to_bits(),
                t.metadata_wait_secs,
            )
                .hash(state);
        }
        self.pinned.hash(state);
        self.selected.hash(state);
        self.name_budget.hash(state);
        self.palette.hash(state);
    }
}

pub(crate) fn name_budget(width: f32) -> usize {
    ((width - ROW_CHROME).max(0.0) / NAME_CHAR_WIDTH) as usize
}

pub(crate) fn view(row: RowView<'_>) -> Element<'_, Message> {
    lazy(row, build).into()
}

fn build(r: &RowView<'_>) -> Element<'static, Message> {
    let p = *r.palette;
    let id = r.id;
    let tone = tone(r.status, &p);

    let label = |name: String| {
        text(name)
            .size(style::TEXT_BODY)
            .color(p.text_primary)
            .wrapping(Wrapping::None)
    };
    let name: Element<'static, Message> = match middle_ellipsis(r.name, r.name_budget) {
        Cow::Borrowed(name) => label(name.to_owned()).into(),
        Cow::Owned(short) => hint_on(label(short), r.name.to_owned(), tooltip::Position::Top, &p),
    };
    let title = row![pill(tone, DOT, DOT)]
        .push(
            r.pinned
                .then(|| bootstrap::pin_angle_fill().size(10).color(p.text_tertiary)),
        )
        .push(container(name).width(Length::Fill).clip(true))
        .spacing(SPACE_S)
        .align_y(Alignment::Center);

    let detail = text(detail(r))
        .size(style::TEXT_SMALL)
        .color(if r.selected {
            p.text_secondary
        } else {
            p.text_tertiary
        })
        .wrapping(Wrapping::None);
    let detail: Element<'static, Message> = match r.error {
        Some(error) if r.status.is_failed() => {
            hint_on(detail, error.to_owned(), tooltip::Position::Bottom, &p)
        },
        _ => detail.into(),
    };
    let bar: Element<'static, Message> = match progress(r) {
        Some(fraction) => progress_bar(0.0..=1.0, fraction)
            .girth(2.0)
            .style(style::progress(p, tone))
            .into(),
        None => Space::new().height(2).into(),
    };
    let lower = column![container(detail).width(Length::Fill).clip(true), bar].spacing(SPACE_XS);

    let text_column = column![
        title,
        container(lower).padding(Padding::ZERO.left(NAME_INSET))
    ]
    .spacing(2)
    .width(Length::Fill);
    let trailing = if r.selected {
        actions(r, &p)
    } else {
        Space::new().width(ACTIONS_WIDTH).into()
    };
    let base = container(
        row![text_column, trailing]
            .spacing(SPACE_S)
            .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .padding(PADDING)
    .style(style::row(p, r.selected));

    let overlay = container((!r.selected).then(|| actions(r, &p)))
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(PADDING)
        .align_x(Alignment::End)
        .align_y(Alignment::Center);

    mouse_area(hover(base, overlay))
        .on_press(Message::RowPressed(id))
        .on_double_click(Message::RowDoubleClicked(id))
        .on_right_press(Message::OpenRowMenu(id, Point::ORIGIN))
        .into()
}

fn actions(r: &RowView<'_>, p: &Palette) -> Element<'static, Message> {
    let id = r.id;
    let primary = match r.status {
        DownloadStatus::Pending
        | DownloadStatus::Queued
        | DownloadStatus::Starting
        | DownloadStatus::Downloading
        | DownloadStatus::FetchingMetadata => {
            Some((bootstrap::pause_fill(), "pause", Message::Pause(id)))
        },
        DownloadStatus::Paused | DownloadStatus::Cancelled => {
            Some((bootstrap::play_fill(), "resume", Message::Resume(id)))
        },
        DownloadStatus::Error => Some((bootstrap::arrow_clockwise(), "retry", Message::Retry(id))),
        DownloadStatus::ExtractError => Some((
            bootstrap::arrow_clockwise(),
            "retry extraction",
            Message::RetryExtract(id),
        )),
        DownloadStatus::PasswordRequired => Some((
            bootstrap::key(),
            "enter password",
            Message::RequestPassword(id),
        )),
        DownloadStatus::Seeding => Some((
            bootstrap::stop_fill(),
            "stop seeding",
            Message::StopSeeding(id),
        )),
        DownloadStatus::Extracting | DownloadStatus::Completed => None,
    };
    let primary = match primary {
        Some((icon, hint, message)) => icon_button(icon, hint, Some(message), p),
        None => Space::new().width(ICON_BUTTON).into(),
    };
    row![
        primary,
        icon_button(
            bootstrap::foldertwo_open(),
            "open folder",
            Some(Message::OpenFolder(id)),
            p,
        ),
        icon_button(
            bootstrap::trash(),
            "remove…",
            Some(Message::RequestRemove(id)),
            p,
        ),
    ]
    .spacing(SPACE_XS)
    .align_y(Alignment::Center)
    .into()
}

const fn tone(status: DownloadStatus, p: &Palette) -> Color {
    match status {
        DownloadStatus::Starting
        | DownloadStatus::Downloading
        | DownloadStatus::FetchingMetadata
        | DownloadStatus::Extracting => p.accent,
        DownloadStatus::Seeding | DownloadStatus::Completed => p.success,
        DownloadStatus::PasswordRequired => p.warning,
        DownloadStatus::Error | DownloadStatus::ExtractError => p.error,
        DownloadStatus::Pending
        | DownloadStatus::Queued
        | DownloadStatus::Paused
        | DownloadStatus::Cancelled => p.text_tertiary,
    }
}

fn progress(r: &RowView<'_>) -> Option<f32> {
    let working = matches!(
        r.status,
        DownloadStatus::Pending
            | DownloadStatus::Queued
            | DownloadStatus::Starting
            | DownloadStatus::FetchingMetadata
            | DownloadStatus::Downloading
            | DownloadStatus::Paused
            | DownloadStatus::Extracting
    );
    let total = r.total.filter(|&total| working && total > 0)?;
    Some((r.downloaded as f64 / total as f64).min(1.0) as f32)
}

struct Size(u64, Option<u64>);

impl std::fmt::Display for Size {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.1 {
            Some(total) => write!(f, "{} of {}", ByteSize(self.0), ByteSize(total)),
            None => write!(f, "{}", ByteSize(self.0)),
        }
    }
}

fn detail(r: &RowView<'_>) -> String {
    let size = Size(r.downloaded, r.total);
    let failed = |label: &str| match r.error.and_then(|error| error.lines().next()) {
        Some(error) => format!("{label} · {error}"),
        None => label.to_owned(),
    };
    match r.status {
        DownloadStatus::Starting => "starting".to_owned(),
        DownloadStatus::Downloading => match (r.torrent, r.speed) {
            (Some(t), 0) if t.peers == 0 => format!("{size} · finding peers"),
            (_, 0) => format!("{size} · waiting for data"),
            (_, speed) => {
                let left = r
                    .total
                    .and_then(|total| eta(total.saturating_sub(r.downloaded), speed));
                match left {
                    Some(left) => format!("{size} · {}/s · {left}", ByteSize(speed)),
                    None => format!("{size} · {}/s", ByteSize(speed)),
                }
            },
        },
        DownloadStatus::FetchingMetadata => match r.torrent {
            Some(t) if t.peers > 0 => format!("getting metadata · {} peers", t.peers),
            Some(t) if t.metadata_wait_secs >= 30 => "getting metadata · no peers yet".to_owned(),
            _ => "getting metadata".to_owned(),
        },
        DownloadStatus::Pending | DownloadStatus::Queued if r.downloaded == 0 => {
            "queued".to_owned()
        },
        DownloadStatus::Pending | DownloadStatus::Queued => format!("queued · {size}"),
        DownloadStatus::Paused => format!("paused · {size}"),
        DownloadStatus::Cancelled => format!("stopped · {size}"),
        DownloadStatus::Extracting => "extracting".to_owned(),
        DownloadStatus::PasswordRequired => "needs the archive password".to_owned(),
        DownloadStatus::Seeding => match r.torrent {
            Some(t) => format!(
                "seeding · ↑ {}/s · ratio {:.2}",
                ByteSize(t.upload_speed),
                t.ratio
            ),
            None => "seeding".to_owned(),
        },
        DownloadStatus::Completed => {
            format!("done · {}", ByteSize(r.total.unwrap_or(r.downloaded)))
        },
        DownloadStatus::Error => failed("failed"),
        DownloadStatus::ExtractError => failed("extraction failed"),
    }
}

fn eta(remaining: u64, speed: u64) -> Option<String> {
    if speed == 0 || remaining == 0 {
        return None;
    }
    let secs = remaining.div_ceil(speed);
    let (days, hours, minutes) = (secs / 86_400, secs / 3_600 % 24, secs / 60 % 60);
    Some(match (days, hours, minutes) {
        (0, 0, 0) => "less than a minute left".to_owned(),
        (0, 0, m) => format!("{m} min left"),
        (0, h, 0) => format!("{h} h left"),
        (0, h, m) => format!("{h} h {m} min left"),
        (d, h, _) if h > 0 => format!("{d} d {h} h left"),
        (d, _, _) => format!("{d} d left"),
    })
}

/// Keeps the tail so the extension and any part number stay visible; wide chars take two cells.
fn middle_ellipsis(name: &str, budget: usize) -> Cow<'_, str> {
    let cells = |c: char| if is_wide(c) { 2 } else { 1 };
    let width: usize = name.chars().map(cells).sum();
    if width <= budget {
        return Cow::Borrowed(name);
    }
    let room = budget.saturating_sub(1).max(2);
    let extension: usize = name
        .rfind('.')
        .map_or(0, |dot| name[dot..].chars().map(cells).sum());
    let tail_room = (room / 3).max(extension + 2).min(room - 1);
    let head_room = room - tail_room;

    let mut used = 0;
    let head_end = name
        .char_indices()
        .find(|&(_, c)| {
            used += cells(c);
            used > head_room
        })
        .map_or(name.len(), |(i, _)| i);
    let mut used = 0;
    let tail_start = name
        .char_indices()
        .rev()
        .find(|&(_, c)| {
            used += cells(c);
            used > tail_room
        })
        .map_or(0, |(i, c)| i + c.len_utf8());
    Cow::Owned(format!(
        "{}…{}",
        name[..head_end].trim_end(),
        name[tail_start.max(head_end)..].trim_start()
    ))
}

const fn is_wide(c: char) -> bool {
    matches!(
        c as u32,
        0x1100..=0x115F
            | 0x2E80..=0xA4CF
            | 0xAC00..=0xD7A3
            | 0xF900..=0xFAFF
            | 0xFE30..=0xFE4F
            | 0xFF00..=0xFF60
            | 0xFFE0..=0xFFE6
            | 0x1F300..=0x1FAFF
            | 0x20000..=0x3FFFD
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cells(text: &str) -> usize {
        text.chars().map(|c| if is_wide(c) { 2 } else { 1 }).sum()
    }

    #[test]
    fn middle_ellipsis_fits_budget_and_keeps_the_extension() {
        for (name, budget) in [
            ("shio-portable-update-1.0.2-to-1.0.3-windows-x64.rar", 24),
            ("shio-portable-update-1.0.2-to-1.0.3-windows-x64.rar", 12),
            ("archive.part0042.rar", 14),
            ("映画のタイトルがとても長いファイル名.mkv", 20),
            ("no-extension-but-very-long-name-indeed", 16),
        ] {
            let short = middle_ellipsis(name, budget);
            assert!(cells(&short) <= budget, "{short} exceeds {budget}");
            assert!(short.contains('…'), "{short}");
            if let Some(dot) = name.rfind('.') {
                assert!(
                    short.ends_with(&name[dot..]),
                    "{short} lost {}",
                    &name[dot..]
                );
            }
        }
        assert_eq!(middle_ellipsis("short.zip", 20), "short.zip");
    }
}
