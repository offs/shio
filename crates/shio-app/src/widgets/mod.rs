pub(crate) mod download_row;
mod menu_anchor;
mod search_field;
mod software_probe;

pub(crate) use menu_anchor::menu_anchor;
pub(crate) use search_field::search_field;
pub(crate) use software_probe::SoftwareProbe;

use crate::message::Message;
use crate::style::{self, Button, ICON_BUTTON, Palette, SPACE_L, SPACE_S, SPACE_XL, SPACE_XS};
use iced::widget::{
    Space, Text, button, center, column, container, mouse_area, opaque, row, text, toggler, tooltip,
};
use iced::{Alignment, Color, Element, Length};
use std::time::Duration;

pub(crate) fn modal<'a>(
    card: impl Into<Element<'a, Message>>,
    on_dismiss: Option<Message>,
    p: &Palette,
) -> Element<'a, Message> {
    let backdrop = mouse_area(
        center(opaque(card))
            .padding(SPACE_XL)
            .style(style::backdrop(*p)),
    );
    let backdrop = match on_dismiss {
        Some(message) => backdrop.on_press(message),
        None => backdrop,
    };
    opaque(backdrop)
}

pub(crate) fn text_button<'a>(
    label: &'a str,
    kind: Button,
    on_press: Option<Message>,
    p: &Palette,
) -> Element<'a, Message> {
    button(text(label).size(style::TEXT_SMALL))
        .padding(style::CONTROL_PADDING)
        .style(style::button(*p, kind))
        .on_press_maybe(on_press)
        .into()
}

pub(crate) fn icon_button<'a>(
    icon: Text<'a>,
    hint: &'a str,
    on_press: Option<Message>,
    p: &Palette,
) -> Element<'a, Message> {
    hint_on(
        button(center(icon.size(12)))
            .width(ICON_BUTTON)
            .height(ICON_BUTTON)
            .padding(0)
            .style(style::button(*p, Button::Ghost))
            .on_press_maybe(on_press),
        hint,
        tooltip::Position::Bottom,
        p,
    )
}

pub(crate) fn hint_on<'a>(
    content: impl Into<Element<'a, Message>>,
    hint: impl text::IntoFragment<'a>,
    position: tooltip::Position,
    p: &Palette,
) -> Element<'a, Message> {
    tooltip(
        content,
        container(text(hint).size(style::TEXT_SMALL).color(p.text_primary))
            .padding([SPACE_XS, SPACE_S])
            .max_width(420)
            .style(style::popup(*p)),
        position,
    )
    .gap(SPACE_XS)
    .delay(Duration::from_millis(500))
    .into()
}

pub(crate) fn pill<'a>(color: Color, width: f32, height: f32) -> Element<'a, Message> {
    container(Space::new().width(width).height(height))
        .style(style::rounded(color, height / 2.0))
        .into()
}

pub(crate) fn setting<'a>(
    title: &'a str,
    hint: Option<&'a str>,
    control: impl Into<Element<'a, Message>>,
    p: &Palette,
) -> Element<'a, Message> {
    let label = column![text(title).size(style::TEXT_BODY).color(p.text_primary)]
        .push(hint.map(|hint| text(hint).size(style::TEXT_SMALL).color(p.text_tertiary)))
        .spacing(2);
    row![container(label).width(Length::Fill), control.into()]
        .spacing(SPACE_L)
        .align_y(Alignment::Center)
        .into()
}

pub(crate) fn switch<'a>(
    on: bool,
    message: impl Fn(bool) -> Message + 'a,
    p: &Palette,
) -> Element<'a, Message> {
    toggler(on)
        .on_toggle(message)
        .size(18)
        .style(style::toggler(*p))
        .into()
}
