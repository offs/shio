use std::sync::OnceLock;
use std::sync::mpsc::{SyncSender, sync_channel};

const NOTIFICATION_QUEUE_CAPACITY: usize = 32;

struct NotificationMessage {
    title: String,
    body: String,
}

pub(crate) fn send_notification(title: &str, body: &str) {
    let message = NotificationMessage {
        title: title.to_string(),
        body: body.to_string(),
    };
    if let Err(error) = notification_tx().try_send(message) {
        tracing::warn!("notification queue full: {error}");
    }
}

fn notification_tx() -> &'static SyncSender<NotificationMessage> {
    static TX: OnceLock<SyncSender<NotificationMessage>> = OnceLock::new();
    TX.get_or_init(|| {
        let (tx, rx) = sync_channel::<NotificationMessage>(NOTIFICATION_QUEUE_CAPACITY);
        std::thread::spawn(move || {
            while let Ok(message) = rx.recv() {
                show_notification(&message.title, &message.body);
            }
        });
        tx
    })
}

fn show_notification(title: &str, body: &str) {
    let mut n = notify_rust::Notification::new();
    n.summary(title)
        .body(body)
        .appname(crate::platform::app_id());
    #[cfg(target_os = "windows")]
    n.app_id(crate::platform::app_id());
    if let Err(error) = n.show() {
        tracing::warn!("notification failed: {error}");
    }
}
