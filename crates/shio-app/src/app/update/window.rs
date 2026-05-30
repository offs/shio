#![expect(
    clippy::unused_self,
    reason = "update handlers stay as Shio methods for dispatch consistency"
)]

use super::super::state::{Shio, ToastKind};
use super::super::vibrancy::apply_vibrancy;
use crate::message::Message;
use crate::widgets::download_row::copyable_source;
use iced::Task;
use iced::widget::text_editor;
use shio_core::{DownloadId, EngineCommand};
use std::path::PathBuf;
use std::time::Duration;

const SHUTDOWN_SEND_TIMEOUT: Duration = Duration::from_millis(300);
const SHUTDOWN_ACK_TIMEOUT: Duration = Duration::from_secs(5);

const fn close_to_tray_mode() -> iced::window::Mode {
    iced::window::Mode::Hidden
}

const fn tray_show_mode() -> iced::window::Mode {
    iced::window::Mode::Windowed
}

impl Shio {
    pub(super) fn open_file(&self, id: DownloadId) -> Task<Message> {
        if let Some(dl) = self.downloads.iter().find(|d| d.id == id) {
            let path = dl.file_path();
            return Task::perform(open_path(path), Message::OpenFileCompleted);
        }
        Task::none()
    }

    pub(super) fn open_folder(&self, id: DownloadId) -> Task<Message> {
        if let Some(dl) = self.downloads.iter().find(|d| d.id == id) {
            let path = dl.file_dir();
            return Task::perform(open_path(path), Message::OpenFolderCompleted);
        }
        Task::none()
    }

    pub(super) fn copy_url(&self, id: DownloadId) -> Task<Message> {
        let url = self
            .downloads
            .iter()
            .find(|d| d.id == id)
            .and_then(|d| copyable_source(d).map(String::from));
        let Some(url) = url else {
            return Task::none();
        };
        Task::perform(copy_text(url), Message::CopyUrlCompleted)
    }

    pub(super) fn open_file_completed(&mut self, result: Result<(), String>) -> Task<Message> {
        if let Err(message) = result {
            tracing::warn!("open file failed: {message}");
            self.push_toast("open file failed", ToastKind::Error);
        }
        Task::none()
    }

    pub(super) fn open_folder_completed(&mut self, result: Result<(), String>) -> Task<Message> {
        if let Err(message) = result {
            tracing::warn!("open folder failed: {message}");
            self.push_toast("open folder failed", ToastKind::Error);
        }
        Task::none()
    }

    pub(super) fn copy_url_completed(&mut self, result: Result<(), String>) -> Task<Message> {
        match result {
            Ok(()) => self.push_toast("url copied", ToastKind::Info),
            Err(message) => {
                tracing::warn!("clipboard write failed: {message}");
                self.push_toast("copy failed", ToastKind::Error);
            },
        }
        Task::none()
    }

    pub(super) fn clipboard_url(&mut self, url: String) -> Task<Message> {
        if self.show_first_run() || url == self.last_clipboard_url {
            return Task::none();
        }
        let reset_task = self.reset_add_dialog_state();
        self.add_urls = text_editor::Content::with_text(&url);
        let refresh_task = self.refresh_add_sources();
        self.last_clipboard_url = url;
        Task::batch([reset_task, refresh_task])
    }

    pub(super) fn url_dropped(&mut self, url: &str) -> Task<Message> {
        if self.show_first_run() {
            return Task::none();
        }
        let reset_task = self.reset_add_dialog_state();
        self.add_urls = text_editor::Content::with_text(url);
        let refresh_task = self.refresh_add_sources();
        Task::batch([reset_task, refresh_task])
    }

    pub(super) fn file_dropped(&self, path: PathBuf) -> Task<Message> {
        Task::perform(read_dropped_shortcut(path), Message::DroppedShortcutRead)
    }

    pub(super) fn dropped_shortcut_read(&mut self, url: Option<String>) -> Task<Message> {
        let Some(url) = url else {
            return Task::none();
        };
        self.url_dropped(&url)
    }

    pub(super) fn window_opened(
        &mut self,
        id: iced::window::Id,
        size: iced::Size,
    ) -> Task<Message> {
        self.window.id = Some(id);
        self.window.focused = true;
        self.window.size = Some(size);
        tracing::info!("window opened: {:?}", id);
        apply_vibrancy(id, self.config.window.material)
    }

    pub(super) fn window_focused(&mut self, focused: bool) -> Task<Message> {
        self.window.focused = focused;
        Task::none()
    }

    pub(super) fn window_resized(&mut self, size: iced::Size) -> Task<Message> {
        self.window.size = Some(size);
        Task::none()
    }

    pub(super) fn window_minimize(&self) -> Task<Message> {
        self.latest_window()
            .and_then(|id| iced::window::minimize(id, true))
    }

    pub(super) fn window_maximize_toggle(&mut self) -> Task<Message> {
        self.window.maximized = !self.window.maximized;
        self.latest_window().and_then(iced::window::toggle_maximize)
    }

    pub(super) fn window_close(&self) -> Task<Message> {
        if cfg!(target_os = "windows") && self.config.close_to_tray {
            self.latest_window()
                .and_then(|id| iced::window::set_mode(id, close_to_tray_mode()))
        } else {
            Task::done(Message::AppExit)
        }
    }

    pub(super) fn window_drag_start(&self) -> Task<Message> {
        self.latest_window().and_then(iced::window::drag)
    }

    pub(super) fn app_exit(&self) -> Task<Message> {
        let tx = self.engine_tx.clone();
        Task::perform(
            async move {
                let (cmd, ack) = EngineCommand::shutdown();
                let notified = ack.notified();
                match tokio::time::timeout(SHUTDOWN_SEND_TIMEOUT, tx.send(cmd)).await {
                    Ok(Ok(())) => {
                        if tokio::time::timeout(SHUTDOWN_ACK_TIMEOUT, notified)
                            .await
                            .is_err()
                        {
                            tracing::warn!("engine shutdown acknowledgement timed out");
                        }
                    },
                    Ok(Err(e)) => tracing::warn!("engine shutdown command dropped: {e}"),
                    Err(_) => tracing::warn!("engine shutdown command timed out"),
                }
            },
            |()| std::process::exit(0),
        )
    }

    pub(super) fn tray_show(&self) -> Task<Message> {
        self.latest_window().and_then(|id| {
            iced::window::set_mode(id, tray_show_mode())
                .chain(iced::window::minimize(id, false))
                .chain(iced::window::gain_focus(id))
        })
    }

    fn latest_window(&self) -> Task<Option<iced::window::Id>> {
        if let Some(id) = self.window.id {
            Task::done(Some(id))
        } else {
            iced::window::latest()
        }
    }
}

async fn open_path(path: PathBuf) -> Result<(), String> {
    tokio::task::spawn_blocking(move || open::that(&path).map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())?
}

async fn copy_text(text: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        let mut clipboard = arboard::Clipboard::new().map_err(|e| e.to_string())?;
        clipboard.set_text(text).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

async fn read_dropped_shortcut(path: PathBuf) -> Option<String> {
    tokio::task::spawn_blocking(move || {
        let content = std::fs::read_to_string(path).ok()?;
        dropped_shortcut_url(&content)
    })
    .await
    .ok()
    .flatten()
}

fn dropped_shortcut_url(content: &str) -> Option<String> {
    for line in content.lines().map(str::trim) {
        if line.starts_with("http://") || line.starts_with("https://") {
            return Some(line.to_string());
        }
        if let Some(url) = line.strip_prefix("URL=") {
            return Some(url.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dropped_shortcut_url_reads_url_key() {
        let content = "[InternetShortcut]\nURL=https://example.com/file.zip\n";

        assert_eq!(
            dropped_shortcut_url(content),
            Some("https://example.com/file.zip".to_string())
        );
    }

    #[test]
    fn dropped_shortcut_url_reads_plain_url() {
        let content = "Name=download\nhttps://example.com/file.zip\n";

        assert_eq!(
            dropped_shortcut_url(content),
            Some("https://example.com/file.zip".to_string())
        );
    }
}
