use super::PASSWORD_ID;
use crate::message::Message;
use crate::style::{self, Button, Palette, SPACE_L, SPACE_M, SPACE_S, SPACE_XL};
use crate::widgets::text_button;
use iced::widget::text::{IntoFragment, Wrapping};
use iced::widget::{column, container, row, text, text_input};
use iced::{Element, Length};

const WIDTH: f32 = 440.0;

fn dialog<'a>(
    title: impl IntoFragment<'a>,
    body: impl Into<Element<'a, Message>>,
    actions: impl IntoIterator<Item = Element<'a, Message>>,
    width: f32,
    p: &Palette,
) -> Element<'a, Message> {
    container(
        column![
            text(title)
                .size(style::TEXT_TITLE)
                .color(p.text_primary)
                .wrapping(Wrapping::WordOrGlyph),
            body.into(),
            container(row(actions).spacing(SPACE_S)).align_right(Length::Fill),
        ]
        .spacing(SPACE_L),
    )
    .padding(SPACE_XL)
    .max_width(width)
    .style(style::dialog(*p))
    .into()
}

fn note<'a>(content: &'a str, p: &Palette) -> Element<'a, Message> {
    text(content)
        .size(style::TEXT_BODY)
        .color(p.text_secondary)
        .wrapping(Wrapping::WordOrGlyph)
        .into()
}

fn subject(name: Option<&str>, count: usize) -> String {
    match name {
        Some(name) if count == 1 => format!("“{name}”"),
        _ => format!("{count} downloads"),
    }
}

pub(crate) fn confirm_remove<'a>(
    name: Option<&str>,
    count: usize,
    p: &Palette,
) -> Element<'a, Message> {
    dialog(
        format!("remove {}?", subject(name, count)),
        note(
            "downloaded files stay on disk unless you delete them too.",
            p,
        ),
        [
            text_button("cancel", Button::Ghost, Some(Message::CloseOverlay), p),
            text_button(
                "delete files",
                Button::Danger,
                Some(Message::ConfirmRemove { delete_files: true }),
                p,
            ),
            text_button(
                "remove",
                Button::Primary,
                Some(Message::ConfirmRemove {
                    delete_files: false,
                }),
                p,
            ),
        ],
        WIDTH,
        p,
    )
}

pub(crate) fn confirm_cancel<'a>(
    name: Option<&str>,
    count: usize,
    p: &Palette,
) -> Element<'a, Message> {
    dialog(
        format!("stop {}?", subject(name, count)),
        note(
            "the partial file stays on disk, but the download can't be resumed.",
            p,
        ),
        [
            text_button("keep going", Button::Ghost, Some(Message::CloseOverlay), p),
            text_button("stop", Button::Danger, Some(Message::ConfirmCancel), p),
        ],
        WIDTH,
        p,
    )
}

pub(crate) fn password<'a>(name: &'a str, input: &'a str, p: &Palette) -> Element<'a, Message> {
    let submit = (!input.is_empty()).then_some(Message::SubmitPassword);
    dialog(
        "archive password",
        column![
            container(
                text(name)
                    .size(style::TEXT_BODY)
                    .color(p.text_secondary)
                    .wrapping(Wrapping::None)
            )
            .width(Length::Fill)
            .clip(true),
            text_input("password", input)
                .id(PASSWORD_ID)
                .secure(true)
                .on_input(Message::PasswordChanged)
                .on_submit_maybe(submit.clone())
                .size(style::TEXT_BODY)
                .padding(style::CONTROL_PADDING)
                .style(style::text_input(*p)),
        ]
        .spacing(SPACE_M),
        [
            text_button("cancel", Button::Ghost, Some(Message::CloseOverlay), p),
            text_button("extract", Button::Primary, submit, p),
        ],
        WIDTH,
        p,
    )
}

pub(crate) fn startup_problem<'a>(
    title: &'a str,
    message: &'a str,
    p: &Palette,
) -> Element<'a, Message> {
    dialog(
        title,
        note(message, p),
        [
            text_button(
                "open location",
                Button::Ghost,
                Some(Message::OpenStartupLocation),
                p,
            ),
            text_button(
                "open logs",
                Button::Primary,
                Some(Message::OpenLogsFolder),
                p,
            ),
        ],
        520.0,
        p,
    )
}
