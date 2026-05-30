use crate::app::SEARCH_INPUT_ID;
use crate::message::Message;
use crate::style;
use iced::widget::{Space, button, container, mouse_area, row, text, text_input};
use iced::{Element, Length};

pub(crate) const TITLEBAR_HEIGHT: f32 = 38.0;

pub(crate) fn view<'a>(
    search_query: &'a str,
    p: &'a crate::style::Palette,
) -> Element<'a, Message> {
    let title = text("shio").size(12).color(p.text_secondary);

    let add_btn = button(
        row![iced_fonts::bootstrap::plus().size(12), text("new").size(11)]
            .spacing(4)
            .align_y(iced::Alignment::Center),
    )
    .style(style::btn_primary(p))
    .on_press(Message::AddDownloadPressed)
    .padding([4, 10]);

    let pause_all = button(iced_fonts::bootstrap::pause_fill().size(12))
        .style(style::btn_ghost(p))
        .on_press(Message::PauseAll)
        .padding([4, 8]);

    let resume_all = button(iced_fonts::bootstrap::play_fill().size(12))
        .style(style::btn_ghost(p))
        .on_press(Message::ResumeAll)
        .padding([4, 8]);

    let search_icon = iced_fonts::bootstrap::search()
        .size(11)
        .color(p.text_tertiary);
    let search_input = text_input("search... (type: size:)", search_query)
        .id(SEARCH_INPUT_ID.clone())
        .on_input(Message::SearchTextChanged)
        .style(style::search_input(p))
        .size(12)
        .padding([4, 8])
        .width(200);
    let search =
        row![search_icon, Space::new().width(4), search_input].align_y(iced::Alignment::Center);

    let settings = button(iced_fonts::bootstrap::gear().size(12))
        .style(style::btn_ghost(p))
        .on_press(Message::OpenSettings)
        .padding([4, 8]);

    let minimize = button(iced_fonts::bootstrap::dash_lg().size(12))
        .style(style::btn_icon(p))
        .on_press(Message::WindowMinimize)
        .padding([4, 12])
        .height(TITLEBAR_HEIGHT);

    let maximize = button(iced_fonts::bootstrap::square().size(10))
        .style(style::btn_icon(p))
        .on_press(Message::WindowMaximizeToggle)
        .padding([4, 12])
        .height(TITLEBAR_HEIGHT);

    let close = button(iced_fonts::bootstrap::x_lg().size(12))
        .style(style::btn_icon(p))
        .on_press(Message::WindowClose)
        .padding([4, 12])
        .height(TITLEBAR_HEIGHT);

    let left = row![
        Space::new().width(12),
        title,
        Space::new().width(8),
        add_btn,
        Space::new().width(2),
        pause_all,
        resume_all
    ]
    .align_y(iced::Alignment::Center)
    .width(Length::FillPortion(1));

    let center = container(search)
        .width(Length::FillPortion(1))
        .center_x(Length::Fill);

    let right = row![
        Space::new().width(Length::Fill),
        settings,
        Space::new().width(4),
        minimize,
        maximize,
        close,
    ]
    .align_y(iced::Alignment::Center)
    .width(Length::FillPortion(1));

    let content = row![left, center, right]
        .align_y(iced::Alignment::Center)
        .height(TITLEBAR_HEIGHT);

    let bar = container(content)
        .style(style::titlebar(p))
        .width(Length::Fill)
        .height(TITLEBAR_HEIGHT);

    mouse_area(bar).on_press(Message::WindowDragStart).into()
}
