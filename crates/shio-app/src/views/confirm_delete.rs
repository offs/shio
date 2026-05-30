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
    let title = text("delete").size(16).color(p.text_primary);

    let name = text(label).size(13).color(p.text_secondary);

    let explanation_text = if multiple {
        "choose what to do with these downloads."
    } else {
        "choose what to do with this download."
    };
    let explanation = text(explanation_text).size(12).color(p.text_tertiary);

    let delete_label = if multiple {
        "delete files"
    } else {
        "delete file"
    };

    let cancel = button(text("cancel").size(13))
        .style(style::btn_ghost(p))
        .on_press(Message::CancelDeleteWithFiles)
        .padding([8, 16]);

    let remove_from_list = button(text("remove from list").size(13))
        .style(style::btn_secondary(p))
        .on_press(Message::ConfirmRemoveFromList)
        .padding([8, 16]);

    let delete_file = button(text(delete_label).size(13))
        .style(style::btn_danger(p))
        .on_press(Message::ConfirmDeleteFiles)
        .padding([8, 16]);

    let buttons = row![
        Space::new().width(Length::Fill),
        cancel,
        Space::new().width(8),
        remove_from_list,
        Space::new().width(8),
        delete_file
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

    let overlay = mouse_area(center(opaque(card))).on_press(Message::CancelDeleteWithFiles);

    let backdrop = container(overlay)
        .style(style::modal_backdrop(p))
        .width(Length::Fill)
        .height(Length::Fill);

    stack![base, backdrop].into()
}
