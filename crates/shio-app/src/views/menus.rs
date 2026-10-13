use super::chrome::{TITLEBAR_HEIGHT, search_span};
use super::download_list::sort_arrow;
use crate::message::{Message, SortKey};
use crate::search::Suggestion;
use crate::style::{self, Button, Palette, SPACE_S, SPACE_XS};
use iced::widget::{Space, button, column, container, mouse_area, opaque, pin, row, text};
use iced::{Alignment, Element, Length, Point, Size};
use shio_core::{DownloadId, DownloadStatus};

const MENU_WIDTH: f32 = 200.0;
const ITEM_HEIGHT: f32 = 28.0;
const CARD_INSET: f32 = 2.0 * (SPACE_XS + 1.0);
const SORT_MENU_TOP: f32 = TITLEBAR_HEIGHT + 30.0;
const COPY_SHORTCUT: &str = if cfg!(target_os = "macos") {
    "⌘c"
} else {
    "ctrl+c"
};

pub(crate) struct RowMenu {
    pub(crate) id: DownloadId,
    pub(crate) status: DownloadStatus,
    pub(crate) pinned: bool,
    pub(crate) torrent: bool,
    pub(crate) can_open_file: bool,
    pub(crate) can_copy_link: bool,
}

fn float(
    (menu, height): (Element<'_, Message>, f32),
    at: Point,
    window: Size,
) -> Element<'_, Message> {
    let x = at.x.min(window.width - MENU_WIDTH - SPACE_S).max(SPACE_S);
    let y = at.y.min(window.height - height - SPACE_S).max(SPACE_S);
    opaque(
        mouse_area(pin(opaque(menu)).x(x).y(y))
            .on_press(Message::CloseMenu)
            .on_right_press(Message::CloseMenu),
    )
}

/// Returns the menu with its height, which `float` needs to keep it on screen.
fn card<'a>(
    groups: impl IntoIterator<Item = Vec<Element<'a, Message>>>,
    width: f32,
    p: &Palette,
) -> (Element<'a, Message>, f32) {
    let mut items = column![];
    let mut height = CARD_INSET;
    for (index, group) in groups.into_iter().filter(|g| !g.is_empty()).enumerate() {
        if index > 0 {
            items = items.push(Space::new().height(SPACE_XS));
            height += SPACE_XS;
        }
        height = ITEM_HEIGHT.mul_add(group.len() as f32, height);
        items = items.extend(group);
    }
    (
        container(items)
            .width(width)
            .padding(SPACE_XS)
            .style(style::popup(*p))
            .into(),
        height,
    )
}

fn item<'a>(
    label: &'a str,
    trailing: Option<Element<'a, Message>>,
    message: Message,
    kind: Button,
    p: &Palette,
) -> Element<'a, Message> {
    let content = row![text(label).size(style::TEXT_SMALL).width(Length::Fill)]
        .push(trailing)
        .spacing(SPACE_S)
        .height(ITEM_HEIGHT)
        .align_y(Alignment::Center);
    button(content)
        .width(Length::Fill)
        .padding([0.0, 10.0])
        .style(style::button(*p, kind))
        .on_press(message)
        .into()
}

fn caption<'a>(content: &'a str, p: &Palette) -> Element<'a, Message> {
    text(content)
        .size(style::TEXT_CAPTION)
        .color(p.text_secondary)
        .into()
}

pub(crate) fn row_menu<'a>(
    m: &RowMenu,
    at: Point,
    window: Size,
    p: &Palette,
) -> Element<'a, Message> {
    let id = m.id;
    let entry = |label: &'a str, shortcut: Option<&'a str>, message: Message| {
        item(
            label,
            shortcut.map(|keys| caption(keys, p)),
            message,
            Button::MenuItem { highlighted: false },
            p,
        )
    };

    let primary = match m.status {
        DownloadStatus::Pending
        | DownloadStatus::Queued
        | DownloadStatus::Starting
        | DownloadStatus::Downloading
        | DownloadStatus::FetchingMetadata => {
            Some(entry("pause", Some("space"), Message::Pause(id)))
        },
        DownloadStatus::Paused | DownloadStatus::Cancelled => {
            Some(entry("resume", Some("space"), Message::Resume(id)))
        },
        DownloadStatus::Error => Some(entry("retry", Some("f5"), Message::Retry(id))),
        DownloadStatus::ExtractError => Some(entry(
            "retry extraction",
            Some("f5"),
            Message::RetryExtract(id),
        )),
        DownloadStatus::PasswordRequired => {
            Some(entry("enter password…", None, Message::RequestPassword(id)))
        },
        DownloadStatus::Seeding => Some(entry("stop seeding", None, Message::StopSeeding(id))),
        DownloadStatus::Extracting | DownloadStatus::Completed => None,
    };
    let verifiable = m.torrent
        && matches!(
            m.status,
            DownloadStatus::Paused
                | DownloadStatus::Completed
                | DownloadStatus::Seeding
                | DownloadStatus::Error
        );
    let transfer: Vec<_> = [
        primary,
        verifiable.then(|| entry("verify files", None, Message::ForceRecheck(id))),
    ]
    .into_iter()
    .flatten()
    .collect();

    let open: Vec<_> = [
        m.can_open_file
            .then(|| entry("open file", None, Message::OpenFile(id))),
        Some(entry("open folder", None, Message::OpenFolder(id))),
        m.can_copy_link
            .then(|| entry("copy link", Some(COPY_SHORTCUT), Message::CopyUrl(id))),
        Some(entry(
            if m.pinned { "unpin" } else { "pin to top" },
            None,
            Message::TogglePin(id),
        )),
    ]
    .into_iter()
    .flatten()
    .collect();

    let stoppable = matches!(
        m.status,
        DownloadStatus::Pending
            | DownloadStatus::Queued
            | DownloadStatus::Starting
            | DownloadStatus::Downloading
            | DownloadStatus::FetchingMetadata
            | DownloadStatus::Paused
            | DownloadStatus::Extracting
    );
    let remove: Vec<_> = [
        stoppable.then(|| entry("stop download…", None, Message::RequestCancel(id))),
        Some(item(
            "remove…",
            Some(caption("del", p)),
            Message::RequestRemove(id),
            Button::MenuDanger,
            p,
        )),
    ]
    .into_iter()
    .flatten()
    .collect();

    float(card([transfer, open, remove], MENU_WIDTH, p), at, window)
}

pub(crate) fn sort_menu<'a>(
    current: SortKey,
    descending: bool,
    window: Size,
    p: &Palette,
) -> Element<'a, Message> {
    let items = SortKey::ALL
        .iter()
        .map(|&key| {
            let arrow = (key == current).then(|| {
                sort_arrow(descending)
                    .size(11)
                    .color(p.text_secondary)
                    .into()
            });
            item(
                key.label(),
                arrow,
                Message::SortBy(key),
                Button::MenuItem { highlighted: false },
                p,
            )
        })
        .collect();
    let menu = card([items], MENU_WIDTH, p);
    let at = Point::new(window.width - MENU_WIDTH - SPACE_S, SORT_MENU_TOP);
    float(menu, at, window)
}

/// Sits beside the search field rather than over a click-away layer, so typing continues.
pub(crate) fn search_dropdown<'a>(
    suggestions: &[Suggestion],
    highlighted: Option<usize>,
    window: Size,
    p: &Palette,
) -> Element<'a, Message> {
    let (x, width) = search_span(window.width);
    let items = suggestions
        .iter()
        .enumerate()
        .map(|(index, suggestion)| {
            item(
                suggestion.label,
                (!suggestion.hint.is_empty()).then(|| caption(suggestion.hint, p)),
                Message::ApplySuggestion(suggestion.value),
                Button::MenuItem {
                    highlighted: highlighted == Some(index),
                },
                p,
            )
        })
        .collect();
    let (menu, _) = card([items], width, p);
    pin(opaque(menu)).x(x).y(TITLEBAR_HEIGHT - 2.0).into()
}
