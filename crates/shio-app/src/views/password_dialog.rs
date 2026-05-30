use crate::message::Message;
use crate::style;
use iced::widget::{
    Space, button, center, column, container, mouse_area, opaque, row, stack, text, text_input,
};
use iced::{Element, Length};
use shio_core::DownloadId;

pub(crate) fn view<'a>(
    id: DownloadId,
    filename: &'a str,
    password: &'a str,
    p: &'a style::Palette,
    material: shio_core::WindowMaterialPreference,
    base: Element<'a, Message>,
) -> Element<'a, Message> {
    let title = text("password required").size(16).color(p.text_primary);

    let name = text(filename)
        .size(13)
        .color(p.text_secondary)
        .wrapping(iced::widget::text::Wrapping::None);

    let description = text("enter the password to extract this archive.")
        .size(12)
        .color(p.text_tertiary);

    let input = text_input("password", password)
        .on_input(Message::PasswordChanged)
        .on_submit(Message::ConfirmPassword(id))
        .secure(true)
        .style(style::input(p))
        .size(13)
        .padding([8, 12])
        .width(Length::Fill);

    let cancel = button(text("cancel").size(13))
        .style(style::btn_ghost(p))
        .on_press(Message::CancelPassword)
        .padding([8, 16]);

    let mut unlock = button(text("unlock").size(13))
        .style(style::btn_primary(p))
        .padding([8, 16]);
    if !password.is_empty() {
        unlock = unlock.on_press(Message::ConfirmPassword(id));
    }

    let buttons = row![
        Space::new().width(Length::Fill),
        cancel,
        Space::new().width(8),
        unlock,
    ]
    .align_y(iced::Alignment::Center);

    let form = column![
        title,
        Space::new().height(12),
        name,
        Space::new().height(6),
        description,
        Space::new().height(14),
        input,
        Space::new().height(18),
        buttons,
    ]
    .width(440)
    .padding(24);

    let card = container(form).style(style::modal_card(p, material));

    let overlay = mouse_area(center(opaque(card))).on_press(Message::CancelPassword);

    let backdrop = container(overlay)
        .style(style::modal_backdrop(p))
        .width(Length::Fill)
        .height(Length::Fill);

    stack![base, backdrop].into()
}
