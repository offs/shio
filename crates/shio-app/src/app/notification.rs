use std::sync::LazyLock;
use std::sync::mpsc::{SyncSender, sync_channel};

// Some backends block, so notifications are shown from a dedicated thread.
static QUEUE: LazyLock<SyncSender<(String, String)>> = LazyLock::new(|| {
    let (queue, notifications) = sync_channel::<(String, String)>(32);
    std::thread::spawn(move || {
        #[cfg(target_os = "macos")]
        if let Err(error) = notify_rust::set_application(crate::platform::APP_ID) {
            tracing::warn!("notification identity: {error}");
        }
        while let Ok((title, body)) = notifications.recv() {
            show(&title, &body);
        }
    });
    queue
});

pub(super) fn notify(title: &str, body: &str) {
    if QUEUE.try_send((title.to_owned(), body.to_owned())).is_err() {
        tracing::warn!("notification queue is full");
    }
}

pub(crate) fn show(title: &str, body: &str) {
    let mut notification = notify_rust::Notification::new();
    notification.summary(title);
    #[cfg(all(unix, not(target_os = "macos")))]
    notification
        .body(&escape_markup(body))
        .appname("Shio")
        .hint(notify_rust::Hint::DesktopEntry(
            crate::platform::APP_ID.into(),
        ));
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    notification.body(body);
    #[cfg(windows)]
    notification.app_id(crate::platform::APP_ID);
    if let Err(error) = notification.show() {
        tracing::warn!("notification failed: {error}");
    }
}

// Linux notification servers parse a markup subset in bodies.
#[cfg(all(unix, not(target_os = "macos")))]
fn escape_markup(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
