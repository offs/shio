use super::super::notification::send_notification;
use super::super::state::{Shio, ToastKind};
use crate::message::Message;
use iced::Task;
use shio_core::{DownloadId, DownloadProgress, DownloadStatus};
use std::time::Instant;

struct Toast {
    title: &'static str,
    detail: String,
    kind: ToastKind,
    notify: bool,
}

enum Event {
    Toast(Toast),
    PasswordNeeded(DownloadId),
}

fn transition_event(
    prev: DownloadStatus,
    next: DownloadStatus,
    name: &str,
    reason: &str,
    id: DownloadId,
) -> Option<Event> {
    let detail = if reason.is_empty() {
        name.to_string()
    } else {
        format!("{name} — {reason}")
    };
    let toast = |title, kind, notify| {
        Event::Toast(Toast {
            title,
            detail: detail.clone(),
            kind,
            notify,
        })
    };
    match (prev, next) {
        (_, DownloadStatus::Extracting) => Some(toast("extracting", ToastKind::Info, false)),
        (_, DownloadStatus::Seeding) => Some(toast("seeding", ToastKind::Info, false)),
        (DownloadStatus::Extracting, DownloadStatus::Completed) => {
            Some(toast("extracted", ToastKind::Success, true))
        },
        (_, DownloadStatus::Completed) => Some(toast("completed", ToastKind::Success, true)),
        (_, DownloadStatus::PasswordRequired) => Some(Event::PasswordNeeded(id)),
        (_, DownloadStatus::ExtractError) => Some(toast("extract failed", ToastKind::Error, true)),
        (_, DownloadStatus::Error) => Some(toast("failed", ToastKind::Error, true)),
        _ => None,
    }
}

const fn should_apply_progress_status(current: DownloadStatus, next: DownloadStatus) -> bool {
    if matches!(
        current,
        DownloadStatus::Paused | DownloadStatus::Cancelled | DownloadStatus::Completed
    ) {
        return false;
    }

    if matches!(current, DownloadStatus::Pending | DownloadStatus::Queued)
        && matches!(next, DownloadStatus::Paused | DownloadStatus::Cancelled)
    {
        return false;
    }

    true
}

impl Shio {
    pub(super) fn progress_tick(&mut self, progress_list: Vec<DownloadProgress>) -> Task<Message> {
        let _span = tracing::trace_span!("progress_tick", updates = progress_list.len()).entered();
        self.now = Instant::now();
        let mut events: Vec<Event> = Vec::new();

        for p in progress_list {
            if let Some(package) = self
                .packages
                .iter_mut()
                .find(|package| shio_core::DownloadId(package.id.0) == p.id)
            {
                package.extract_state = match p.status {
                    DownloadStatus::Extracting => shio_core::PackageExtractState::Extracting,
                    DownloadStatus::Completed => shio_core::PackageExtractState::Completed,
                    DownloadStatus::PasswordRequired => {
                        shio_core::PackageExtractState::PasswordRequired
                    },
                    DownloadStatus::ExtractError => shio_core::PackageExtractState::Error,
                    _ => package.extract_state,
                };
                continue;
            }
            let Some(dl) = self.downloads.iter_mut().find(|d| d.id == p.id) else {
                continue;
            };
            let prev_status = dl.status;
            if let Some(filename) = p.filename.as_ref() {
                dl.filename.clone_from(filename);
            }
            if let Some(snapshot) = p.torrent_snapshot.as_ref() {
                if let Some(torrent) = dl.torrent_mut() {
                    torrent.is_private = snapshot.is_private;
                    torrent.files.clone_from(&snapshot.files);
                    torrent.trackers.clone_from(&snapshot.trackers);
                }
            }
            dl.downloaded = p.downloaded;
            dl.total_size = p.total_size.or(dl.total_size);
            dl.speed = p.speed;
            dl.avg_speed = p.avg_speed;
            if should_apply_progress_status(dl.status, p.status) {
                dl.status = p.status;
            }
            if let shio_core::ProgressDetail::Torrent {
                peers_connected,
                seeders,
                leechers,
                uploaded,
                upload_speed,
                ratio,
                seed_elapsed_secs,
                metadata_wait_secs,
            } = &p.detail
            {
                if let Some(torrent) = dl.torrent_mut() {
                    torrent.peers_connected = *peers_connected;
                    torrent.seeders = *seeders;
                    torrent.leechers = *leechers;
                    torrent.uploaded = *uploaded;
                    torrent.upload_speed = *upload_speed;
                    torrent.ratio = *ratio;
                    torrent.seed_elapsed_secs = *seed_elapsed_secs;
                    torrent.metadata_wait_secs = *metadata_wait_secs;
                }
            }

            let id = dl.id;
            let status = dl.status;
            let filename = dl.filename.clone();
            let reason = dl.error_message.clone().unwrap_or_default();

            if prev_status != status {
                if let Some(event) = transition_event(prev_status, status, &filename, &reason, id) {
                    events.push(event);
                }
            }
        }

        super::super::state::sync_package_rows(&mut self.downloads, &self.packages);

        let global_notify = self.config.notifications;
        for event in events {
            match event {
                Event::Toast(Toast {
                    title,
                    detail,
                    kind,
                    notify,
                }) => {
                    self.push_toast(&format!("{title}: {detail}"), kind);
                    if notify && global_notify {
                        send_notification(title, &detail);
                    }
                },
                Event::PasswordNeeded(id) => {
                    if self.password_prompt().is_none() {
                        let _ = self.update(Message::RequestPassword(id));
                    }
                },
            }
        }

        Task::none()
    }

    pub(super) fn download_added(&mut self) -> Task<Message> {
        self.push_toast("download added to queue", ToastKind::Info);
        Task::none()
    }

    pub(super) fn frame_tick(&mut self, now: Instant) -> Task<Message> {
        let _span = tracing::trace_span!("frame_tick", toasts = self.toasts.len()).entered();
        self.now = now;
        self.tick_toasts();
        Task::none()
    }
}
