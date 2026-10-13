use super::super::state::{
    Menu, Overlay, Row, RowSource, SearchFocus, Shio, ToastKind, is_pausable, is_single_file,
};
use super::window::open_path;
use crate::message::{Message, SortKey};
use crate::search::Query;
use iced::{Point, Task};
use shio_core::{DownloadId, DownloadKind, DownloadStatus, EngineCommand, TorrentSource};
use tokio::sync::oneshot;

type Reply = oneshot::Sender<shio_core::Result<()>>;

impl Shio {
    pub(super) fn set_search(&mut self, text: String) -> Task<Message> {
        self.search = super::super::truncate_text_bytes(text, super::super::MAX_SEARCH_TEXT_BYTES);
        self.query = Query::parse(&self.search);
        if let Some(focus) = &mut self.search_focus {
            *focus = SearchFocus::default();
        }
        self.refresh();
        Task::none()
    }

    pub(super) fn highlight_suggestion(&mut self, down: bool) -> Task<Message> {
        let count = self.suggestions().len();
        if let Some(focus) = &mut self.search_focus
            && count > 0
        {
            focus.highlighted = Some(match (focus.highlighted, down) {
                (Some(i), true) => (i + 1) % count,
                (None, true) => 0,
                (Some(i), false) => (i + count - 1) % count,
                (None, false) => count - 1,
            });
        }
        Task::none()
    }

    pub(super) fn accept_suggestion(&mut self) -> Task<Message> {
        let index = self
            .search_focus
            .and_then(|focus| focus.highlighted)
            .unwrap_or(0);
        match self.suggestions().get(index) {
            Some(suggestion) => self.apply_suggestion(suggestion.value),
            None => Task::none(),
        }
    }

    pub(super) fn apply_suggestion(&mut self, value: &str) -> Task<Message> {
        let text = crate::search::apply(&self.search, value);
        self.set_search(text)
    }

    pub(super) fn sort_by(&mut self, key: SortKey) -> Task<Message> {
        if self.sort == key {
            self.sort_descending = !self.sort_descending;
        } else {
            self.sort = key;
            self.sort_descending = key != SortKey::Name;
        }
        self.menu = None;
        self.refresh();
        Task::none()
    }

    pub(super) fn row_pressed(&mut self, id: DownloadId) -> Task<Message> {
        self.menu = None;
        if self.modifiers.shift()
            && let Some(anchor) = self.anchor
        {
            let position = |target| self.rows.iter().position(|row| row.id == target);
            if let (Some(from), Some(to)) = (position(anchor), position(id)) {
                let range = from.min(to)..=from.max(to);
                self.selection = self.rows[range].iter().map(|row| row.id).collect();
            }
        } else if self.modifiers.command() {
            if !self.selection.remove(&id) {
                self.selection.insert(id);
            }
            self.anchor = Some(id);
        } else {
            self.selection.clear();
            self.selection.insert(id);
            self.anchor = Some(id);
        }
        Task::none()
    }

    pub(super) fn select_all(&mut self) -> Task<Message> {
        self.selection = self.rows.iter().map(|row| row.id).collect();
        self.anchor = self.rows.first().map(|row| row.id);
        Task::none()
    }

    pub(super) fn open_row_menu(&mut self, id: DownloadId, at: Point) -> Task<Message> {
        if !self.selection.contains(&id) {
            self.selection.clear();
            self.selection.insert(id);
            self.anchor = Some(id);
        }
        self.menu = Some(Menu::Row { id, at });
        Task::none()
    }

    pub(super) fn row_double_clicked(&self, id: DownloadId) -> Task<Message> {
        let finished = self.row(id).is_some_and(|row| {
            matches!(
                row.status,
                DownloadStatus::Completed | DownloadStatus::Seeding
            )
        });
        self.open_target(id, finished)
    }

    // An action on a row of a multi-selection applies to its eligible rows.
    pub(super) fn targets(
        &self,
        id: DownloadId,
        eligible: impl Fn(&Row) -> bool,
    ) -> Vec<DownloadId> {
        if self.selection.len() > 1 && self.selection.contains(&id) {
            self.rows
                .iter()
                .filter(|row| self.selection.contains(&row.id) && eligible(row))
                .map(|row| row.id)
                .collect()
        } else {
            vec![id]
        }
    }

    pub(super) fn command_each(
        &mut self,
        ids: Vec<DownloadId>,
        build: impl Fn(DownloadId, Reply) -> EngineCommand,
    ) -> Task<Message> {
        self.menu = None;
        Task::batch(
            ids.into_iter()
                .map(|id| self.command(|reply| build(id, reply)))
                .collect::<Vec<_>>(),
        )
    }

    pub(super) fn pause(&mut self, id: DownloadId) -> Task<Message> {
        let ids = self.targets(id, |row| is_pausable(row.status));
        self.command_each(ids, |id, reply| EngineCommand::Pause { id, reply })
    }

    pub(super) fn resume(&mut self, id: DownloadId) -> Task<Message> {
        let ids = self.targets(id, |row| is_resumable(row.status));
        self.command_each(ids, |id, reply| EngineCommand::Resume { id, reply })
    }

    pub(super) fn retry(&mut self, id: DownloadId) -> Task<Message> {
        let ids = self.targets(id, |row| row.status == DownloadStatus::Error);
        self.command_each(ids, |id, reply| EngineCommand::Retry { id, reply })
    }

    pub(super) fn retry_extract(&mut self, id: DownloadId) -> Task<Message> {
        let ids = self.targets(id, |row| row.status == DownloadStatus::ExtractError);
        self.command_each(ids, |id, reply| EngineCommand::RetryExtract {
            id,
            password: None,
            reply,
        })
    }

    pub(super) fn force_recheck(&mut self, id: DownloadId) -> Task<Message> {
        let ids = self.targets(id, |row| row.torrent.is_some());
        self.command_each(ids, |id, reply| EngineCommand::ForceRecheck { id, reply })
    }

    pub(super) fn stop_seeding(&mut self, id: DownloadId) -> Task<Message> {
        let ids = self.targets(id, |row| row.status == DownloadStatus::Seeding);
        self.command_each(ids, |id, reply| EngineCommand::StopSeeding { id, reply })
    }

    pub(super) fn toggle_pin(&mut self, id: DownloadId) -> Task<Message> {
        let pinned = !self.row(id).is_some_and(|row| row.pinned);
        let ids = self.targets(id, |row| row.pinned != pinned);
        self.command_each(ids, move |id, reply| EngineCommand::SetPin {
            id,
            pinned,
            reply,
        })
    }

    pub(super) fn request_remove(&mut self, id: DownloadId) -> Task<Message> {
        self.menu = None;
        self.overlay = Overlay::ConfirmRemove(self.targets(id, |_| true));
        Task::none()
    }

    pub(super) fn request_cancel(&mut self, id: DownloadId) -> Task<Message> {
        self.menu = None;
        self.overlay = Overlay::ConfirmCancel(self.targets(id, |row| is_cancellable(row.status)));
        Task::none()
    }

    pub(super) fn confirm_remove(&mut self, delete_files: bool) -> Task<Message> {
        let Overlay::ConfirmRemove(ids) = std::mem::replace(&mut self.overlay, Overlay::None)
        else {
            return Task::none();
        };
        self.command_each(ids, |id, reply| EngineCommand::Remove {
            id,
            delete_files,
            reply,
        })
    }

    pub(super) fn confirm_cancel(&mut self) -> Task<Message> {
        let Overlay::ConfirmCancel(ids) = std::mem::replace(&mut self.overlay, Overlay::None)
        else {
            return Task::none();
        };
        self.command_each(ids, |id, reply| EngineCommand::Cancel { id, reply })
    }

    pub(super) fn request_password(&mut self, id: DownloadId) -> Task<Message> {
        self.menu = None;
        if self
            .row(id)
            .is_some_and(|row| row.status == DownloadStatus::PasswordRequired)
        {
            return self.open_password(id);
        }
        Task::none()
    }

    pub(super) fn open_password(&mut self, id: DownloadId) -> Task<Message> {
        self.overlay = Overlay::Password {
            id,
            input: String::new(),
        };
        iced::widget::operation::focus(crate::views::PASSWORD_ID)
    }

    pub(super) fn submit_password(&mut self) -> Task<Message> {
        let Overlay::Password { id, input } = &mut self.overlay else {
            return Task::none();
        };
        if input.is_empty() {
            return Task::none();
        }
        let (id, password) = (*id, std::mem::take(input));
        self.overlay = Overlay::None;
        self.command(|reply| EngineCommand::RetryExtract {
            id,
            password: Some(password),
            reply,
        })
    }

    pub(super) fn open_file(&mut self, id: DownloadId) -> Task<Message> {
        self.menu = None;
        self.open_target(id, true)
    }

    pub(super) fn open_folder(&mut self, id: DownloadId) -> Task<Message> {
        self.menu = None;
        self.open_target(id, false)
    }

    fn open_target(&self, id: DownloadId, file: bool) -> Task<Message> {
        let Some(row) = self.row(id) else {
            return Task::none();
        };
        let path = match row.source {
            RowSource::Package(index) => self.packages[index].folder_path(),
            RowSource::Download(index) => {
                let download = &self.downloads[index];
                if file && is_single_file(&download.kind) {
                    download.file_path()
                } else {
                    download.file_dir()
                }
            },
        };
        open_path(path)
    }

    pub(super) fn copy_url(&mut self, id: DownloadId) -> Task<Message> {
        self.menu = None;
        let link = self.download(id).and_then(|download| match &download.kind {
            DownloadKind::Http(http) => Some(http.url.clone()),
            DownloadKind::Torrent(torrent) => match &torrent.source {
                TorrentSource::Magnet(magnet) => Some(magnet.clone()),
                TorrentSource::File(_) => None,
            },
        });
        let Some(link) = link else {
            return Task::none();
        };
        self.note_clipboard(Some(&link));
        self.push_toast(ToastKind::Info, "link copied");
        iced::clipboard::write(link)
    }
}

const fn is_resumable(status: DownloadStatus) -> bool {
    matches!(status, DownloadStatus::Paused | DownloadStatus::Error)
}

const fn is_cancellable(status: DownloadStatus) -> bool {
    matches!(
        status,
        DownloadStatus::Queued
            | DownloadStatus::Starting
            | DownloadStatus::Downloading
            | DownloadStatus::FetchingMetadata
            | DownloadStatus::Extracting
            | DownloadStatus::Paused
            | DownloadStatus::Error
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tests::{download, id};
    use shio_core::ProgressEvent;
    use tokio::sync::mpsc;

    #[test]
    fn batch_actions_only_target_eligible_rows_of_the_selection() {
        let (engine, _) = mpsc::channel(1);
        let mut app = Shio::build(shio_core::AppConfig::default(), engine);
        let _ = app.apply_engine_events(vec![ProgressEvent::Resync {
            downloads: vec![
                download(1, DownloadStatus::Downloading),
                download(2, DownloadStatus::Paused),
                download(3, DownloadStatus::Completed),
            ],
            packages: Vec::new(),
        }]);
        app.selection.extend([id(1), id(2), id(3)]);

        assert_eq!(app.targets(id(2), |row| is_pausable(row.status)), [id(1)]);
        assert_eq!(app.targets(id(1), |row| is_resumable(row.status)), [id(2)]);
        app.selection.remove(&id(1));
        app.selection.remove(&id(2));
        assert_eq!(app.targets(id(1), |row| is_pausable(row.status)), [id(1)]);
    }
}
