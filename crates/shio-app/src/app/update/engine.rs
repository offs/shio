use super::super::notification::notify;
use super::super::state::{Overlay, Shio, ToastKind};
use crate::message::{Message, NoticeId};
use iced::Task;
use shio_core::{
    ArchivePackage, DownloadId, DownloadKind, DownloadStatus, PackageExtractState, ProgressEvent,
};

struct Transition {
    id: DownloadId,
    name: String,
    to: DownloadStatus,
    from: DownloadStatus,
    reason: Option<String>,
}

impl Shio {
    pub(super) fn apply_engine_events(&mut self, events: Vec<ProgressEvent>) -> Task<Message> {
        let mut transitions = Vec::new();
        for event in events {
            self.apply_engine_event(event, &mut transitions);
        }
        self.refresh();
        Task::batch(
            transitions
                .into_iter()
                .map(|transition| self.announce(transition))
                .collect::<Vec<_>>(),
        )
    }

    fn apply_engine_event(&mut self, event: ProgressEvent, transitions: &mut Vec<Transition>) {
        match event {
            ProgressEvent::Update(progress) => {
                let Some(download) = self.downloads.iter_mut().find(|d| d.id == progress.id) else {
                    return;
                };
                let from = download.status;
                download.status = progress.status;
                download.downloaded = progress.downloaded;
                download.total_size = progress.total_size;
                download.speed = progress.speed;
                if let (Some(stats), DownloadKind::Torrent(torrent)) =
                    (progress.torrent, &mut download.kind)
                {
                    torrent.stats = stats;
                }
                self.note_download(progress.id, from, transitions);
            },
            ProgressEvent::Changed {
                downloads,
                packages,
            } => {
                for download in downloads {
                    let id = download.id;
                    match self.downloads.iter_mut().find(|d| d.id == id) {
                        Some(existing) => {
                            let from = std::mem::replace(existing, download).status;
                            self.note_download(id, from, transitions);
                        },
                        None => self.downloads.push(download),
                    }
                }
                for package in packages {
                    match self.packages.iter_mut().find(|p| p.id == package.id) {
                        Some(existing) => {
                            let from = std::mem::replace(existing, package);
                            self.note_package(&from, transitions);
                        },
                        None => self.packages.push(package),
                    }
                }
            },
            ProgressEvent::Removed {
                downloads,
                packages,
            } => {
                self.downloads.retain(|d| !downloads.contains(&d.id));
                self.packages.retain(|p| !packages.contains(&p.id));
            },
            ProgressEvent::Resync {
                downloads,
                packages,
            } => {
                let previous_downloads = std::mem::replace(&mut self.downloads, downloads);
                let previous_packages = std::mem::replace(&mut self.packages, packages);
                for previous in &previous_downloads {
                    self.note_download(previous.id, previous.status, transitions);
                }
                for previous in &previous_packages {
                    self.note_package(previous, transitions);
                }
            },
        }
    }

    // Archive-set parts are announced by their package.
    fn note_download(
        &self,
        id: DownloadId,
        from: DownloadStatus,
        transitions: &mut Vec<Transition>,
    ) {
        let Some(download) = self.downloads.iter().find(|d| d.id == id) else {
            return;
        };
        if download.status == from
            || self
                .packages
                .iter()
                .any(|package| package.child_ids().any(|part| part == id))
        {
            return;
        }
        transitions.push(Transition {
            id,
            name: download.filename.clone(),
            to: download.status,
            from,
            reason: download.error_message.clone(),
        });
    }

    fn note_package(&self, previous: &ArchivePackage, transitions: &mut Vec<Transition>) {
        let Some(package) = self.packages.iter().find(|p| p.id == previous.id) else {
            return;
        };
        if package.extract_state == previous.extract_state {
            return;
        }
        let (from, to) = (extract_status(previous), extract_status(package));
        if let Some(to) = to {
            transitions.push(Transition {
                id: package.id.row_id(),
                name: package.name.clone(),
                to,
                from: from.unwrap_or(DownloadStatus::Downloading),
                reason: package.error_message.clone(),
            });
        }
    }

    fn announce(&mut self, transition: Transition) -> Task<Message> {
        let Transition {
            id,
            name,
            to,
            from,
            reason,
        } = transition;
        let mut task = Task::none();
        let (title, kind) = match to {
            DownloadStatus::Completed => match from {
                DownloadStatus::Extracting => ("extracted", ToastKind::Success),
                DownloadStatus::Seeding => return task,
                _ => ("done", ToastKind::Success),
            },
            DownloadStatus::Seeding
                if matches!(
                    from,
                    DownloadStatus::Downloading | DownloadStatus::FetchingMetadata
                ) =>
            {
                ("done", ToastKind::Success)
            },
            DownloadStatus::Error => ("failed", ToastKind::Error),
            DownloadStatus::ExtractError => ("extraction failed", ToastKind::Error),
            DownloadStatus::PasswordRequired => {
                if matches!(self.overlay, Overlay::None) {
                    task = self.open_password(id);
                }
                ("password needed", ToastKind::Info)
            },
            _ => return task,
        };
        let detail = match reason {
            Some(reason) if kind == ToastKind::Error => format!("{name} — {reason}"),
            _ => name,
        };
        if self.config.notifications {
            notify(title, &detail);
        }
        self.push_toast(kind, format!("{title} · {detail}"));
        task
    }

    pub(super) fn engine_stopped(&mut self, result: Result<(), String>) -> Task<Message> {
        if self.shutting_down {
            return Task::none();
        }
        let message = match result {
            Ok(()) => "it exited unexpectedly; restart shio to continue".to_owned(),
            Err(reason) => format!("it crashed ({reason}); restart shio to continue"),
        };
        tracing::error!("download engine stopped: {message}");
        self.set_notice(NoticeId::EngineStopped, "download engine stopped", message);
        Task::none()
    }
}

const fn extract_status(package: &ArchivePackage) -> Option<DownloadStatus> {
    match package.extract_state {
        PackageExtractState::NotStarted => None,
        PackageExtractState::Extracting => Some(DownloadStatus::Extracting),
        PackageExtractState::Completed => Some(DownloadStatus::Completed),
        PackageExtractState::Error => Some(DownloadStatus::ExtractError),
        PackageExtractState::PasswordRequired => Some(DownloadStatus::PasswordRequired),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tests::{download, id};
    use shio_core::{Download, PackageId, PackageItem};
    use tokio::sync::mpsc;

    fn shio() -> Shio {
        let (engine, _) = mpsc::channel(1);
        let config = shio_core::AppConfig {
            notifications: false,
            ..Default::default()
        };
        Shio::build(config, engine)
    }

    fn package(parts: &[usize]) -> ArchivePackage {
        ArchivePackage {
            id: PackageId(id(100).0),
            name: "set".to_owned(),
            save_path: "/downloads".into(),
            auto_extract: true,
            extract_state: PackageExtractState::NotStarted,
            error_message: None,
            created_at: std::time::SystemTime::UNIX_EPOCH.into(),
            completed_at: None,
            pinned: false,
            items: parts
                .iter()
                .zip(1..)
                .map(|(&part, number)| PackageItem {
                    download_id: id(part),
                    position: number - 1,
                    part_number: number,
                })
                .collect(),
        }
    }

    fn resync(downloads: Vec<Download>, packages: Vec<ArchivePackage>) -> ProgressEvent {
        ProgressEvent::Resync {
            downloads,
            packages,
        }
    }

    #[test]
    fn events_apply_the_engine_state_verbatim() {
        let mut app = shio();
        let _ = app.apply_engine_events(vec![
            resync(vec![download(1, DownloadStatus::Downloading)], Vec::new()),
            ProgressEvent::Changed {
                downloads: vec![
                    download(1, DownloadStatus::Paused),
                    download(2, DownloadStatus::Queued),
                ],
                packages: Vec::new(),
            },
            ProgressEvent::Update(shio_core::DownloadProgress {
                id: id(2),
                status: DownloadStatus::Downloading,
                downloaded: 40,
                total_size: Some(100),
                speed: 10,
                torrent: None,
            }),
        ]);

        let statuses = [1, 2].map(|n| app.row(id(n)).map(|row| (row.status, row.downloaded)));
        assert_eq!(
            statuses,
            [
                Some((DownloadStatus::Paused, 0)),
                Some((DownloadStatus::Downloading, 40))
            ]
        );
        assert_eq!(app.totals.down, 10);

        let _ = app.apply_engine_events(vec![ProgressEvent::Removed {
            downloads: vec![id(1)],
            packages: Vec::new(),
        }]);
        assert!(app.row(id(1)).is_none());
        assert!(app.row(id(2)).is_some());
    }

    #[test]
    fn archive_parts_fold_into_one_package_row_and_leave_with_it() {
        let mut app = shio();
        let mut part = download(1, DownloadStatus::Downloading);
        part.speed = 7;
        let _ = app.apply_engine_events(vec![resync(
            vec![
                part,
                download(2, DownloadStatus::Queued),
                download(3, DownloadStatus::Completed),
            ],
            vec![package(&[1, 2])],
        )]);

        let package_row = app
            .row(PackageId(id(100).0).row_id())
            .copied()
            .expect("package row");
        assert_eq!(app.rows.len(), 2);
        assert_eq!(package_row.status, DownloadStatus::Downloading);
        assert_eq!((package_row.total, package_row.speed), (Some(200), 7));
        assert_eq!(
            (app.counts.all, app.counts.active, app.counts.done),
            (2, 1, 1)
        );
        assert_eq!(app.totals.down, 7);

        let _ = app.apply_engine_events(vec![ProgressEvent::Removed {
            downloads: vec![id(1), id(2)],
            packages: vec![PackageId(id(100).0)],
        }]);
        assert_eq!(app.rows.len(), 1);
        assert_eq!(app.packages, Vec::<ArchivePackage>::new());
    }

    #[test]
    fn package_rows_follow_part_progress_and_extraction() {
        let mut app = shio();
        let _ = app.apply_engine_events(vec![resync(
            vec![
                download(1, DownloadStatus::Queued),
                download(2, DownloadStatus::Queued),
            ],
            vec![package(&[1, 2])],
        )]);
        let row_id = PackageId(id(100).0).row_id();

        let _ = app.apply_engine_events(vec![ProgressEvent::Update(shio_core::DownloadProgress {
            id: id(2),
            status: DownloadStatus::Downloading,
            downloaded: 50,
            total_size: Some(100),
            speed: 5,
            torrent: None,
        })]);
        let row = app.row(row_id).copied().expect("package row");
        assert_eq!(
            (row.status, row.downloaded, row.speed),
            (DownloadStatus::Downloading, 50, 5)
        );

        let mut extracting = package(&[1, 2]);
        extracting.extract_state = PackageExtractState::PasswordRequired;
        let _ = app.apply_engine_events(vec![ProgressEvent::Changed {
            downloads: vec![
                download(1, DownloadStatus::Completed),
                download(2, DownloadStatus::Completed),
            ],
            packages: vec![extracting],
        }]);
        assert_eq!(
            app.row(row_id).map(|row| row.status),
            Some(DownloadStatus::PasswordRequired)
        );
        assert!(matches!(app.overlay, Overlay::Password { id, .. } if id == row_id));
    }

    #[test]
    fn resync_replaces_rows_and_drops_selection_of_vanished_ones() {
        let mut app = shio();
        let _ = app.apply_engine_events(vec![resync(
            vec![
                download(1, DownloadStatus::Paused),
                download(2, DownloadStatus::Paused),
            ],
            Vec::new(),
        )]);
        app.selection.extend([id(1), id(2)]);

        let _ = app.apply_engine_events(vec![resync(
            vec![download(2, DownloadStatus::Error)],
            Vec::new(),
        )]);

        assert_eq!(app.rows.len(), 1);
        assert_eq!(app.selection.iter().copied().collect::<Vec<_>>(), [id(2)]);
        assert_eq!(app.counts.failed, 1);
        assert!(app.toast.is_some());
    }
}
