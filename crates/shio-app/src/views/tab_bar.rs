use crate::message::{Message, Tab};
use crate::style;
use iced::widget::{Space, button, container, row, text};
use iced::{Element, Length};
use shio_core::{Download, DownloadStatus};

pub(crate) fn view<'a>(
    active_tab: Tab,
    downloads: &[Download],
    p: &'a crate::style::Palette,
) -> Element<'a, Message> {
    let tabs = Tab::ALL.iter().fold(
        row![].spacing(2).align_y(iced::Alignment::Center),
        |r, &tab| {
            let count = count_for_tab(tab, downloads);
            let label = format!("{} {}", tab.label().to_lowercase(), count);
            let is_active = tab == active_tab;

            r.push(
                button(text(label).size(12))
                    .style(style::tab(p, is_active))
                    .on_press(Message::TabSelected(tab))
                    .padding([8, 14]),
            )
        },
    );

    let content = row![Space::new().width(16), tabs]
        .align_y(iced::Alignment::Center)
        .height(36);

    container(content)
        .style(style::tab_bar(p))
        .width(Length::Fill)
        .into()
}

fn count_for_tab(tab: Tab, downloads: &[Download]) -> usize {
    match tab {
        Tab::All => downloads.len(),
        Tab::Active => downloads
            .iter()
            .filter(|d| {
                matches!(
                    d.status,
                    DownloadStatus::Downloading
                        | DownloadStatus::Starting
                        | DownloadStatus::Extracting
                        | DownloadStatus::FetchingMetadata
                        | DownloadStatus::Seeding
                )
            })
            .count(),
        Tab::Completed => downloads
            .iter()
            .filter(|d| d.status == DownloadStatus::Completed)
            .count(),
        Tab::Queued => downloads
            .iter()
            .filter(|d| matches!(d.status, DownloadStatus::Queued | DownloadStatus::Pending))
            .count(),
        Tab::Errors => downloads.iter().filter(|d| d.status.is_failed()).count(),
    }
}
