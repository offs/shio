use super::state::Shio;
use crate::message::Message;
use crate::tray::TrayEvent;
use iced::futures::{SinkExt, Stream};
use iced::{Event, Subscription, event, keyboard, window};
use parking_lot::Mutex;
use shio_core::{AppConfig, ProgressStream};
use std::time::Duration;

// A burst of engine events costs one update and one view.
const ENGINE_BATCH: Duration = Duration::from_millis(16);
const THEME_POLL: Duration = Duration::from_secs(2);

static ENGINE_EVENTS: Mutex<Option<ProgressStream>> = Mutex::new(None);

pub(super) fn install_engine_events(stream: ProgressStream) {
    *ENGINE_EVENTS.lock() = Some(stream);
}

impl Shio {
    pub(crate) fn subscription(&self) -> Subscription<Message> {
        let toast = if self.toast.is_some() && self.window.focused {
            iced::time::every(Duration::from_secs(1)).map(|_| Message::ToastTick)
        } else {
            Subscription::none()
        };
        Subscription::batch([
            Subscription::run(engine_events),
            event::listen_with(app_event),
            Subscription::run(tray_events),
            Subscription::run(instance_events),
            Subscription::run(theme_changes),
            toast,
        ])
    }
}

fn app_event(event: Event, status: event::Status, id: window::Id) -> Option<Message> {
    match event {
        Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) => {
            Some(Message::KeyPressed {
                key,
                modifiers,
                captured: status == event::Status::Captured,
            })
        },
        Event::Keyboard(keyboard::Event::ModifiersChanged(modifiers)) => {
            Some(Message::ModifiersChanged(modifiers))
        },
        Event::Window(window::Event::Focused) => Some(Message::WindowFocused(true)),
        Event::Window(window::Event::Unfocused) => Some(Message::WindowFocused(false)),
        Event::Window(window::Event::Resized(size)) => Some(Message::WindowResized(size)),
        Event::Window(window::Event::FileDropped(path)) => Some(Message::FileDropped(path)),
        Event::Window(window::Event::CloseRequested) => Some(Message::WindowClose),
        Event::Window(window::Event::Closed) => Some(Message::WindowClosed(id)),
        _ => None,
    }
}

fn engine_events() -> impl Stream<Item = Message> {
    iced::stream::channel(8, async |mut output| {
        let stream = ENGINE_EVENTS
            .lock()
            .as_ref()
            .map(ProgressStream::resubscribe);
        let Some(mut stream) = stream else {
            return;
        };
        while let Some(first) = stream.recv().await {
            let mut batch = vec![first];
            let deadline = tokio::time::sleep(ENGINE_BATCH);
            tokio::pin!(deadline);
            let closed = loop {
                tokio::select! {
                    () = &mut deadline => break false,
                    next = stream.recv() => match next {
                        Some(event) => batch.push(event),
                        None => break true,
                    },
                }
            };
            if output.send(Message::Engine(batch)).await.is_err() || closed {
                return;
            }
        }
    })
}

fn tray_events() -> impl Stream<Item = Message> {
    iced::stream::channel(8, async |mut output| {
        let mut events = crate::tray::subscribe();
        while let Some(event) = events.recv().await {
            let message = match event {
                TrayEvent::Show => Message::TrayShow,
                TrayEvent::PauseAll => Message::PauseAll,
                TrayEvent::ResumeAll => Message::ResumeAll,
                TrayEvent::Quit => Message::Quit,
            };
            if output.send(message).await.is_err() {
                return;
            }
        }
    })
}

fn instance_events() -> impl Stream<Item = Message> {
    iced::stream::channel(4, async |mut output| {
        let Some(mut activations) = crate::instance::take_activation_receiver() else {
            return;
        };
        while let Some(activation) = activations.recv().await {
            if output
                .send(Message::InstanceActivated(activation))
                .await
                .is_err()
            {
                return;
            }
        }
    })
}

// Polled rather than watched: Noctalia rewrites its theme in place, and only a changed stamp
// wakes the UI.
fn theme_changes() -> impl Stream<Item = Message> {
    iced::stream::channel(1, async |mut output| {
        let dir = AppConfig::theme_dir();
        let mut seen = None;
        loop {
            let (probe, previous) = (dir.clone(), seen.take());
            let polled = tokio::task::spawn_blocking(move || {
                let stamp = crate::theme::dir_stamp(&probe);
                let changed = previous.is_some_and(|previous| previous != stamp);
                let catalog = changed.then(|| crate::theme::ThemeCatalog::load(&probe));
                (stamp, catalog)
            });
            let Ok((stamp, catalog)) = polled.await else {
                return;
            };
            if let Some(catalog) = catalog
                && output.send(Message::ThemesChanged(catalog)).await.is_err()
            {
                return;
            }
            seen = Some(stamp);
            tokio::time::sleep(THEME_POLL).await;
        }
    })
}
