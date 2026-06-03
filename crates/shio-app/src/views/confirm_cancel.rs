use crate::message::Message;
use crate::style;
use iced::widget::{
    Space, button, center, column, container, mouse_area, opaque, row, stack, text,
};
use iced::{Element, Length};

pub(crate) fn view<'a>(
    label: String,
    multiple: bool,
    p: &'a crate::style::Palette,
    material: shio_core::WindowMaterialPreference,
    base: Element<'a, Message>,
) -> Element<'a, Message> {
    let title = if multiple {
        text("cancel downloads").size(16).color(p.text_primary)
    } else {
        text("cancel download").size(16).color(p.text_primary)
    };
    let name = text(label).size(13).color(p.text_secondary);
    let explanation_text = if multiple {
        "this will stop these downloads immediately."
    } else {
        "this will stop the download immediately."
    };
    let explanation = text(explanation_text).size(12).color(p.text_tertiary);

    let go_back = button(text("go back").size(13))
        .style(style::btn_ghost(p))
        .on_press(Message::CancelCancelDownload)
        .padding([8, 16]);

    let confirm_label = if multiple {
        "cancel downloads"
    } else {
        "cancel download"
    };
    let confirm_cancel = button(text(confirm_label).size(13))
        .style(style::btn_danger(p))
        .on_press(Message::ConfirmCancelDownload)
        .padding([8, 16]);

    let buttons = row![
        Space::new().width(Length::Fill),
        go_back,
        Space::new().width(8),
        confirm_cancel
    ]
    .align_y(iced::Alignment::Center);

    let form = column![
        title,
        Space::new().height(12),
        name,
        Space::new().height(6),
        explanation,
        Space::new().height(20),
        buttons,
    ]
    .width(420)
    .padding(24);

    let card = container(form).style(style::modal_card(p, material));

    let overlay = mouse_area(center(opaque(card))).on_press(Message::CancelCancelDownload);

    let backdrop = container(overlay)
        .style(style::modal_backdrop(p))
        .width(Length::Fill)
        .height(Length::Fill);

    stack![base, backdrop].into()
}
