use crate::message::Message;
use crate::search::Suggestion;
use crate::style;
use iced::widget::{button, column, container, text};
use iced::{Element, Length};

pub(crate) fn view<'a>(
    suggestions: &[Suggestion],
    selected: Option<usize>,
    p: &'a crate::style::Palette,
) -> Element<'a, Message> {
    let items = suggestions.iter().enumerate().fold(
        column![].spacing(0).width(Length::Fill),
        |col, (i, s)| {
            let is_selected = selected == Some(i);
            let value = s.value.to_string();
            let btn = button(text(s.label).size(12))
                .on_press(Message::SearchApplySuggestion(value))
                .padding([6, 12])
                .width(Length::Fill);
            let btn = if is_selected {
                btn.style(style::btn_dropdown_active(p))
            } else {
                btn.style(style::btn_dropdown(p))
            };
            col.push(btn)
        },
    );

    container(items)
        .style(style::card(p))
        .width(180)
        .padding(4)
        .into()
}
