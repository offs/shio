use crate::message::{Message, SortKey, Tab};
use crate::style::{self, Button, Palette, SPACE_S, SPACE_XS};
use iced::widget::{Space, Text, button, center, column, keyed_column, row, scrollable, text};
use iced::{Alignment, Element, Length};
use iced_fonts::bootstrap;
use shio_core::DownloadId;

const BAR_BUTTON_PADDING: [f32; 2] = [3.0, SPACE_S];

pub(super) fn sort_arrow(descending: bool) -> Text<'static> {
    if descending {
        bootstrap::arrow_down()
    } else {
        bootstrap::arrow_up()
    }
}

pub(crate) fn tabs<'a>(
    active: Tab,
    counts: impl IntoIterator<Item = (Tab, usize)>,
    sort: SortKey,
    descending: bool,
    p: &Palette,
) -> Element<'a, Message> {
    let tabs = row(counts.into_iter().map(|(tab, count)| {
        let selected = tab == active;
        let label = row![
            text(tab.label())
                .size(style::TEXT_SMALL)
                .color(if selected {
                    p.text_primary
                } else {
                    p.text_tertiary
                })
        ]
        .push(
            (count > 0 || tab == Tab::All)
                .then(|| text(count).size(style::TEXT_CAPTION).color(p.text_tertiary)),
        )
        .spacing(5)
        .align_y(Alignment::Center);
        button(label)
            .padding(BAR_BUTTON_PADDING)
            .style(style::button(*p, Button::Nav { active: selected }))
            .on_press(Message::TabSelected(tab))
            .into()
    }))
    .spacing(2);

    let sort = button(
        row![
            text(sort.label()).size(style::TEXT_SMALL),
            sort_arrow(descending).size(10),
        ]
        .spacing(SPACE_XS)
        .align_y(Alignment::Center),
    )
    .padding(BAR_BUTTON_PADDING)
    .style(style::button(*p, Button::Ghost))
    .on_press(Message::ToggleSortMenu);

    row![tabs, Space::new().width(Length::Fill), sort]
        .align_y(Alignment::Center)
        .padding([SPACE_XS, SPACE_S])
        .into()
}

pub(crate) fn rows<'a>(
    rows: impl IntoIterator<Item = (DownloadId, Element<'a, Message>)>,
    p: &Palette,
) -> Element<'a, Message> {
    scrollable(
        keyed_column(rows)
            .width(Length::Fill)
            .spacing(2)
            .padding(SPACE_XS),
    )
    .height(Length::Fill)
    .style(style::scrollable(*p))
    .into()
}

pub(crate) fn empty<'a>(searching: bool, has_downloads: bool, p: &Palette) -> Element<'a, Message> {
    let quiet = |label: &'a str| text(label).size(style::TEXT_SMALL).color(p.text_tertiary);
    let content: Element<'a, Message> = if searching {
        quiet("no matches").into()
    } else if has_downloads {
        quiet("nothing here").into()
    } else {
        column![
            text("no downloads")
                .size(style::TEXT_BODY)
                .color(p.text_secondary),
            quiet("paste a link or drop a .torrent"),
        ]
        .spacing(SPACE_XS)
        .align_x(Alignment::Center)
        .into()
    };
    center(content).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{ThemeCatalog, ThemeId};
    use crate::widgets::download_row::{self, RowView};
    use shio_core::DownloadStatus;

    #[test]
    fn lazy_rows_still_fill_the_list_width() {
        let theme =
            ThemeCatalog::load(std::path::Path::new("")).resolve(&ThemeId::default(), false);
        let id = DownloadId(uuid::Uuid::nil());
        let row = download_row::view(RowView {
            id,
            name: "file.iso",
            error: None,
            status: DownloadStatus::Downloading,
            downloaded: 1,
            total: Some(2),
            speed: 1,
            torrent: None,
            pinned: false,
            selected: false,
            name_budget: 40,
            palette: &theme.palette,
        });
        let list = rows([(id, row)], &theme.palette);
        assert_eq!(list.as_widget().size().width, Length::Fill);
    }
}
