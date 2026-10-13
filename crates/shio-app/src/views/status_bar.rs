use crate::app::{ToastKind, Totals};
use crate::message::Message;
use crate::style::{self, Button, Palette, SPACE_M, SPACE_S, SPACE_XS};
use crate::widgets::hint_on;
use bytesize::ByteSize;
use iced::widget::text::Wrapping;
use iced::widget::{button, container, row, text, tooltip};
use iced::{Alignment, Element, Length, Padding};
use iced_fonts::bootstrap;

const HEIGHT: f32 = 28.0;

pub(crate) fn view<'a>(
    s: &Totals,
    toast: Option<(ToastKind, &'a str)>,
    clipboard: bool,
    p: &Palette,
) -> Element<'a, Message> {
    let quiet = |content: String| {
        text(content)
            .size(style::TEXT_CAPTION)
            .color(p.text_tertiary)
    };
    let ghost = |content: Element<'a, Message>, message: Option<Message>| {
        button(content)
            .padding([2.0, 6.0])
            .style(style::button(*p, Button::Ghost))
            .on_press_maybe(message)
    };

    let noun = if s.downloads == 1 {
        "download"
    } else {
        "downloads"
    };
    let left: Element<'a, Message> = match toast {
        Some((kind, message)) => {
            let glyph = match kind {
                ToastKind::Info => bootstrap::info_circle_fill().color(p.accent),
                ToastKind::Success => bootstrap::check_circle_fill().color(p.success),
                ToastKind::Error => bootstrap::exclamation_circle_fill().color(p.error),
            };
            row![
                glyph.size(11),
                text(message)
                    .size(style::TEXT_SMALL)
                    .color(p.text_primary)
                    .wrapping(Wrapping::None),
            ]
            .spacing(SPACE_S)
            .align_y(Alignment::Center)
            .into()
        },
        None if s.active > 0 => {
            quiet(format!("{} {noun} · {} downloading", s.downloads, s.active)).into()
        },
        None => quiet(format!("{} {noun}", s.downloads)).into(),
    };
    let speeds = if s.up > 0 {
        format!("↓ {}/s   ↑ {}/s", ByteSize(s.down), ByteSize(s.up))
    } else {
        format!("↓ {}/s", ByteSize(s.down))
    };

    let (icon, label) = if clipboard {
        (bootstrap::clipboard_check(), "clipboard on")
    } else {
        (bootstrap::clipboard(), "clipboard off")
    };
    let clipboard = hint_on(
        ghost(
            row![icon.size(11), text(label).size(style::TEXT_CAPTION)]
                .spacing(SPACE_XS)
                .align_y(Alignment::Center)
                .into(),
            Some(Message::ToggleClipboard),
        ),
        "pick up links copied anywhere",
        tooltip::Position::Top,
        p,
    );

    let (icon, hint, message) = if s.can_pause_all {
        (
            bootstrap::pause_fill(),
            "pause all",
            Some(Message::PauseAll),
        )
    } else {
        (
            bootstrap::play_fill(),
            "resume all",
            s.can_resume_all.then_some(Message::ResumeAll),
        )
    };
    let all = hint_on(
        ghost(icon.size(11).into(), message),
        hint,
        tooltip::Position::Top,
        p,
    );

    row![
        container(left).width(Length::Fill).clip(true),
        quiet(speeds),
        clipboard,
        all,
    ]
    .spacing(SPACE_M)
    .height(HEIGHT)
    .align_y(Alignment::Center)
    .padding(Padding::from([0.0, SPACE_S]).left(SPACE_M + SPACE_XS))
    .into()
}
