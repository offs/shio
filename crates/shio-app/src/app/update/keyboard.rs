use super::super::state::{Overlay, Shio, is_pausable};
use crate::message::{Message, Tab};
use iced::Task;
use iced::keyboard::{Key, Modifiers, key::Named};
use iced::widget::operation;
use shio_core::{DownloadId, DownloadStatus, EngineCommand};

impl Shio {
    pub(super) fn key_pressed(
        &mut self,
        key: &Key,
        modifiers: Modifiers,
        captured: bool,
    ) -> Task<Message> {
        if *key == Key::Named(Named::Escape) {
            return self.escape();
        }
        if captured || self.startup_problem.is_some() {
            return Task::none();
        }
        let idle = matches!(self.overlay, Overlay::None);
        match key {
            Key::Named(Named::Tab) if modifiers.shift() => operation::focus_previous(),
            Key::Named(Named::Tab) => operation::focus_next(),
            Key::Named(Named::Enter) if modifiers.command() => self.add_submit(),
            Key::Character(text) if modifiers.command() => {
                self.command_shortcut(text, modifiers.shift(), idle)
            },
            _ if !idle => Task::none(),
            Key::Named(Named::Space) => self.first_selected().map_or_else(Task::none, |id| {
                if self.selected_rows_any(is_pausable) {
                    self.pause(id)
                } else {
                    self.resume(id)
                }
            }),
            Key::Named(Named::Delete | Named::Backspace) => self
                .first_selected()
                .map_or_else(Task::none, |id| self.request_remove(id)),
            Key::Named(Named::F5) => self.first_selected().map_or_else(Task::none, |id| {
                let ids = self.targets(id, |row| row.status == DownloadStatus::Error);
                self.command_each(ids, |id, reply| EngineCommand::Retry { id, reply })
            }),
            Key::Character(text) if !modifiers.alt() => {
                let tab = match text.as_str() {
                    "1" => Tab::All,
                    "2" => Tab::Active,
                    "3" => Tab::Seeding,
                    "4" => Tab::Done,
                    "5" => Tab::Failed,
                    _ => return Task::none(),
                };
                self.update(Message::TabSelected(tab))
            },
            _ => Task::none(),
        }
    }

    fn command_shortcut(&mut self, text: &str, shift: bool, idle: bool) -> Task<Message> {
        let key = text.to_ascii_lowercase();
        let adding = matches!(self.overlay, Overlay::Add(_));
        match key.as_str() {
            "n" if idle => self.open_add(""),
            "v" if idle || adding => iced::clipboard::read().map(Message::Pasted),
            "f" if idle => operation::focus(crate::views::SEARCH_ID),
            "," => self.open_settings(),
            "p" if shift => self.update(Message::ResumeAll),
            "p" => self.update(Message::PauseAll),
            "q" => self.quit(),
            "a" if idle => self.select_all(),
            "c" if idle => self
                .first_selected()
                .map_or_else(Task::none, |id| self.copy_url(id)),
            _ => Task::none(),
        }
    }

    fn escape(&mut self) -> Task<Message> {
        if self.menu.take().is_some() {
            Task::none()
        } else if !self.suggestions().is_empty()
            && let Some(focus) = &mut self.search_focus
        {
            focus.closed = true;
            Task::none()
        } else if !matches!(self.overlay, Overlay::None) {
            self.close_overlay()
        } else if !self.search.is_empty() {
            self.set_search(String::new())
        } else {
            self.selection.clear();
            self.anchor = None;
            Task::none()
        }
    }

    fn first_selected(&self) -> Option<DownloadId> {
        self.rows
            .iter()
            .find(|row| self.selection.contains(&row.id))
            .map(|row| row.id)
    }

    fn selected_rows_any(&self, predicate: fn(DownloadStatus) -> bool) -> bool {
        self.rows
            .iter()
            .any(|row| self.selection.contains(&row.id) && predicate(row.status))
    }
}
