#![expect(
    clippy::unused_self,
    reason = "update handlers stay as Shio methods for dispatch consistency"
)]

use super::super::state::Shio;
use super::SEARCH_INPUT_ID;
use crate::message::{Message, SortCol, SortDirection, Tab};
use iced::Task;
use shio_core::DownloadId;

impl Shio {
    pub(super) fn select_clicked(&mut self, id: DownloadId) -> Task<Message> {
        self.drag_hover = None;
        if self.modifiers.shift() {
            self.range_select(id);
        } else if self.modifiers.command() {
            self.toggle_selection(id);
        } else {
            self.select_single(id);
        }
        Task::none()
    }

    pub(super) fn select_all(&mut self) -> Task<Message> {
        self.select_all_visible();
        Task::none()
    }

    pub(super) fn tab_selected(&mut self, tab: Tab) -> Task<Message> {
        self.active_tab = tab;
        Task::none()
    }

    pub(super) fn search_text_changed(&mut self, text: String) -> Task<Message> {
        self.search_text = text;
        self.reparse_search();
        Task::none()
    }

    pub(super) fn search_focus(&self) -> Task<Message> {
        iced::widget::operation::focus(SEARCH_INPUT_ID.clone())
    }

    pub(super) fn search_apply_suggestion(&mut self, value: &str) -> Task<Message> {
        self.search_text = crate::search::apply_suggestion(&self.search_text, value);
        self.reparse_search();
        iced::widget::operation::focus(SEARCH_INPUT_ID.clone())
    }

    pub(super) fn sort_column(&mut self, col: SortCol) -> Task<Message> {
        self.manual_order = false;
        if self.sort_column == col {
            self.sort_direction = self.sort_direction.toggle();
        } else {
            self.sort_column = col;
            self.sort_direction = SortDirection::Ascending;
        }
        Task::none()
    }
}
