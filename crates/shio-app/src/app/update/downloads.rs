use super::super::state::{ColumnResize, Overlay, Shio};
use crate::message::{DownloadColumn, EngineAction, Message};
use iced::Task;
use shio_core::{DownloadId, DownloadStatus, EngineCommand};

const COLUMN_RESIZE_MIN_WIDTH: f32 = 72.0;
const NAME_COLUMN_MIN_WIDTH: f32 = 140.0;

impl Shio {
    pub(super) fn pause_download(&self, id: DownloadId) -> Task<Message> {
        self.send_engine_cmd_with_action(
            move |reply| EngineCommand::Pause { id, reply },
            EngineAction::SetStatus {
                id,
                status: DownloadStatus::Paused,
                clear_error: false,
            },
        )
    }

    pub(super) fn resume_download(&self, id: DownloadId) -> Task<Message> {
        self.send_engine_cmd_with_action(
            move |reply| EngineCommand::Resume { id, reply },
            EngineAction::SetStatus {
                id,
                status: DownloadStatus::Queued,
                clear_error: false,
            },
        )
    }

    pub(super) fn cancel_download(&self, id: DownloadId) -> Task<Message> {
        self.send_engine_cmd_with_action(
            move |reply| EngineCommand::Cancel { id, reply },
            EngineAction::SetStatus {
                id,
                status: DownloadStatus::Cancelled,
                clear_error: false,
            },
        )
    }

    pub(super) fn retry_download(&self, id: DownloadId) -> Task<Message> {
        self.send_engine_cmd_with_action(
            move |reply| EngineCommand::Retry { id, reply },
            EngineAction::SetStatus {
                id,
                status: DownloadStatus::Queued,
                clear_error: true,
            },
        )
    }

    pub(super) fn force_recheck(&self, id: DownloadId) -> Task<Message> {
        self.send_engine_cmd_with_action(
            move |reply| EngineCommand::ForceRecheck { id, reply },
            EngineAction::SetStatus {
                id,
                status: DownloadStatus::Queued,
                clear_error: false,
            },
        )
    }

    pub(super) fn retry_extract(&self, id: DownloadId) -> Task<Message> {
        self.start_extract(id, None)
    }

    pub(super) fn stop_seeding(&self, id: DownloadId) -> Task<Message> {
        self.send_engine_cmd_with_action(
            move |reply| EngineCommand::StopSeeding { id, reply },
            EngineAction::SetStatus {
                id,
                status: DownloadStatus::Completed,
                clear_error: false,
            },
        )
    }

    pub(super) fn start_extract(&self, id: DownloadId, password: Option<String>) -> Task<Message> {
        self.send_engine_cmd_with_action(
            move |reply| EngineCommand::RetryExtract {
                id,
                password,
                reply,
            },
            EngineAction::SetStatus {
                id,
                status: DownloadStatus::Extracting,
                clear_error: true,
            },
        )
    }

    pub(super) fn remove_download(&self, id: DownloadId) -> Task<Message> {
        self.remove_ids(&[id], false)
    }

    pub(super) fn remove_ids(&self, ids: &[DownloadId], delete_files: bool) -> Task<Message> {
        if ids.is_empty() {
            return Task::none();
        }
        let tasks: Vec<_> = ids
            .iter()
            .map(|&id| {
                self.send_engine_cmd_with_action(
                    move |reply| EngineCommand::Remove {
                        id,
                        delete_files,
                        reply,
                    },
                    EngineAction::Remove { id },
                )
            })
            .collect();
        Task::batch(tasks)
    }

    pub(super) fn request_password(&mut self, id: DownloadId) -> Task<Message> {
        if self
            .downloads
            .iter()
            .any(|d| d.id == id && d.status == DownloadStatus::PasswordRequired)
        {
            self.overlay = Overlay::Password(id);
            self.password_input.clear();
        }
        Task::none()
    }

    pub(super) fn password_changed(&mut self, value: String) -> Task<Message> {
        self.password_input = value;
        Task::none()
    }

    pub(super) fn confirm_password(&mut self, id: DownloadId) -> Task<Message> {
        if self.password_input.is_empty() {
            return Task::none();
        }
        let password = std::mem::take(&mut self.password_input);
        self.overlay = Overlay::None;
        self.start_extract(id, Some(password))
    }

    pub(super) fn cancel_password(&mut self) -> Task<Message> {
        self.overlay = Overlay::None;
        self.password_input.clear();
        Task::none()
    }

    pub(super) fn toggle_pin(&self, id: DownloadId) -> Task<Message> {
        let Some(dl) = self.downloads.iter().find(|d| d.id == id) else {
            return Task::none();
        };
        let pinned = !dl.pinned;
        self.send_engine_cmd_with_action(
            move |reply| EngineCommand::SetPin { id, pinned, reply },
            EngineAction::SetPin { id, pinned },
        )
    }

    pub(super) fn pause_all(&self) -> Task<Message> {
        self.send_engine_cmd_with_action(
            |reply| EngineCommand::PauseAll { reply },
            EngineAction::PauseAll,
        )
    }

    pub(super) fn resume_all(&self) -> Task<Message> {
        self.send_engine_cmd_with_action(
            |reply| EngineCommand::ResumeAll { reply },
            EngineAction::ResumeAll,
        )
    }

    pub(super) fn column_resize_start(&mut self, column: DownloadColumn) -> Task<Message> {
        self.column_resize = Some(ColumnResize {
            column,
            last_x: None,
        });
        Task::none()
    }

    pub(super) fn column_resize_move(&mut self, x: f32) -> Task<Message> {
        let Some(resize) = &mut self.column_resize else {
            return Task::none();
        };
        let Some(last_x) = resize.last_x.replace(x) else {
            return Task::none();
        };
        let width = self.column_widths.get(resize.column) + x - last_x;
        self.column_widths
            .set(resize.column, width.max(min_column_width(resize.column)));
        Task::none()
    }

    pub(super) fn column_resize_end(&mut self) -> Task<Message> {
        self.column_resize = None;
        Task::none()
    }
}

const fn min_column_width(column: DownloadColumn) -> f32 {
    match column {
        DownloadColumn::Name => NAME_COLUMN_MIN_WIDTH,
        DownloadColumn::Size
        | DownloadColumn::Progress
        | DownloadColumn::Speed
        | DownloadColumn::Eta => COLUMN_RESIZE_MIN_WIDTH,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::DownloadColumnWidths;

    #[test]
    fn column_widths_have_expected_defaults() {
        let widths = DownloadColumnWidths::default();

        assert_eq!(widths.get(DownloadColumn::Name), 280.0);
    }
}
