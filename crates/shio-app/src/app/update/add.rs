use super::super::add_model::{
    AddDialog, AddRequest, AddValidation, MAX_URL_TEXT_BYTES, Previews, TorrentPick, file_label,
};
use super::super::state::{ClipboardOffer, Overlay, Shio, ToastKind};
use super::ENGINE_STOPPED;
use crate::link::{is_link, is_magnet};
use crate::message::Message;
use crate::platform::AssociationLaunch;
use iced::Task;
use iced::widget::{operation, text_editor};
use shio_core::{
    EngineCommand, HttpPreviewResult, HttpPreviewState, MAX_TORRENT_METADATA_BYTES,
    MagnetPreviewResult,
};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use tokio::sync::{mpsc, oneshot};

const MAX_PENDING_TORRENTS: usize = 32;
const MAX_SHORTCUT_BYTES: u64 = 64 * 1024;

impl Shio {
    pub(super) fn open_sources(&mut self, launch: AssociationLaunch) -> Task<Message> {
        if launch.magnets.is_empty() && launch.torrent_files.is_empty() {
            return Task::none();
        }
        self.queue_sources(&launch.magnets.join("\n"), launch.torrent_files)
    }

    pub(super) fn open_add(&mut self, text: &str) -> Task<Message> {
        self.queue_sources(text, Vec::new())
    }

    pub(super) fn queue_sources(&mut self, links: &str, torrents: Vec<PathBuf>) -> Task<Message> {
        let pending = &mut self.pending;
        if !links.trim().is_empty() && pending.links.len() + links.len() < MAX_URL_TEXT_BYTES {
            pending.links.push_str(links);
            pending.links.push('\n');
        }
        let room = MAX_PENDING_TORRENTS.saturating_sub(pending.torrents.len());
        pending.torrents.extend(torrents.into_iter().take(room));
        if matches!(self.overlay, Overlay::None | Overlay::Add(_)) {
            self.show_pending()
        } else {
            Task::none()
        }
    }

    fn show_pending(&mut self) -> Task<Message> {
        if self.startup_problem.is_some() {
            return Task::none();
        }
        let focus = if matches!(self.overlay, Overlay::None) {
            self.menu = None;
            self.overlay = Overlay::Add(Box::new(AddDialog::new(&self.config)));
            operation::focus(crate::views::ADD_URLS_ID)
        } else {
            Task::none()
        };
        let Overlay::Add(dialog) = &mut self.overlay else {
            return Task::none();
        };
        let links = std::mem::take(&mut self.pending.links);
        let torrents = std::mem::take(&mut self.pending.torrents);
        let previews = if links.is_empty() {
            Previews::default()
        } else {
            dialog.append_text(links.trim_end());
            dialog.sync_links(&mut self.next_request)
        };
        Task::batch([
            focus,
            self.preview_tasks(previews),
            read_torrent_files(torrents),
        ])
    }

    pub(super) fn edit_add(&mut self, edit: impl FnOnce(&mut AddDialog)) -> Task<Message> {
        if let Overlay::Add(dialog) = &mut self.overlay {
            edit(dialog);
        }
        Task::none()
    }

    pub(super) fn add_urls_action(&mut self, action: text_editor::Action) -> Task<Message> {
        let Overlay::Add(dialog) = &mut self.overlay else {
            return Task::none();
        };
        let edit = action.is_edit();
        dialog.urls.perform(action);
        if !edit {
            return Task::none();
        }
        let previews = dialog.sync_links(&mut self.next_request);
        self.preview_tasks(previews)
    }

    pub(super) fn http_previewed(&mut self, result: HttpPreviewResult) -> Task<Message> {
        let Overlay::Add(dialog) = &mut self.overlay else {
            return Task::none();
        };
        let previews = dialog.apply_http_preview(result, &mut self.next_request);
        self.preview_tasks(previews)
    }

    pub(super) fn torrent_files_read(
        &mut self,
        picks: Vec<Result<TorrentPick, String>>,
    ) -> Task<Message> {
        let Overlay::Add(dialog) = &mut self.overlay else {
            return Task::none();
        };
        for problem in dialog.add_torrents(picks, &mut self.next_request) {
            self.push_toast(ToastKind::Error, problem);
        }
        Task::none()
    }

    pub(super) fn add_folder_picked(&mut self, path: &Path) -> Task<Message> {
        match super::super::path_text(path) {
            Ok(text) => self.edit_add(|dialog| {
                dialog.save_path =
                    super::super::truncate_text_bytes(text, super::super::MAX_PATH_TEXT_BYTES);
                dialog.revalidate();
            }),
            Err(problem) => {
                self.push_toast(ToastKind::Error, problem);
                Task::none()
            },
        }
    }

    pub(super) fn add_submit(&mut self) -> Task<Message> {
        let Overlay::Add(dialog) = &mut self.overlay else {
            return Task::none();
        };
        if dialog.submitting || !matches!(dialog.validation, AddValidation::Ready(_)) {
            return Task::none();
        }
        let requests = dialog.submit();
        let engine = self.engine.clone();
        Task::perform(
            async move {
                let mut results = Vec::with_capacity(requests.len());
                for request in requests {
                    let (reply, outcome) = oneshot::channel();
                    let command = match request {
                        AddRequest::Http(request) => EngineCommand::AddHttp { request, reply },
                        AddRequest::ArchiveSet(request) => {
                            EngineCommand::AddHttpArchiveSet { request, reply }
                        },
                        AddRequest::Torrent(request) => {
                            EngineCommand::AddTorrentPrepared { request, reply }
                        },
                    };
                    let result = async {
                        engine
                            .send(command)
                            .await
                            .map_err(|_| ENGINE_STOPPED.to_owned())?;
                        outcome
                            .await
                            .map_err(|_| ENGINE_STOPPED.to_owned())?
                            .map_err(|error| error.short_label())
                    };
                    results.push(result.await);
                }
                results
            },
            Message::AddSubmitted,
        )
    }

    pub(super) fn add_submitted(&mut self, results: Vec<Result<(), String>>) -> Task<Message> {
        let Overlay::Add(dialog) = &mut self.overlay else {
            let failed = results.iter().filter(|result| result.is_err()).count();
            if failed > 0 {
                self.push_toast(ToastKind::Error, format!("{failed} could not be added"));
            }
            return Task::none();
        };
        let (added, previews) = dialog.finish_submit(results, &mut self.next_request);
        let finished =
            dialog.failures.is_empty() && dialog.links.is_empty() && dialog.torrents.is_empty();
        let task = self.preview_tasks(previews);
        if added > 0 {
            self.push_toast(
                ToastKind::Success,
                if added == 1 {
                    "added 1 download".to_owned()
                } else {
                    format!("added {added} downloads")
                },
            );
        }
        if finished {
            return Task::batch([task, self.close_overlay()]);
        }
        task
    }

    // Returns whether `text` is new clipboard content; the first read only sets the baseline.
    pub(super) fn note_clipboard(&mut self, text: Option<&str>) -> bool {
        let mut hasher = DefaultHasher::new();
        text.hash(&mut hasher);
        let fingerprint = hasher.finish();
        self.clipboard_seen
            .replace(fingerprint)
            .is_some_and(|seen| seen != fingerprint)
    }

    pub(super) fn clipboard_read(&mut self, text: Option<String>) {
        if !self.note_clipboard(text.as_deref()) {
            return;
        }
        let Some(text) = text.filter(|text| text.len() <= MAX_URL_TEXT_BYTES) else {
            return;
        };
        let links: Vec<&str> = text
            .lines()
            .map(str::trim)
            .filter(|line| is_link(line))
            .collect();
        let name = match links.as_slice() {
            [] => return,
            [link] if is_magnet(link) => "magnet link".to_owned(),
            [link] => shio_core::extract_filename(link),
            links => format!("{} links", links.len()),
        };
        self.clipboard_offer = Some(ClipboardOffer {
            links: links.join("\n"),
            name,
        });
    }

    pub(super) fn file_dropped(&mut self, path: PathBuf) -> Task<Message> {
        let Some(extension) = path.extension().and_then(|extension| extension.to_str()) else {
            return Task::none();
        };
        if extension.eq_ignore_ascii_case("torrent") {
            return self.queue_sources("", vec![path]);
        }
        if extension.eq_ignore_ascii_case("url") || extension.eq_ignore_ascii_case("desktop") {
            return Task::perform(read_shortcut(path), Message::ShortcutRead);
        }
        Task::none()
    }

    pub(super) fn close_overlay(&mut self) -> Task<Message> {
        let commit = self.commit_settings();
        let task =
            match std::mem::replace(&mut self.overlay, Overlay::None) {
                Overlay::Add(dialog) => Task::batch(dialog.resolving().map(|request_id| {
                    self.send_detached(EngineCommand::CancelPreview { request_id })
                })),
                Overlay::Welcome(_) => self.save_config(),
                Overlay::None
                | Overlay::Settings(_)
                | Overlay::ConfirmRemove(_)
                | Overlay::ConfirmCancel(_)
                | Overlay::Password { .. } => Task::none(),
            };
        let pending = if self.pending.links.is_empty() && self.pending.torrents.is_empty() {
            Task::none()
        } else {
            self.show_pending()
        };
        Task::batch([commit, task, pending])
    }

    fn preview_tasks(&self, previews: Previews) -> Task<Message> {
        let mut tasks = Vec::new();
        for request_id in previews.cancel {
            tasks.push(self.send_detached(EngineCommand::CancelPreview { request_id }));
        }
        for (request_id, url) in previews.http {
            let engine = self.engine.clone();
            tasks.push(Task::perform(
                async move {
                    let command = |reply| EngineCommand::ResolveHttpPreview {
                        request_id,
                        url,
                        reply,
                    };
                    ask(&engine, command)
                        .await
                        .unwrap_or_else(|| HttpPreviewResult {
                            request_id,
                            state: HttpPreviewState::Error {
                                message: ENGINE_STOPPED.to_owned(),
                            },
                        })
                },
                Message::HttpPreviewed,
            ));
        }
        for (request_id, magnet) in previews.magnets {
            let engine = self.engine.clone();
            tasks.push(Task::perform(
                async move {
                    let command = |reply| EngineCommand::ResolveMagnetPreview {
                        request_id,
                        magnet: magnet.clone(),
                        reply,
                    };
                    ask(&engine, command)
                        .await
                        .unwrap_or_else(|| MagnetPreviewResult {
                            request_id,
                            magnet,
                            result: Err(ENGINE_STOPPED.to_owned()),
                        })
                },
                Message::MagnetPreviewed,
            ));
        }
        Task::batch(tasks)
    }

    fn send_detached(&self, command: EngineCommand) -> Task<Message> {
        let engine = self.engine.clone();
        Task::future(async move {
            let _ = engine.send(command).await;
        })
        .discard()
    }
}

async fn ask<R>(
    engine: &mpsc::Sender<EngineCommand>,
    command: impl FnOnce(mpsc::Sender<R>) -> EngineCommand,
) -> Option<R> {
    let (reply, mut result) = mpsc::channel(1);
    engine.send(command(reply)).await.ok()?;
    result.recv().await
}

fn read_torrent_files(paths: Vec<PathBuf>) -> Task<Message> {
    if paths.is_empty() {
        return Task::none();
    }
    Task::perform(
        async move {
            let mut picks = Vec::with_capacity(paths.len());
            for path in paths {
                let pick =
                    super::super::read_bounded_file(&path, MAX_TORRENT_METADATA_BYTES as u64)
                        .await
                        .map_err(|error| format!("{}: {error}", file_label(&path)))
                        .and_then(|bytes| TorrentPick::from_file(path, bytes));
                picks.push(pick);
            }
            picks
        },
        Message::TorrentFilesRead,
    )
}

pub(super) fn pick_torrent_files() -> Task<Message> {
    Task::future(
        rfd::AsyncFileDialog::new()
            .add_filter("torrent", &["torrent"])
            .set_title("choose torrent files")
            .pick_files(),
    )
    .and_then(|files| {
        Task::done(Message::TorrentFilesPicked(
            files.iter().map(|file| file.path().to_path_buf()).collect(),
        ))
    })
}

pub(super) fn pick_folder(picked: fn(PathBuf) -> Message) -> Task<Message> {
    Task::future(
        rfd::AsyncFileDialog::new()
            .set_title("choose a folder")
            .pick_folder(),
    )
    .and_then(move |folder| Task::done(picked(folder.path().to_path_buf())))
}

async fn read_shortcut(path: PathBuf) -> Option<String> {
    let bytes = super::super::read_bounded_file(&path, MAX_SHORTCUT_BYTES)
        .await
        .ok()?;
    shortcut_url(std::str::from_utf8(&bytes).ok()?)
}

fn shortcut_url(content: &str) -> Option<String> {
    content
        .lines()
        .map(str::trim)
        .map(|line| line.strip_prefix("URL=").unwrap_or(line))
        .find(|line| is_link(line))
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcut_files_yield_their_link() {
        let cases = [
            (
                "[InternetShortcut]\nURL=https://example.com/file.zip\n",
                Some("https://example.com/file.zip"),
            ),
            (
                "[Desktop Entry]\nType=Link\nURL=https://example.com/a.iso",
                Some("https://example.com/a.iso"),
            ),
            (
                "Name=download\nhttps://example.com/file.zip\n",
                Some("https://example.com/file.zip"),
            ),
            ("URL=file:///etc/passwd", None),
        ];
        for (content, expected) in cases {
            assert_eq!(shortcut_url(content).as_deref(), expected, "{content}");
        }
    }

    #[test]
    fn clipboard_offers_only_links_copied_while_away_and_never_opens_a_dialog() {
        let (engine, _) = mpsc::channel(1);
        let mut app = Shio::build(shio_core::AppConfig::default(), engine);
        let offered = |app: &Shio| app.clipboard_offer.as_ref().map(|offer| offer.name.clone());

        app.clipboard_read(Some("https://e.com/already-there.iso".to_owned()));
        assert_eq!(offered(&app), None, "the first read is only a baseline");

        app.note_clipboard(Some("https://e.com/copied-in-shio.iso"));
        app.clipboard_read(Some("https://e.com/copied-in-shio.iso".to_owned()));
        assert_eq!(offered(&app), None, "shio's own copies are not offered");

        app.clipboard_read(Some("https://e.com/new.iso".to_owned()));
        assert_eq!(offered(&app).as_deref(), Some("new.iso"));
        assert!(matches!(app.overlay, Overlay::None));

        app.clipboard_offer = None;
        app.clipboard_read(Some("https://e.com/new.iso".to_owned()));
        assert_eq!(offered(&app), None, "the same text is never offered twice");
    }
}
