use super::super::state::{Overlay, Shio};
use crate::message::{Message, Tab};
use iced::Task;
use iced::keyboard::{Key, Modifiers, key::Named};
use shio_core::DownloadStatus;

impl Shio {
    pub(super) fn key_pressed(&mut self, key: &Key, modifiers: Modifiers) -> Task<Message> {
        if self.delete_confirm_targets().is_some() {
            return self.key_delete_confirm(key);
        }

        if let Some(task) = self.key_search_suggestion(key) {
            return task;
        }

        match key {
            Key::Named(Named::Escape) => self.key_escape(),
            Key::Named(Named::Enter) if self.show_add_dialog() && modifiers.command() => {
                if self.can_confirm_add_dialog() {
                    self.update(Message::ConfirmAddDownload)
                } else {
                    Task::none()
                }
            },
            Key::Named(Named::Enter) if self.edit_target().is_some() => self.key_enter_edit(),
            Key::Named(Named::Delete) => self.key_delete_selected(),
            Key::Named(Named::Space) if !self.show_add_dialog() && !self.show_settings() => {
                self.key_space_toggle_download()
            },
            Key::Named(Named::F5) => self.key_f5_retry(),
            Key::Character(ch) if modifiers.command() => self.key_command(ch.as_str(), modifiers),
            Key::Character(ch) if self.accepts_plain_shortcut(modifiers) => {
                self.key_plain_shortcut(ch.as_str())
            },
            _ => Task::none(),
        }
    }

    fn key_delete_confirm(&mut self, key: &Key) -> Task<Message> {
        match key {
            Key::Named(Named::Escape) => self.update(Message::CancelDeleteWithFiles),
            Key::Named(Named::Enter) => self.update(Message::ConfirmDeleteFiles),
            _ => Task::none(),
        }
    }

    fn key_search_suggestion(&mut self, key: &Key) -> Option<Task<Message>> {
        let suggestions = crate::search::completions(&self.search_text);
        if suggestions.is_empty() || self.show_add_dialog() || self.show_settings() {
            return None;
        }
        match key {
            Key::Named(Named::ArrowDown) => {
                self.suggestion_index = Some(match self.suggestion_index {
                    Some(i) if i + 1 < suggestions.len() => i + 1,
                    _ => 0,
                });
                Some(Task::none())
            },
            Key::Named(Named::ArrowUp) => {
                self.suggestion_index = Some(match self.suggestion_index {
                    Some(0) | None => suggestions.len().saturating_sub(1),
                    Some(i) => i - 1,
                });
                Some(Task::none())
            },
            Key::Named(Named::Enter | Named::Tab) => {
                let idx = self.suggestion_index?;
                let value = suggestions.get(idx)?.value.to_string();
                Some(self.update(Message::SearchApplySuggestion(value)))
            },
            _ => None,
        }
    }

    fn key_escape(&mut self) -> Task<Message> {
        if self.password_prompt().is_some() {
            return self.update(Message::CancelPassword);
        }
        if self.show_add_dialog() {
            return self.update(Message::CancelAddDownload);
        } else if self.show_settings() {
            return self.update(Message::CloseSettings);
        } else if self.edit_target().is_some() {
            self.overlay = Overlay::None;
        } else if self.suggestion_index.is_some() {
            self.suggestion_index = None;
        } else if self.has_active_search_filters() {
            self.clear_search_filters();
        } else {
            self.clear_selection();
        }
        Task::none()
    }

    const fn has_active_search_filters(&self) -> bool {
        !self.search_text.is_empty()
            || self.search_type.is_some()
            || !matches!(self.search_size, crate::search::SizeFilter::Any)
    }

    fn clear_search_filters(&mut self) {
        self.search_text.clear();
        self.search_query.clear();
        self.search_type = None;
        self.search_size = crate::search::SizeFilter::Any;
        self.suggestion_index = None;
    }

    fn key_enter_edit(&mut self) -> Task<Message> {
        let Some(id) = self.edit_target() else {
            return Task::none();
        };
        if self.edit_filename.trim().is_empty() {
            return Task::none();
        }
        self.update(Message::ConfirmEdit(id))
    }

    fn key_space_toggle_download(&mut self) -> Task<Message> {
        let ids = self.selected_ids();
        if ids.is_empty() {
            return Task::none();
        }
        let any_active = ids.iter().any(|id| {
            self.downloads.iter().any(|d| {
                d.id == *id
                    && matches!(
                        d.status,
                        DownloadStatus::Downloading | DownloadStatus::Starting
                    )
            })
        });
        let msg: fn(shio_core::DownloadId) -> Message = if any_active {
            Message::PauseDownload
        } else {
            Message::ResumeDownload
        };
        self.batch_on_selected(msg, |status| {
            matches!(
                status,
                DownloadStatus::Downloading | DownloadStatus::Starting | DownloadStatus::Paused
            )
        })
    }

    fn key_f5_retry(&mut self) -> Task<Message> {
        self.retry_selected()
    }

    fn retry_selected(&mut self) -> Task<Message> {
        let targets: Vec<(shio_core::DownloadId, DownloadStatus)> = self
            .selected_ids()
            .into_iter()
            .filter_map(|id| {
                self.downloads
                    .iter()
                    .find(|d| d.id == id)
                    .map(|d| (d.id, d.status))
            })
            .filter(|(_, s)| s.is_failed())
            .collect();
        if targets.is_empty() {
            return Task::none();
        }
        let tasks: Vec<Task<Message>> = targets
            .into_iter()
            .map(|(id, s)| match s {
                DownloadStatus::ExtractError => self.update(Message::RetryExtract(id)),
                DownloadStatus::PasswordRequired => self.update(Message::RequestPassword(id)),
                _ => self.update(Message::RetryDownload(id)),
            })
            .collect();
        Task::batch(tasks)
    }

    fn key_delete_selected(&mut self) -> Task<Message> {
        match self.selected_ids().first() {
            Some(id) => self.update(Message::RequestDeleteWithFiles(*id)),
            None => Task::none(),
        }
    }

    fn key_command(&mut self, ch: &str, modifiers: Modifiers) -> Task<Message> {
        match ch {
            "a" => self.update(Message::SelectAll),
            "p" if modifiers.shift() => self.update(Message::ResumeAll),
            "p" => self.update(Message::PauseAll),
            "q" => Task::done(Message::AppExit),
            "n" => self.update(Message::AddDownloadPressed),
            "," => self.update(Message::OpenSettings),
            "f" if self.show_add_dialog() => self.update(Message::AddTorrentSearchFocus),
            "f" if self.show_settings() => self.update(Message::SettingsSearchFocus),
            "f" => self.update(Message::SearchFocus),
            _ => Task::none(),
        }
    }

    fn accepts_plain_shortcut(&self, modifiers: Modifiers) -> bool {
        !self.show_add_dialog()
            && !self.show_settings()
            && !self.show_first_run()
            && self.delete_confirm_targets().is_none()
            && self.edit_target().is_none()
            && !modifiers.command()
    }

    fn key_plain_shortcut(&mut self, ch: &str) -> Task<Message> {
        match ch {
            "1" => {
                self.active_tab = Tab::All;
                Task::none()
            },
            "2" => {
                self.active_tab = Tab::Active;
                Task::none()
            },
            "3" => {
                self.active_tab = Tab::Completed;
                Task::none()
            },
            "4" => {
                self.active_tab = Tab::Queued;
                Task::none()
            },
            "5" => {
                self.active_tab = Tab::Errors;
                Task::none()
            },
            "p" | "P" => self.batch_on_selected(Message::TogglePin, |_| true),
            "r" | "R" => self.retry_selected(),
            "c" | "C" => self.batch_on_selected(Message::CancelDownload, |s| {
                matches!(
                    s,
                    DownloadStatus::Downloading
                        | DownloadStatus::Starting
                        | DownloadStatus::Queued
                        | DownloadStatus::Paused
                )
            }),
            "u" | "U" => self.selected_single_action(Message::CopyUrl),
            "e" | "E" => self.selected_single_action(Message::RequestEdit),
            "f" | "F" => self.selected_single_action(Message::OpenFolder),
            _ => Task::none(),
        }
    }

    fn selected_single_action(
        &mut self,
        to_msg: fn(shio_core::DownloadId) -> Message,
    ) -> Task<Message> {
        let ids = self.selected_ids();
        match ids.first() {
            Some(id) => self.update(to_msg(*id)),
            None => Task::none(),
        }
    }

    fn batch_on_selected(
        &mut self,
        to_msg: fn(shio_core::DownloadId) -> Message,
        filter: fn(DownloadStatus) -> bool,
    ) -> Task<Message> {
        let ids: Vec<_> = self
            .selected_ids()
            .into_iter()
            .filter(|id| {
                self.downloads
                    .iter()
                    .find(|d| d.id == *id)
                    .is_some_and(|d| filter(d.status))
            })
            .collect();
        if ids.is_empty() {
            return Task::none();
        }
        Task::batch(
            ids.into_iter()
                .map(|id| self.update(to_msg(id)))
                .collect::<Vec<_>>(),
        )
    }
}
