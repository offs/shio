use super::state::{Menu, Overlay, RowSource, Shio, is_single_file};
use crate::message::{Message, NoticeId, Tab};
use crate::style::{self, Button, Palette, SPACE_L, SPACE_M, SPACE_S, SPACE_XS};
use crate::views::{
    add_dialog, chrome, dialogs, download_list, menus, settings, status_bar, welcome,
};
use crate::widgets::{SoftwareProbe, download_row, icon_button, menu_anchor, modal, text_button};
use iced::widget::text::Wrapping;
use iced::widget::{Text, column, container, row, stack, text};
use iced::{Alignment, Element, Length, window};
use iced_fonts::bootstrap;
use shio_core::{DownloadId, DownloadKind, DownloadStatus, TorrentSource};

impl Shio {
    pub(crate) fn view(&self, _window: window::Id) -> Element<'_, Message> {
        let p = self.palette();
        let maximized = self.window.maximized;

        let screen = if let Some(problem) = &self.startup_problem {
            column![
                chrome::titlebar(chrome::Search::Hidden, maximized, self.window.size.width, p),
                container(dialogs::startup_problem(problem.title, &problem.message, p))
                    .center(Length::Fill)
                    .padding(SPACE_L),
            ]
            .into()
        } else {
            self.main_screen(p)
        };

        let screen = container(screen)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(style::window(*p));
        let window = stack![chrome::resizable(menu_anchor(screen), maximized)];
        if self.software_rendering {
            window.into()
        } else {
            window.push(SoftwareProbe).into()
        }
    }

    fn main_screen<'a>(&'a self, p: &'a Palette) -> Element<'a, Message> {
        let counts = self.counts;
        let tabs = download_list::tabs(
            self.tab,
            [
                (Tab::All, counts.all),
                (Tab::Active, counts.active),
                (Tab::Seeding, counts.seeding),
                (Tab::Done, counts.done),
                (Tab::Failed, counts.failed),
            ],
            self.sort,
            self.sort_descending,
            p,
        );

        let list = if self.rows.is_empty() {
            download_list::empty(!self.search.is_empty(), !self.downloads.is_empty(), p)
        } else {
            let budget = download_row::name_budget(self.window.size.width);
            download_list::rows(
                self.rows.iter().map(|row| {
                    let element = download_row::view(download_row::RowView {
                        id: row.id,
                        name: self.row_name(row),
                        error: self.row_error(row),
                        status: row.status,
                        downloaded: row.downloaded,
                        total: row.total,
                        speed: row.speed,
                        torrent: row.torrent,
                        pinned: row.pinned,
                        selected: self.selection.contains(&row.id),
                        name_budget: budget,
                        palette: p,
                    });
                    (row.id, element)
                }),
                p,
            )
        };

        let suggestions = self.suggestions();
        let search = if matches!(self.overlay, Overlay::None) {
            chrome::Search::Live {
                query: &self.search,
                focused: self.search_focus.is_some(),
                suggesting: !suggestions.is_empty(),
            }
        } else {
            chrome::Search::Inert(&self.search)
        };
        let base: Element<'a, Message> = column![
            chrome::titlebar(search, self.window.maximized, self.window.size.width, p),
            self.banners(p),
            tabs,
            container(list).height(Length::Fill),
            status_bar::view(
                &self.totals,
                self.toast
                    .as_ref()
                    .map(|toast| (toast.kind, toast.message.as_str())),
                self.config.clipboard_monitor,
                p,
            ),
        ]
        .into();

        // The base stays the first stack child so overlays never reset its widget state.
        let mut layers = vec![base];
        if !suggestions.is_empty() {
            layers.push(menus::search_dropdown(
                &suggestions,
                self.search_focus.and_then(|focus| focus.highlighted),
                self.window.size,
                p,
            ));
        }
        let menu = match self.menu {
            Some(Menu::Sort) => Some(menus::sort_menu(
                self.sort,
                self.sort_descending,
                self.window.size,
                p,
            )),
            Some(Menu::Row { id, at }) => self
                .row_menu(id)
                .map(|menu| menus::row_menu(&menu, at, self.window.size, p)),
            None => None,
        };
        layers.extend(menu);

        let dismiss = Some(Message::CloseOverlay);
        let dialog = match &self.overlay {
            Overlay::None => None,
            Overlay::Welcome(step) => Some(modal(
                welcome::view(
                    *step,
                    &self.config,
                    &self.theme_catalog,
                    self.blur_active,
                    self.noctalia.is_some(),
                    p,
                ),
                None,
                p,
            )),
            Overlay::Add(add) => Some(modal(add_dialog::view(add, p), dismiss, p)),
            Overlay::Settings(draft) => Some(modal(
                settings::view(
                    draft,
                    &self.config,
                    &self.theme_catalog,
                    self.blur_active,
                    self.noctalia.is_some(),
                    p,
                ),
                dismiss,
                p,
            )),
            Overlay::ConfirmRemove(ids) => Some(modal(
                dialogs::confirm_remove(self.single_name(ids), ids.len(), p),
                dismiss,
                p,
            )),
            Overlay::ConfirmCancel(ids) => Some(modal(
                dialogs::confirm_cancel(self.single_name(ids), ids.len(), p),
                dismiss,
                p,
            )),
            Overlay::Password { id, input } => Some(modal(
                dialogs::password(self.name_of(*id).unwrap_or_default(), input, p),
                dismiss,
                p,
            )),
        };
        layers.extend(dialog);
        stack(layers).into()
    }

    fn name_of(&self, id: DownloadId) -> Option<&str> {
        self.download(id)
            .map(|download| download.filename.as_str())
            .or_else(|| {
                self.packages
                    .iter()
                    .find(|package| package.id.row_id() == id)
                    .map(|package| package.name.as_str())
            })
    }

    fn single_name(&self, ids: &[DownloadId]) -> Option<&str> {
        match ids {
            [id] => self.name_of(*id),
            _ => None,
        }
    }

    fn row_menu(&self, id: DownloadId) -> Option<menus::RowMenu> {
        let row = self.row(id)?;
        let (can_open_file, can_copy_link) = match row.source {
            RowSource::Download(index) => {
                let download = &self.downloads[index];
                let finished = matches!(
                    download.status,
                    DownloadStatus::Completed
                        | DownloadStatus::Seeding
                        | DownloadStatus::ExtractError
                        | DownloadStatus::PasswordRequired
                );
                let link = match &download.kind {
                    DownloadKind::Http(http) => !http.url.is_empty(),
                    DownloadKind::Torrent(torrent) => {
                        matches!(torrent.source, TorrentSource::Magnet(_))
                    },
                };
                (finished && is_single_file(&download.kind), link)
            },
            RowSource::Package(_) => (false, false),
        };
        Some(menus::RowMenu {
            id,
            status: row.status,
            pinned: row.pinned,
            torrent: row.torrent.is_some(),
            can_open_file,
            can_copy_link,
        })
    }

    // Present even when empty so the list keeps its column slot and scroll position.
    fn banners<'a>(&'a self, p: &Palette) -> Element<'a, Message> {
        let notices = self.notices.iter().map(|notice| {
            let tone = match notice.id {
                NoticeId::ConfigSave => p.warning,
                NoticeId::EngineStopped => p.error,
            };
            banner(
                bootstrap::exclamation_triangle_fill().color(tone),
                notice.title,
                text(&notice.message).wrapping(Wrapping::WordOrGlyph),
                [
                    text_button(
                        "open logs",
                        Button::Secondary,
                        Some(Message::OpenLogsFolder),
                        p,
                    ),
                    icon_button(
                        bootstrap::x_lg(),
                        "dismiss",
                        Some(Message::DismissNotice(notice.id)),
                        p,
                    ),
                ],
                p,
            )
        });
        let offer = self.clipboard_offer.as_ref().map(|offer| {
            banner(
                bootstrap::link_fourfivedeg().color(p.accent),
                "add from clipboard?",
                text(&offer.name).wrapping(Wrapping::None),
                [
                    text_button("add", Button::Primary, Some(Message::AddFromClipboard), p),
                    icon_button(
                        bootstrap::x_lg(),
                        "dismiss",
                        Some(Message::DismissClipboardOffer),
                        p,
                    ),
                ],
                p,
            )
        });
        column(notices.chain(offer))
            .spacing(SPACE_XS)
            .padding([0.0, SPACE_S])
            .into()
    }
}

fn banner<'a>(
    glyph: Text<'a>,
    title: &'a str,
    detail: Text<'a>,
    actions: [Element<'a, Message>; 2],
    p: &Palette,
) -> Element<'a, Message> {
    container(
        row![
            glyph.size(12),
            text(title).size(style::TEXT_SMALL).color(p.text_primary),
            container(detail.size(style::TEXT_SMALL).color(p.text_secondary))
                .width(Length::Fill)
                .clip(true),
        ]
        .extend(actions)
        .spacing(SPACE_S)
        .align_y(Alignment::Center),
    )
    .padding([SPACE_XS, SPACE_M])
    .style(style::rounded(p.hover, style::RADIUS_FIELD))
    .into()
}
