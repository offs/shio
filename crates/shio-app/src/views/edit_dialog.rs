use crate::message::Message;
use crate::style;
use iced::widget::{
    Space, button, center, column, container, mouse_area, opaque, row, stack, text, text_input,
};
use iced::{Alignment, Element, Length};
use shio_core::{DownloadId, DownloadStatus};

#[expect(
    clippy::too_many_arguments,
    reason = "edit-dialog view composition keeps state explicit"
)]
pub(crate) fn view<'a>(
    id: DownloadId,
    status: DownloadStatus,
    filename: &'a str,
    save_path: &'a str,
    conflict_error: bool,
    sanitized_hint: Option<String>,
    p: &'a crate::style::Palette,
    material: shio_core::WindowMaterialPreference,
    base: Element<'a, Message>,
) -> Element<'a, Message> {
    let title = text("edit").size(16).color(p.text_primary);

    let status_locked = matches!(
        status,
        DownloadStatus::Downloading | DownloadStatus::Starting
    );

    let filename_label = text("filename").size(11).color(p.text_tertiary);
    let mut filename_input = text_input("", filename)
        .size(13)
        .padding([8, 12])
        .style(style::input(p));
    if !status_locked {
        filename_input = filename_input.on_input(Message::EditFilenameChanged);
    }
    let mut filename_column = column![filename_label, Space::new().height(4), filename_input];
    if status_locked {
        filename_column = filename_column.push(Space::new().height(4));
        filename_column = filename_column.push(
            text("pause the download to change filename or folder")
                .size(11)
                .color(p.text_tertiary),
        );
    } else if let Some(hint) = sanitized_hint {
        filename_column = filename_column.push(Space::new().height(4));
        filename_column = filename_column.push(
            text(format!("will be saved as: {hint}"))
                .size(11)
                .color(p.text_tertiary),
        );
    }

    let path_label = text("save path").size(11).color(p.text_tertiary);
    let mut path_input = text_input("", save_path)
        .size(13)
        .padding([8, 12])
        .style(style::input(p))
        .width(Length::Fill);
    if !status_locked {
        path_input = path_input.on_input(Message::EditSavePathChanged);
    }

    let mut browse_btn = button(text("browse").size(12))
        .style(style::btn_secondary(p))
        .padding([8, 14]);
    if !status_locked {
        browse_btn = browse_btn.on_press(Message::EditPickSavePath);
    }

    let path_row = row![path_input, Space::new().width(8), browse_btn].align_y(Alignment::Center);
    let mut path_column = column![path_label, Space::new().height(4), path_row];
    if conflict_error {
        path_column = path_column.push(Space::new().height(4));
        path_column = path_column.push(
            text("a file already exists at that location")
                .size(11)
                .color(p.error),
        );
    }

    let save_enabled = !filename.trim().is_empty() && !conflict_error;
    let cancel_btn = button(text("cancel").size(13))
        .style(style::btn_ghost(p))
        .on_press(Message::CancelEdit)
        .padding([8, 16]);
    let mut save_btn = button(text("save").size(13))
        .style(style::btn_primary(p))
        .padding([8, 16]);
    if save_enabled {
        save_btn = save_btn.on_press(Message::ConfirmEdit(id));
    }
    let buttons = row![
        Space::new().width(Length::Fill),
        cancel_btn,
        Space::new().width(8),
        save_btn
    ]
    .align_y(Alignment::Center);

    let form = column![
        title,
        Space::new().height(16),
        filename_column,
        Space::new().height(16),
        path_column,
        Space::new().height(16),
        buttons,
    ]
    .width(480)
    .padding(24);

    let card = container(form).style(style::modal_card(p, material));
    let overlay = mouse_area(center(opaque(card))).on_press(Message::CancelEdit);
    let backdrop = container(overlay)
        .style(style::modal_backdrop(p))
        .width(Length::Fill)
        .height(Length::Fill);

    stack![base, backdrop].into()
}
