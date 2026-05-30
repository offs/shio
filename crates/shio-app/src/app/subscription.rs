use super::state::Shio;
use crate::clipboard;
use crate::message::Message;
use crate::tray::{self, TrayEvent};
use iced::Subscription;
use iced::futures::SinkExt;
use parking_lot::Mutex;
use shio_core::ProgressStream;
use std::sync::OnceLock;
use std::time::Duration;

const CLIPBOARD_POLL: Duration = Duration::from_millis(500);
const TRAY_POLL: Duration = Duration::from_millis(100);
const PROGRESS_BATCH: Duration = Duration::from_millis(16);

static PROGRESS_RX: OnceLock<Mutex<Option<ProgressStream>>> = OnceLock::new();

struct FrameNeeds<'a> {
    has_toasts: bool,
    scroll_long_names: bool,
    downloads: &'a [shio_core::Download],
}

impl FrameNeeds<'_> {
    fn active(&self) -> bool {
        self.has_toasts || self.needs_name_carousel()
    }

    fn needs_name_carousel(&self) -> bool {
        self.scroll_long_names
            && self
                .downloads
                .iter()
                .any(|download| download.filename.chars().count() > 34)
    }
}

pub(super) fn install_progress_receiver(rx: ProgressStream) {
    let slot = PROGRESS_RX.get_or_init(|| Mutex::new(None));
    *slot.lock() = Some(rx);
}

impl Shio {
    pub(crate) fn subscription(&self) -> Subscription<Message> {
        let frames = self.needs_frames();
        let _span = tracing::trace_span!("subscription", downloads = self.downloads.len(), frames)
            .entered();
        let mut parts: Vec<Subscription<Message>> = Vec::with_capacity(6);

        parts.push(Subscription::run(progress_stream));
        parts.push(events(self.column_resize.is_some()));
        parts.push(clipboard_monitor(self.config.clipboard_monitor));
        parts.push(tray_subscription());

        if frames {
            parts.push(iced::window::frames().map(Message::Frame));
        }

        Subscription::batch(parts)
    }

    fn needs_frames(&self) -> bool {
        FrameNeeds {
            has_toasts: !self.toasts.is_empty(),
            scroll_long_names: self.config.scroll_long_names,
            downloads: &self.downloads,
        }
        .active()
    }
}

fn events(resizing_column: bool) -> Subscription<Message> {
    if resizing_column {
        iced::event::listen_with(event_with_column_resize)
    } else {
        iced::event::listen_with(event_without_column_resize)
    }
}

fn event_without_column_resize(
    event: iced::Event,
    _status: iced::event::Status,
    id: iced::window::Id,
) -> Option<Message> {
    match event {
        iced::Event::Keyboard(iced::keyboard::Event::KeyPressed { key, modifiers, .. }) => {
            Some(Message::KeyPressed(key, modifiers))
        },
        iced::Event::Keyboard(iced::keyboard::Event::ModifiersChanged(modifiers)) => {
            Some(Message::ModifiersChanged(modifiers))
        },
        iced::Event::Window(iced::window::Event::Opened { size, .. }) => {
            Some(Message::WindowOpened { id, size })
        },
        iced::Event::Window(iced::window::Event::Focused) => Some(Message::WindowFocused(true)),
        iced::Event::Window(iced::window::Event::Unfocused) => Some(Message::WindowFocused(false)),
        iced::Event::Window(iced::window::Event::Resized(size)) => {
            Some(Message::WindowResized(size))
        },
        iced::Event::Window(iced::window::Event::FileDropped(path)) => file_dropped(path),
        iced::Event::Window(iced::window::Event::CloseRequested) => Some(Message::WindowClose),
        _ => None,
    }
}

fn event_with_column_resize(
    event: iced::Event,
    status: iced::event::Status,
    id: iced::window::Id,
) -> Option<Message> {
    match event {
        iced::Event::Mouse(iced::mouse::Event::CursorMoved { position }) => {
            Some(Message::ColumnResizeMove(position.x))
        },
        iced::Event::Mouse(iced::mouse::Event::ButtonReleased(iced::mouse::Button::Left)) => {
            Some(Message::ColumnResizeEnd)
        },
        event => event_without_column_resize(event, status, id),
    }
}

fn file_dropped(path: std::path::PathBuf) -> Option<Message> {
    let ext = path.extension()?.to_str()?;
    if ext.eq_ignore_ascii_case("torrent") {
        return Some(Message::TorrentFilesDropped(vec![path]));
    }
    if ext.eq_ignore_ascii_case("url") || ext.eq_ignore_ascii_case("desktop") {
        return Some(Message::FileDropped(path));
    }
    None
}

fn progress_stream() -> iced::futures::stream::BoxStream<'static, Message> {
    use iced::futures::stream::StreamExt;

    iced::stream::channel(8, async |mut output| {
        let Some(mut rx) = PROGRESS_RX
            .get()
            .and_then(|slot| slot.lock().as_ref().map(ProgressStream::resubscribe))
        else {
            std::future::pending::<()>().await;
            return;
        };

        while let Some(first) = rx.recv().await {
            let mut batch = vec![first];
            let deadline = tokio::time::sleep(PROGRESS_BATCH);
            tokio::pin!(deadline);
            loop {
                tokio::select! {
                    () = &mut deadline => break,
                    next = rx.recv() => {
                        let Some(p) = next else {
                            if !batch.is_empty() {
                                let _ = output.send(Message::ProgressTick(batch)).await;
                            }
                            return;
                        };
                        batch.push(p);
                    }
                }
            }

            if output.send(Message::ProgressTick(batch)).await.is_err() {
                return;
            }
        }
    })
    .boxed()
}

fn clipboard_monitor(enabled: bool) -> Subscription<Message> {
    if !enabled {
        return Subscription::none();
    }

    Subscription::run(|| {
        iced::stream::channel(
            4,
            async |mut output: iced::futures::channel::mpsc::Sender<Message>| {
                let mut last = String::new();
                loop {
                    tokio::time::sleep(CLIPBOARD_POLL).await;
                    let Some(text) = read_clipboard().await else {
                        continue;
                    };
                    if text == last {
                        continue;
                    }
                    last.clone_from(&text);
                    if clipboard::is_downloadable_url(&text) {
                        let _ = output.send(Message::ClipboardUrl(text)).await;
                    }
                }
            },
        )
    })
}

async fn read_clipboard() -> Option<String> {
    tokio::task::spawn_blocking(|| arboard::Clipboard::new().ok()?.get_text().ok())
        .await
        .ok()
        .flatten()
}

fn tray_subscription() -> Subscription<Message> {
    Subscription::run(|| {
        iced::stream::channel(
            8,
            async |mut output: iced::futures::channel::mpsc::Sender<Message>| {
                let Some(rx) = tray::subscribe() else {
                    std::future::pending::<()>().await;
                    return;
                };
                loop {
                    tokio::time::sleep(TRAY_POLL).await;
                    while let Ok(event) = rx.try_recv() {
                        let msg = match event {
                            TrayEvent::Show => Message::TrayShow,
                            TrayEvent::PauseAll => Message::PauseAll,
                            TrayEvent::ResumeAll => Message::ResumeAll,
                            TrayEvent::Quit => Message::TrayQuit,
                        };
                        if output.send(msg).await.is_err() {
                            return;
                        }
                    }
                }
            },
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use shio_core::{Download, DownloadStatus};
    use std::path::PathBuf;

    fn needs(downloads: &[Download]) -> FrameNeeds<'_> {
        FrameNeeds {
            has_toasts: false,
            scroll_long_names: false,
            downloads,
        }
    }

    fn download(status: DownloadStatus) -> Download {
        let mut download =
            Download::new("https://example.com/file.zip".into(), PathBuf::from("/tmp"));
        download.status = status;
        download
    }

    #[test]
    fn frame_needs_idle_false() {
        assert!(!needs(&[]).active());
    }

    #[test]
    fn frame_needs_downloading_only_false() {
        assert!(!needs(&[download(DownloadStatus::Downloading)]).active());
    }

    #[test]
    fn frame_needs_starting_only_false() {
        assert!(!needs(&[download(DownloadStatus::Starting)]).active());
    }

    #[test]
    fn frame_needs_toast_present_true() {
        let mut needs = needs(&[]);
        needs.has_toasts = true;

        assert!(needs.active());
    }

    #[test]
    fn frame_needs_long_name_carousel_true() {
        let mut download = download(DownloadStatus::Completed);
        download.filename = "abcdefghijklmnopqrstuvwxyz123456789".to_string();
        let downloads = [download];
        let mut needs = needs(&downloads);
        needs.scroll_long_names = true;

        assert!(needs.active());
    }
}
