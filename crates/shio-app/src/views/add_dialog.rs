use super::ADD_URLS_ID;
use crate::app::{AddDialog, AddValidation, Link, LinkState, TorrentPick};
use crate::message::Message;
use crate::style::{self, Button, Palette, SPACE_L, SPACE_M, SPACE_S, SPACE_XL, SPACE_XS};
use crate::widgets::{setting, switch, text_button};
use bytesize::ByteSize;
use iced::keyboard::{Key, key};
use iced::widget::text::Wrapping;
use iced::widget::{
    Row, Space, button, checkbox, column, container, row, scrollable, text, text_editor, text_input,
};
use iced::{Alignment, Color, Element, Length};
use std::borrow::Cow;

const FILE_ROW_LIMIT: usize = 500;
const FILE_SEARCH_THRESHOLD: usize = 20;
const CHECKBOX_SIZE: f32 = 14.0;

pub(crate) fn view<'a>(add: &'a AddDialog, p: &Palette) -> Element<'a, Message> {
    let busy = add.submitting;

    let mut urls = text_editor(&add.urls)
        .id(ADD_URLS_ID)
        .placeholder("paste links or magnets, one per line")
        .key_binding(|press| {
            if press.modifiers.command() && press.key == Key::Named(key::Named::Enter) {
                Some(text_editor::Binding::Custom(Message::AddSubmit))
            } else {
                text_editor::Binding::from_key_press(press)
            }
        })
        .height(96)
        .size(style::TEXT_BODY)
        .padding(SPACE_M)
        .style(style::text_editor(*p));
    if !busy {
        urls = urls.on_action(Message::AddUrlsAction);
    }

    let (status, color) = match &add.validation {
        AddValidation::Empty => ("", p.text_tertiary),
        AddValidation::Invalid(message) => (message.as_str(), p.error),
        AddValidation::Ready(summary) => (summary.as_str(), p.text_secondary),
    };
    let sources_bar = row![
        text_button(
            "choose .torrent…",
            Button::Secondary,
            (!busy).then_some(Message::PickTorrentFiles),
            p
        ),
        text(status)
            .size(style::TEXT_SMALL)
            .color(color)
            .wrapping(Wrapping::WordOrGlyph)
            .width(Length::Fill),
    ]
    .spacing(SPACE_M)
    .align_y(Alignment::Center);

    let sources = (!add.links.is_empty() || !add.torrents.is_empty()).then(|| {
        let items = add.links.iter().map(|link| link_item(link, p)).chain(
            add.torrents
                .iter()
                .map(|torrent| torrent_item(torrent, add.selected_torrent, p)),
        );
        container(
            scrollable(column(items).spacing(2).padding(SPACE_XS)).style(style::scrollable(*p)),
        )
        .max_height(168)
        .style(style::rounded(p.hover, style::RADIUS_FIELD))
    });
    let picker = add
        .selected_torrent
        .and_then(|id| add.torrents.iter().find(|torrent| torrent.id == id))
        .map(|torrent| file_picker(torrent, &add.torrent_search, busy, p));

    let subfolder = row![]
        .push(add.create_subfolder.then(|| {
            field(
                &add.subfolder,
                "folder name",
                Message::AddSubfolderNameChanged,
                busy,
                p,
            )
        }))
        .push(switch(
            add.create_subfolder,
            Message::AddSubfolderToggled,
            p,
        ))
        .spacing(SPACE_M)
        .align_y(Alignment::Center);
    let options = column![]
        .push(add.single_http().then(|| {
            setting(
                "file name",
                None,
                field(
                    &add.filename,
                    "from the server",
                    Message::AddFilenameChanged,
                    busy,
                    p,
                ),
                p,
            )
        }))
        .push(setting(
            "save to",
            None,
            row![
                field(
                    &add.save_path,
                    "folder",
                    Message::AddSavePathChanged,
                    busy,
                    p
                ),
                text_button(
                    "browse…",
                    Button::Secondary,
                    (!busy).then_some(Message::AddPickFolder),
                    p
                ),
            ]
            .spacing(SPACE_S)
            .align_y(Alignment::Center),
            p,
        ))
        .push(setting("put in a new folder", None, subfolder, p))
        .push(setting(
            "extract archives",
            None,
            switch(add.auto_extract, Message::AddAutoExtractToggled, p),
            p,
        ))
        .spacing(SPACE_M);

    let failures = (!add.failures.is_empty()).then(|| {
        column(add.failures.iter().map(|(item, reason)| {
            text(format!("{item}: {reason}"))
                .size(style::TEXT_SMALL)
                .color(p.error)
                .wrapping(Wrapping::WordOrGlyph)
                .into()
        }))
        .spacing(SPACE_XS)
    });

    let body = column![urls, sources_bar]
        .push(sources)
        .push(picker)
        .push(Space::new().height(SPACE_XS))
        .push(options)
        .push(failures)
        .spacing(SPACE_M);

    let can_submit = matches!(add.validation, AddValidation::Ready(_)) && !busy;
    let actions = row![
        Space::new().width(Length::Fill),
        text_button("cancel", Button::Ghost, Some(Message::CloseOverlay), p),
        text_button(
            if busy { "adding…" } else { "add" },
            Button::Primary,
            can_submit.then_some(Message::AddSubmit),
            p
        ),
    ]
    .spacing(SPACE_S);

    container(
        column![
            text("add downloads")
                .size(style::TEXT_TITLE)
                .color(p.text_primary),
            scrollable(body).style(style::scrollable(*p)),
            actions
        ]
        .spacing(SPACE_L),
    )
    .padding(SPACE_XL)
    .max_width(640)
    .style(style::dialog(*p))
    .into()
}

fn field<'a>(
    value: &'a str,
    placeholder: &'a str,
    on_input: fn(String) -> Message,
    busy: bool,
    p: &Palette,
) -> Element<'a, Message> {
    text_input(placeholder, value)
        .on_input_maybe((!busy).then_some(on_input))
        .size(style::TEXT_BODY)
        .padding(style::CONTROL_PADDING)
        .width(Length::Fill)
        .style(style::text_input(*p))
        .into()
}

fn source_line<'a>(
    name: &'a str,
    detail: Cow<'a, str>,
    detail_color: Color,
    p: &Palette,
) -> Row<'a, Message> {
    row![
        container(
            text(name)
                .size(style::TEXT_BODY)
                .color(p.text_primary)
                .wrapping(Wrapping::None)
        )
        .width(Length::Fill)
        .clip(true),
        text(detail)
            .size(style::TEXT_SMALL)
            .color(detail_color)
            .wrapping(Wrapping::None),
    ]
    .spacing(SPACE_M)
    .align_y(Alignment::Center)
}

fn link_item<'a>(link: &'a Link, p: &Palette) -> Element<'a, Message> {
    let (detail, color): (Cow<'_, str>, _) = match &link.state {
        _ if link.mirror => ("mirror · skipped".into(), p.text_tertiary),
        LinkState::Unchecked => (Cow::Borrowed(""), p.text_tertiary),
        LinkState::Resolving => ("checking…".into(), p.text_tertiary),
        LinkState::Ready { size: Some(size) } => {
            (ByteSize(*size).to_string().into(), p.text_secondary)
        },
        LinkState::Ready { size: None } => ("size unknown".into(), p.text_tertiary),
        LinkState::Blocked(reason) => (reason.as_str().into(), p.error),
        LinkState::Failed(reason) => (reason.as_str().into(), p.text_tertiary),
    };
    container(source_line(&link.name, detail, color, p))
        .padding([SPACE_XS, SPACE_S])
        .into()
}

fn torrent_item<'a>(
    torrent: &'a TorrentPick,
    selected: Option<u64>,
    p: &Palette,
) -> Element<'a, Message> {
    let detail = format!(
        "{} of {} files · {}",
        torrent.selected_count(),
        torrent.files.len(),
        ByteSize(torrent.selected_size())
    );
    button(source_line(
        &torrent.name,
        detail.into(),
        p.text_secondary,
        p,
    ))
    .width(Length::Fill)
    .padding([SPACE_XS, SPACE_S])
    .style(style::button(
        *p,
        Button::Nav {
            active: selected == Some(torrent.id),
        },
    ))
    .on_press(Message::AddTorrentSelected(torrent.id))
    .into()
}

fn file_picker<'a>(
    torrent: &'a TorrentPick,
    search: &'a str,
    busy: bool,
    p: &Palette,
) -> Element<'a, Message> {
    let query = search.trim().to_lowercase();
    let matching: Vec<_> = torrent
        .files
        .iter()
        .enumerate()
        .filter(|(_, file)| {
            query.is_empty() || file.path.to_string_lossy().to_lowercase().contains(&query)
        })
        .collect();
    let all_selected = matching.iter().all(|(_, file)| file.selected);
    let shown = &matching[..matching.len().min(FILE_ROW_LIMIT)];
    let hidden = matching.len() - shown.len();
    let id = torrent.id;

    let header = row![
        checkbox(all_selected)
            .label(if query.is_empty() {
                "all files"
            } else {
                "all matching"
            })
            .on_toggle_maybe((!busy).then_some(move |selected| {
                Message::AddTorrentFilesToggled {
                    torrent: id,
                    selected,
                }
            }))
            .size(CHECKBOX_SIZE)
            .text_size(style::TEXT_SMALL)
            .style(style::checkbox(*p)),
        Space::new().width(Length::Fill),
    ]
    .push((torrent.files.len() > FILE_SEARCH_THRESHOLD).then(|| {
        text_input("filter files", search)
            .on_input(Message::AddTorrentSearchChanged)
            .size(style::TEXT_SMALL)
            .padding([SPACE_XS, SPACE_S])
            .width(200)
            .style(style::text_input(*p))
    }))
    .spacing(SPACE_M)
    .align_y(Alignment::Center);

    let files = column(shown.iter().map(|&(index, file)| {
        row![
            checkbox(file.selected)
                .on_toggle_maybe((!busy).then_some(move |selected| {
                    Message::AddTorrentFileToggled {
                        torrent: id,
                        file: index,
                        selected,
                    }
                }))
                .size(CHECKBOX_SIZE)
                .style(style::checkbox(*p)),
            container(
                text(file.path.to_string_lossy())
                    .size(style::TEXT_SMALL)
                    .color(p.text_primary)
                    .wrapping(Wrapping::None)
            )
            .width(Length::Fill)
            .clip(true),
            text(ByteSize(file.size).to_string())
                .size(style::TEXT_SMALL)
                .color(p.text_secondary),
        ]
        .spacing(SPACE_S)
        .align_y(Alignment::Center)
        .into()
    }))
    .push((hidden > 0).then(|| {
        text(format!("{hidden} more files — filter to find them"))
            .size(style::TEXT_SMALL)
            .color(p.text_tertiary)
    }))
    .spacing(SPACE_XS);

    container(column![header, scrollable(files).style(style::scrollable(*p))].spacing(SPACE_S))
        .padding(SPACE_M)
        .max_height(220)
        .style(style::rounded(p.hover, style::RADIUS_FIELD))
        .into()
}
