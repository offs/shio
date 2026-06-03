use super::super::add_model::{
    ADD_URL_PREVIEW_LIMIT, add_single_http_name, addable_has_archive_url, addable_url_entries,
    apply_http_preview_result, apply_magnet_preview_result, archive_set_validation_error,
    can_confirm_add_sources, drain_http_preview_request_ids, http_preview_name, is_magnet,
    parse_add_urls, path_label, removed_http_preview_request_ids, repair_add_source_selection,
    set_matching_torrent_files, should_request_http_previews, torrent_display_name,
    torrent_file_preview, torrent_subfolder_name,
};
use super::super::state::{
    AddHttpPreview, AddHttpPreviewState, AddMagnetPreview, AddMagnetPreviewState, AddSourceId,
    AddTorrentFile, AddTorrentSource, Overlay, Shio, ToastKind,
};
use crate::message::{
    EditConflictResult, EditRenameResult, EngineAction, Message, QueueDownloadsResult,
};
use iced::Task;
use iced::widget::text_editor;
use shio_core::{
    Download, DownloadId, DownloadStatus, EngineCommand, HttpArchivePartRequest,
    HttpArchiveSetRequest, HttpDownloadRequest, TorrentDownloadRequest,
};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use tokio::sync::oneshot;

struct ArchiveUrlEntry {
    url: String,
    filename: String,
    part: shio_core::ArchivePart,
}

struct QueuedAdd {
    downloads: Vec<Download>,
    packages: Vec<shio_core::ArchivePackage>,
    command: EngineCommand,
    ack: oneshot::Receiver<shio_core::Result<()>>,
}

fn queued_add(
    downloads: Vec<Download>,
    packages: Vec<shio_core::ArchivePackage>,
    build: impl FnOnce(oneshot::Sender<shio_core::Result<()>>) -> EngineCommand,
) -> QueuedAdd {
    let (reply, ack) = oneshot::channel();
    QueuedAdd {
        downloads,
        packages,
        command: build(reply),
        ack,
    }
}

fn archive_url_entry(url: &str, http_previews: &[AddHttpPreview]) -> Option<ArchiveUrlEntry> {
    let filename = http_preview_name(url, http_previews)
        .unwrap_or_else(|| shio_core::extract_filename(url, None));
    let part = shio_core::parse_archive_part(&filename)?;
    Some(ArchiveUrlEntry {
        url: url.to_string(),
        filename,
        part,
    })
}

fn archive_set_commands(
    urls: &[String],
    http_previews: &[AddHttpPreview],
    save_path: &Path,
    package_name_override: Option<&str>,
    segments: u8,
    auto_extract: bool,
) -> (Vec<QueuedAdd>, HashSet<String>, Option<String>) {
    let mut groups: BTreeMap<String, Vec<ArchiveUrlEntry>> = BTreeMap::new();
    for url in urls.iter().filter(|url| !is_magnet(url)) {
        if let Some(entry) = archive_url_entry(url, http_previews) {
            groups
                .entry(entry.part.base_name.clone())
                .or_default()
                .push(entry);
        }
    }

    let mut commands = Vec::new();
    let mut grouped_urls = HashSet::new();
    for (base_name, mut entries) in groups {
        if entries.len() < 2 {
            continue;
        }
        entries.sort_by_key(|entry| entry.part.part_number);
        let mut parts = Vec::with_capacity(entries.len());
        for entry in &entries {
            match HttpArchivePartRequest::new(
                entry.url.clone(),
                entry.filename.clone(),
                entry.part.part_number,
                segments,
            ) {
                Ok(part) => parts.push(part),
                Err(error) => return (commands, grouped_urls, Some(error.short_label())),
            }
        }
        let package_name = package_name_override.map_or_else(|| base_name.clone(), str::to_string);
        let request = match HttpArchiveSetRequest::new(
            package_name,
            save_path.to_path_buf(),
            parts,
            auto_extract,
        ) {
            Ok(request) => request,
            Err(error) => return (commands, grouped_urls, Some(error.short_label())),
        };
        let (package, child_downloads) = match request.clone().into_package_downloads() {
            Ok(value) => value,
            Err(error) => return (commands, grouped_urls, Some(error.short_label())),
        };
        for entry in entries {
            grouped_urls.insert(entry.url);
        }
        commands.push(queued_add(child_downloads, vec![package], move |reply| {
            EngineCommand::AddHttpArchiveSet { request, reply }
        }));
    }
    (commands, grouped_urls, None)
}

impl Shio {
    fn refresh_add_subfolder_suggestion(&mut self) {
        if self.add_subfolder_name_dirty {
            return;
        }
        let addable_urls = addable_url_entries(&self.add_url_entries, &self.add_http_previews);
        let mut names = addable_urls
            .iter()
            .filter(|url| !is_magnet(url))
            .map(|url| {
                http_preview_name(url, &self.add_http_previews)
                    .unwrap_or_else(|| shio_core::extract_filename(url, None))
            })
            .collect::<Vec<_>>();
        names.extend(
            self.add_torrent_files
                .iter()
                .filter_map(torrent_subfolder_name),
        );
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        self.add_subfolder_name = shio_core::suggest_folder_name(&refs).unwrap_or_else(|| {
            names
                .first()
                .map_or_else(String::new, |name| shio_core::sanitize_filename(name))
        });
    }

    pub(crate) fn addable_add_url_count(&self) -> usize {
        addable_url_entries(&self.add_url_entries, &self.add_http_previews).len()
    }

    pub(super) fn add_dialog_open(&mut self) -> Task<Message> {
        self.reset_add_dialog_state()
    }

    pub(super) fn add_dialog_cancel(&mut self) -> Task<Message> {
        let tasks = self.cancel_all_http_previews();
        self.overlay = Overlay::None;
        Task::batch(tasks)
    }

    pub(super) fn can_confirm_add_dialog(&self) -> bool {
        can_confirm_add_sources(
            &addable_url_entries(&self.add_url_entries, &self.add_http_previews),
            &self.add_torrent_files,
        ) && archive_set_validation_error(&self.add_url_entries, &self.add_http_previews).is_none()
    }

    pub(super) fn add_dialog_confirm(&mut self) -> Task<Message> {
        if !self.can_confirm_add_dialog() {
            return Task::none();
        }

        let urls = addable_url_entries(&self.add_url_entries, &self.add_http_previews);
        let torrent_files = self.add_torrent_files.clone();

        let subfolder_opt =
            shio_core::subfolder_value(self.add_create_subfolder, &self.add_subfolder_name);
        let auto_extract = self.add_auto_extract;

        let save_path = PathBuf::from(&self.add_save_path);
        let segments = self.config.default_segments;
        let http_count = urls.iter().filter(|url| !is_magnet(url)).count();
        let filename_override = if http_count == 1 && torrent_files.is_empty() {
            if self.add_filename.is_empty() {
                urls.iter()
                    .find(|url| !is_magnet(url))
                    .and_then(|url| http_preview_name(url, &self.add_http_previews))
            } else {
                Some(self.add_filename.clone())
            }
        } else {
            None
        };

        let (mut downloads, grouped_urls, archive_error) = archive_set_commands(
            &urls,
            &self.add_http_previews,
            &save_path,
            subfolder_opt.as_deref(),
            segments,
            auto_extract,
        );
        if let Some(error) = archive_error {
            self.push_toast(&error, ToastKind::Error);
            return Task::none();
        }
        for url in urls {
            if grouped_urls.contains(&url) {
                continue;
            }
            let queued = if is_magnet(&url) {
                let mut request = TorrentDownloadRequest::new(
                    shio_core::TorrentSource::Magnet(url),
                    save_path.clone(),
                );
                if auto_extract {
                    request = request.enable_auto_extract();
                }
                let download = match request.clone().into_download() {
                    Ok(download) => download,
                    Err(error) => {
                        self.push_toast(&format!("invalid magnet link: {error}"), ToastKind::Error);
                        continue;
                    },
                };
                queued_add(vec![download], Vec::new(), move |reply| {
                    EngineCommand::AddTorrentPrepared { request, reply }
                })
            } else {
                let mut request = HttpDownloadRequest::new(url, save_path.clone())
                    .with_segments(segments)
                    .with_subfolder(subfolder_opt.clone());
                if let Some(ref name) = filename_override {
                    request = request.with_filename(name.clone());
                }
                if auto_extract {
                    request = request.enable_auto_extract();
                }
                let download = request.clone().into_download();
                queued_add(vec![download], Vec::new(), move |reply| {
                    EngineCommand::AddHttp { request, reply }
                })
            };
            downloads.push(queued);
        }

        for path in torrent_files {
            if !path.has_selection() {
                self.push_toast(
                    &format!("select at least one file in {}", path.display_name),
                    ToastKind::Error,
                );
                continue;
            }

            let source = match &path.source {
                AddTorrentSource::File { path: file_path } => {
                    let Some(bytes) = path.metadata_bytes.clone() else {
                        self.push_toast(
                            &format!("missing torrent data for {}", path_label(file_path)),
                            ToastKind::Error,
                        );
                        continue;
                    };
                    shio_core::TorrentSource::File(bytes)
                },
                AddTorrentSource::Magnet { magnet, .. } => {
                    shio_core::TorrentSource::Magnet(magnet.clone())
                },
            };

            let mut request = TorrentDownloadRequest::new(
                source,
                subfolder_opt
                    .as_ref()
                    .map_or_else(|| save_path.clone(), |subfolder| save_path.join(subfolder)),
            )
            .with_filename(torrent_display_name(&path))
            .with_total_size(path.selected_size())
            .with_private(path.is_private)
            .with_files(path.files.clone())
            .with_trackers(path.trackers.clone())
            .with_metadata_bytes(path.metadata_bytes.clone());
            if auto_extract {
                request = request.enable_auto_extract();
            }
            let download = match request.clone().into_download() {
                Ok(download) => download,
                Err(error) => {
                    let label = path
                        .file_path()
                        .map_or_else(|| path.display_name.clone(), |p| path_label(p));
                    self.push_toast(
                        &format!("invalid torrent source {label}: {error}"),
                        ToastKind::Error,
                    );
                    continue;
                },
            };
            downloads.push(queued_add(vec![download], Vec::new(), move |reply| {
                EngineCommand::AddTorrentPrepared { request, reply }
            }));
        }

        if downloads.is_empty() {
            return Task::none();
        }

        self.overlay = Overlay::None;
        self.queue_new_downloads(downloads)
    }

    pub(super) fn add_urls_action(&mut self, action: text_editor::Action) -> Task<Message> {
        self.add_urls.perform(action);
        self.refresh_add_sources()
    }

    pub(super) fn add_subfolder_name_changed(&mut self, name: String) -> Task<Message> {
        self.add_subfolder_name = name;
        self.add_subfolder_name_dirty = true;
        Task::none()
    }

    pub(super) fn refresh_add_sources(&mut self) -> Task<Message> {
        let parsed = parse_add_urls(&self.add_urls.text());
        let parsed_magnets = parsed.magnets.clone();
        let parsed_http_urls = parsed.http_urls.clone();

        self.add_torrent_files
            .retain(|torrent| match &torrent.source {
                AddTorrentSource::File { .. } => true,
                AddTorrentSource::Magnet { magnet, .. } => parsed_magnets.contains(magnet),
            });

        let resolved_magnets = self
            .add_torrent_files
            .iter()
            .filter_map(AddTorrentFile::magnet)
            .map(str::to_string)
            .collect::<Vec<_>>();

        self.add_magnet_previews.retain(|preview| {
            parsed_magnets.contains(&preview.magnet) && !resolved_magnets.contains(&preview.magnet)
        });
        let mut tasks = Vec::new();
        for request_id in
            removed_http_preview_request_ids(&self.add_http_previews, &parsed_http_urls)
        {
            tasks.push(self.cancel_http_preview(request_id));
        }
        self.add_http_previews
            .retain(|preview| parsed_http_urls.contains(&preview.url));

        if should_request_http_previews(&parsed_http_urls) {
            for url in &parsed_http_urls {
                let already_pending = self
                    .add_http_previews
                    .iter()
                    .any(|preview| preview.url == *url);
                if already_pending {
                    continue;
                }

                let request_id = self.add_next_http_request_id;
                self.add_next_http_request_id = self.add_next_http_request_id.saturating_add(1);
                self.add_http_previews.push(AddHttpPreview {
                    url: url.clone(),
                    request_id,
                    state: AddHttpPreviewState::Resolving,
                });
                tasks.push(self.request_http_preview(request_id, url.clone()));
            }
        } else {
            for request_id in drain_http_preview_request_ids(&mut self.add_http_previews) {
                tasks.push(self.cancel_http_preview(request_id));
            }
        }

        for magnet in &parsed_magnets {
            let already_resolved = resolved_magnets.contains(magnet);
            let already_pending = self
                .add_magnet_previews
                .iter()
                .any(|preview| preview.magnet == *magnet);
            if already_resolved || already_pending {
                continue;
            }

            let request_id = self.add_next_magnet_request_id;
            self.add_next_magnet_request_id = self.add_next_magnet_request_id.saturating_add(1);
            self.add_magnet_previews.push(AddMagnetPreview {
                magnet: magnet.clone(),
                request_id,
                state: AddMagnetPreviewState::Resolving,
            });
            tasks.push(self.request_magnet_preview(request_id, magnet.clone()));
        }

        let filtered = parsed
            .entries
            .iter()
            .cloned()
            .zip(parsed.entry_previews.iter().cloned())
            .filter(|(entry, _)| !resolved_magnets.contains(entry))
            .collect::<Vec<_>>();

        self.add_url_entries = filtered.iter().map(|(entry, _)| entry.clone()).collect();
        self.add_url_preview = filtered
            .iter()
            .map(|(entry, preview)| {
                if is_magnet(entry) {
                    preview.clone()
                } else {
                    http_preview_name(entry, &self.add_http_previews)
                        .unwrap_or_else(|| preview.clone())
                }
            })
            .take(ADD_URL_PREVIEW_LIMIT)
            .collect();
        self.add_http_count = parsed.http_count;
        self.add_magnet_count = self
            .add_url_entries
            .iter()
            .filter(|entry| is_magnet(entry))
            .count();
        self.add_single_url_name = add_single_http_name(&parsed, &self.add_http_previews);
        self.add_has_archive_url =
            addable_has_archive_url(&self.add_url_entries, &self.add_http_previews);
        repair_add_source_selection(
            &mut self.add_selected_source,
            &self.add_url_entries,
            &self.add_torrent_files,
        );

        self.refresh_add_subfolder_suggestion();
        Task::batch(tasks)
    }

    fn request_magnet_preview(&self, request_id: u64, magnet: String) -> Task<Message> {
        let tx = self.engine_tx.clone();
        Task::perform(
            async move {
                let (reply, mut rx) = tokio::sync::mpsc::channel(1);
                let send = tx
                    .send(EngineCommand::ResolveMagnetPreview {
                        request_id,
                        magnet: magnet.clone(),
                        reply,
                    })
                    .await;
                if send.is_err() {
                    return shio_core::MagnetPreviewResult {
                        request_id,
                        magnet,
                        result: Err("metadata service unavailable".to_string()),
                    };
                }
                rx.recv()
                    .await
                    .unwrap_or_else(|| shio_core::MagnetPreviewResult {
                        request_id,
                        magnet,
                        result: Err("metadata service unavailable".to_string()),
                    })
            },
            Message::MagnetPreviewResolved,
        )
    }

    fn request_http_preview(&self, request_id: u64, url: String) -> Task<Message> {
        let tx = self.engine_tx.clone();
        Task::perform(
            async move {
                let (reply, mut rx) = tokio::sync::mpsc::channel(1);
                let send = tx
                    .send(EngineCommand::ResolveHttpPreview {
                        request_id,
                        url,
                        reply,
                    })
                    .await;
                if send.is_err() {
                    return shio_core::HttpPreviewResult {
                        request_id,
                        state: shio_core::HttpPreviewState::Error {
                            message: "preview unavailable".to_string(),
                        },
                    };
                }
                rx.recv()
                    .await
                    .unwrap_or_else(|| shio_core::HttpPreviewResult {
                        request_id,
                        state: shio_core::HttpPreviewState::Error {
                            message: "preview unavailable".to_string(),
                        },
                    })
            },
            Message::HttpPreviewResolved,
        )
    }

    fn cancel_http_preview(&self, request_id: u64) -> Task<Message> {
        self.send_engine_cmd_unacked(EngineCommand::CancelHttpPreview { request_id })
    }

    pub(super) fn magnet_preview_resolved(
        &mut self,
        result: shio_core::MagnetPreviewResult,
    ) -> Task<Message> {
        let parsed = parse_add_urls(&self.add_urls.text());
        apply_magnet_preview_result(
            result,
            &parsed,
            &mut self.add_url_entries,
            &mut self.add_magnet_previews,
            &mut self.add_torrent_files,
        );
        self.add_url_preview = self
            .add_url_entries
            .iter()
            .map(|entry| {
                if is_magnet(entry) {
                    "magnet link".to_string()
                } else {
                    http_preview_name(entry, &self.add_http_previews)
                        .unwrap_or_else(|| shio_core::extract_filename(entry, None))
                }
            })
            .take(ADD_URL_PREVIEW_LIMIT)
            .collect();
        self.add_magnet_count = self
            .add_url_entries
            .iter()
            .filter(|entry| is_magnet(entry))
            .count();
        self.add_single_url_name = add_single_http_name(&parsed, &self.add_http_previews);
        self.add_has_archive_url =
            addable_has_archive_url(&self.add_url_entries, &self.add_http_previews);
        repair_add_source_selection(
            &mut self.add_selected_source,
            &self.add_url_entries,
            &self.add_torrent_files,
        );
        self.refresh_add_subfolder_suggestion();
        Task::none()
    }

    pub(super) fn http_preview_resolved(
        &mut self,
        result: shio_core::HttpPreviewResult,
    ) -> Task<Message> {
        let parsed = parse_add_urls(&self.add_urls.text());
        apply_http_preview_result(result, &parsed, &mut self.add_http_previews);
        self.add_url_preview = self
            .add_url_entries
            .iter()
            .map(|entry| {
                if is_magnet(entry) {
                    "magnet link".to_string()
                } else {
                    http_preview_name(entry, &self.add_http_previews)
                        .unwrap_or_else(|| shio_core::extract_filename(entry, None))
                }
            })
            .take(ADD_URL_PREVIEW_LIMIT)
            .collect();
        self.add_single_url_name = add_single_http_name(&parsed, &self.add_http_previews);
        self.add_has_archive_url =
            addable_has_archive_url(&self.add_url_entries, &self.add_http_previews);
        repair_add_source_selection(
            &mut self.add_selected_source,
            &self.add_url_entries,
            &self.add_torrent_files,
        );
        self.refresh_add_subfolder_suggestion();
        Task::none()
    }

    pub(super) fn add_filename_changed(&mut self, name: String) -> Task<Message> {
        self.add_filename = name;
        Task::none()
    }

    pub(super) fn add_torrent_search_changed(&mut self, query: String) -> Task<Message> {
        self.add_torrent_search = query;
        Task::none()
    }

    pub(super) fn add_torrent_search_cleared(&mut self) -> Task<Message> {
        self.add_torrent_search.clear();
        Task::none()
    }

    pub(super) fn add_torrent_search_focus() -> Task<Message> {
        iced::widget::operation::focus(crate::app::ADD_TORRENT_SEARCH_INPUT_ID.clone())
    }

    pub(super) fn add_source_selected(&mut self, source: AddSourceId) -> Task<Message> {
        self.add_selected_source = Some(source);
        Task::none()
    }

    pub(super) fn pick_torrent_files() -> Task<Message> {
        Task::perform(pick_torrent_files(), Message::TorrentFilesPicked)
    }

    pub(super) fn torrent_files_picked(&mut self, paths: Option<Vec<PathBuf>>) -> Task<Message> {
        let Some(paths) = paths else {
            return Task::none();
        };
        self.ingest_torrent_files(paths, !self.show_add_dialog())
    }

    pub(super) fn open_associated_sources(
        &mut self,
        launch: crate::platform::AssociationLaunch,
    ) -> Task<Message> {
        if launch.magnets.is_empty() && launch.torrent_files.is_empty() {
            return Task::none();
        }

        let mut tasks = vec![self.reset_add_dialog_state()];
        if !launch.magnets.is_empty() {
            self.add_urls = text_editor::Content::with_text(&launch.magnets.join("\n"));
            tasks.push(self.refresh_add_sources());
        }
        if !launch.torrent_files.is_empty() {
            tasks.push(self.ingest_torrent_files(launch.torrent_files, false));
        }

        Task::batch(tasks)
    }

    pub(super) fn torrent_files_dropped(&mut self, paths: Vec<PathBuf>) -> Task<Message> {
        if self.show_first_run() {
            return Task::none();
        }
        self.ingest_torrent_files(paths, !self.show_add_dialog())
    }

    pub(super) fn torrent_file_toggled(
        &mut self,
        torrent_index: usize,
        file_index: usize,
        selected: bool,
    ) -> Task<Message> {
        if let Some(file) = self
            .add_torrent_files
            .get_mut(torrent_index)
            .and_then(|torrent| torrent.files.get_mut(file_index))
        {
            file.selected = selected;
            self.refresh_add_subfolder_suggestion();
        }
        Task::none()
    }

    pub(super) fn torrent_files_selection_changed(
        &mut self,
        torrent_index: usize,
        selected: bool,
    ) -> Task<Message> {
        if let Some(torrent) = self.add_torrent_files.get_mut(torrent_index) {
            for file in &mut torrent.files {
                file.selected = selected;
            }
            self.refresh_add_subfolder_suggestion();
        }
        Task::none()
    }

    pub(super) fn torrent_matching_selection_changed(
        &mut self,
        torrent_index: usize,
        selected: bool,
    ) -> Task<Message> {
        if let Some(torrent) = self.add_torrent_files.get_mut(torrent_index) {
            set_matching_torrent_files(torrent, &self.add_torrent_search, selected);
            self.refresh_add_subfolder_suggestion();
        }
        Task::none()
    }

    pub(super) fn cancel_dialog_open(&mut self, id: DownloadId) -> Task<Message> {
        self.overlay = Overlay::CancelConfirm(self.delete_targets_for(id));
        Task::none()
    }

    pub(super) fn cancel_dialog_cancel(&mut self) -> Task<Message> {
        self.overlay = Overlay::None;
        Task::none()
    }

    pub(super) fn cancel_dialog_confirm(&mut self) -> Task<Message> {
        let Overlay::CancelConfirm(targets) = std::mem::replace(&mut self.overlay, Overlay::None)
        else {
            return Task::none();
        };
        let tasks: Vec<_> = targets.iter().map(|&id| self.cancel_download(id)).collect();
        Task::batch(tasks)
    }

    pub(super) fn delete_dialog_open(&mut self, id: DownloadId) -> Task<Message> {
        self.overlay = Overlay::DeleteConfirm(self.delete_targets_for(id));
        Task::none()
    }

    pub(super) fn delete_dialog_cancel(&mut self) -> Task<Message> {
        self.overlay = Overlay::None;
        Task::none()
    }

    pub(super) fn delete_dialog_confirm_files(&mut self) -> Task<Message> {
        let Overlay::DeleteConfirm(targets) = std::mem::replace(&mut self.overlay, Overlay::None)
        else {
            return Task::none();
        };
        self.remove_ids(&targets, true)
    }

    pub(super) fn delete_dialog_confirm_remove(&mut self) -> Task<Message> {
        let Overlay::DeleteConfirm(targets) = std::mem::replace(&mut self.overlay, Overlay::None)
        else {
            return Task::none();
        };
        self.remove_ids(&targets, false)
    }

    pub(super) fn edit_dialog_open(&mut self, id: DownloadId) -> Task<Message> {
        if let Some(dl) = self.downloads.iter().find(|d| d.id == id) {
            self.overlay = Overlay::Edit(id);
            self.edit_filename.clone_from(&dl.filename);
            self.edit_save_path = dl.save_path.to_string_lossy().to_string();
            self.edit_conflict = false;
            return self.check_edit_conflict(id);
        }
        Task::none()
    }

    pub(super) fn edit_filename_changed(&mut self, s: String) -> Task<Message> {
        self.edit_filename = s;
        self.edit_target()
            .map_or_else(Task::none, |id| self.check_edit_conflict(id))
    }

    pub(super) fn edit_save_path_changed(&mut self, s: String) -> Task<Message> {
        self.edit_save_path = s;
        self.edit_target()
            .map_or_else(Task::none, |id| self.check_edit_conflict(id))
    }

    pub(super) fn edit_pick_save_path() -> Task<Message> {
        Task::perform(
            async {
                rfd::AsyncFileDialog::new()
                    .set_title("choose save folder")
                    .pick_folder()
                    .await
                    .map(|h| h.path().to_path_buf())
            },
            Message::EditPickSavePathResult,
        )
    }

    pub(super) fn edit_pick_save_path_result(&mut self, path: Option<PathBuf>) -> Task<Message> {
        if let Some(path) = path {
            self.edit_save_path = path.to_string_lossy().to_string();
            return self
                .edit_target()
                .map_or_else(Task::none, |id| self.check_edit_conflict(id));
        }
        Task::none()
    }

    pub(super) fn edit_dialog_cancel(&mut self) -> Task<Message> {
        self.overlay = Overlay::None;
        Task::none()
    }

    pub(super) fn edit_dialog_confirm(&mut self, id: DownloadId) -> Task<Message> {
        let filename = shio_core::sanitize_filename(&self.edit_filename);
        let save_path = PathBuf::from(&self.edit_save_path);

        let Some(dl) = self.downloads.iter().find(|d| d.id == id).cloned() else {
            self.overlay = Overlay::None;
            return Task::none();
        };

        let old_path = dl.file_path();
        let new_path = match dl.http().and_then(|h| h.subfolder.as_deref()) {
            Some(sub) if !sub.is_empty() => save_path.join(sub).join(&filename),
            _ => save_path.join(&filename),
        };
        let needs_rename = old_path != new_path;

        if !needs_rename {
            self.overlay = Overlay::None;
            let command_filename = filename.clone();
            let command_save_path = save_path.clone();
            return self.send_engine_cmd_with_action(
                move |reply| EngineCommand::UpdateMetadata {
                    id,
                    filename: command_filename,
                    save_path: command_save_path,
                    reply,
                },
                EngineAction::ApplyMetadata {
                    id,
                    filename,
                    save_path,
                },
            );
        }

        self.overlay = Overlay::None;
        Task::perform(
            async move {
                let result = rename_on_disk(old_path, new_path).await;
                EditRenameResult {
                    id,
                    filename,
                    save_path,
                    result,
                }
            },
            Message::EditRenameCompleted,
        )
    }

    pub(super) fn edit_conflict_checked(&mut self, result: EditConflictResult) -> Task<Message> {
        let EditConflictResult {
            id,
            filename,
            save_path,
            conflict,
        } = result;
        if self.edit_target() == Some(id)
            && self.edit_filename == filename
            && self.edit_save_path == save_path
        {
            self.edit_conflict = conflict;
        }
        Task::none()
    }

    pub(super) fn edit_rename_completed(&mut self, result: EditRenameResult) -> Task<Message> {
        if let Err(message) = result.result {
            tracing::warn!("rename_on_disk failed: {message}");
            self.push_toast("rename failed", ToastKind::Error);
            return Task::none();
        }
        let command_filename = result.filename.clone();
        let command_save_path = result.save_path.clone();
        self.send_engine_cmd_with_action(
            move |reply| EngineCommand::UpdateMetadata {
                id: result.id,
                filename: command_filename,
                save_path: command_save_path,
                reply,
            },
            EngineAction::ApplyMetadata {
                id: result.id,
                filename: result.filename,
                save_path: result.save_path,
            },
        )
    }

    fn cancel_all_http_previews(&mut self) -> Vec<Task<Message>> {
        drain_http_preview_request_ids(&mut self.add_http_previews)
            .into_iter()
            .map(|request_id| self.cancel_http_preview(request_id))
            .collect()
    }

    pub(super) fn reset_add_dialog_state(&mut self) -> Task<Message> {
        let tasks = self.cancel_all_http_previews();
        self.overlay = Overlay::AddDownload;
        self.add_urls = text_editor::Content::new();
        self.add_url_entries.clear();
        self.add_url_preview.clear();
        self.add_http_count = 0;
        self.add_magnet_count = 0;
        self.add_single_url_name = None;
        self.add_has_archive_url = false;
        self.add_http_previews.clear();
        self.add_magnet_previews.clear();
        self.add_torrent_files.clear();
        self.add_torrent_search.clear();
        self.add_selected_source = None;
        self.add_filename.clear();
        self.add_save_path = self.config.download_dir.to_string_lossy().to_string();
        self.add_create_subfolder = self.config.default_create_subfolder;
        self.add_subfolder_name.clear();
        self.add_subfolder_name_dirty = false;
        self.add_auto_extract = self.config.default_auto_extract;
        Task::batch(tasks)
    }

    fn ingest_torrent_files(&mut self, paths: Vec<PathBuf>, reset_form: bool) -> Task<Message> {
        let reset_task = if reset_form {
            self.reset_add_dialog_state()
        } else {
            self.overlay = Overlay::AddDownload;
            Task::none()
        };

        let paths = paths
            .into_iter()
            .filter(|path| {
                let is_torrent = path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("torrent"));
                is_torrent
                    && !self.add_torrent_files.iter().any(|existing| {
                        existing
                            .file_path()
                            .is_some_and(|existing| existing == path)
                    })
            })
            .collect::<Vec<_>>();

        self.refresh_add_subfolder_suggestion();
        if paths.is_empty() {
            return reset_task;
        }

        Task::batch([
            reset_task,
            Task::perform(
                read_torrent_file_previews(paths),
                Message::TorrentFilesIngested,
            ),
        ])
    }

    pub(super) fn torrent_files_ingested(
        &mut self,
        results: Vec<Result<AddTorrentFile, String>>,
    ) -> Task<Message> {
        for result in results {
            match result {
                Ok(torrent) => {
                    if self
                        .add_torrent_files
                        .iter()
                        .any(|existing| existing.file_path() == torrent.file_path())
                    {
                        continue;
                    }
                    self.add_torrent_files.push(torrent);
                },
                Err(message) => self.push_toast(&message, ToastKind::Error),
            }
        }
        repair_add_source_selection(
            &mut self.add_selected_source,
            &self.add_url_entries,
            &self.add_torrent_files,
        );
        self.refresh_add_subfolder_suggestion();
        Task::none()
    }

    fn queue_new_downloads(&self, downloads: Vec<QueuedAdd>) -> Task<Message> {
        let tx = self.engine_tx.clone();
        Task::perform(
            async move {
                let mut queued = Vec::new();
                let mut packages = Vec::new();
                for queued_add in downloads {
                    if let Err(error) = tx.send(queued_add.command).await {
                        return QueueDownloadsResult {
                            downloads: queued,
                            packages,
                            error: Some(format!("engine unavailable: {error}")),
                        };
                    }
                    match queued_add.ack.await {
                        Ok(Ok(())) => {
                            queued.extend(queued_add.downloads);
                            packages.extend(queued_add.packages);
                        },
                        Ok(Err(error)) => {
                            return QueueDownloadsResult {
                                downloads: queued,
                                packages,
                                error: Some(error.short_label()),
                            };
                        },
                        Err(_) => {
                            return QueueDownloadsResult {
                                downloads: queued,
                                packages,
                                error: Some("engine command acknowledgement dropped".to_string()),
                            };
                        },
                    }
                }
                QueueDownloadsResult {
                    downloads: queued,
                    packages,
                    error: None,
                }
            },
            Message::DownloadsQueued,
        )
    }

    pub(super) fn downloads_queued(&mut self, result: QueueDownloadsResult) -> Task<Message> {
        self.downloads.extend(result.downloads);
        self.packages.extend(result.packages);
        super::super::state::sync_package_rows(&mut self.downloads, &self.packages);
        if let Some(message) = result.error {
            tracing::error!("failed to queue download: {message}");
            self.push_toast("download could not be queued", ToastKind::Error);
            return Task::none();
        }
        self.download_added()
    }

    pub(super) fn engine_command_delivered(
        &mut self,
        action: EngineAction,
        result: Result<(), String>,
    ) -> Task<Message> {
        if let Err(message) = result {
            tracing::error!("engine command failed: {message}");
            self.push_toast(&message, ToastKind::Error);
            return Task::none();
        }
        self.apply_engine_action(action);
        Task::none()
    }

    fn apply_engine_action(&mut self, action: EngineAction) {
        match action {
            EngineAction::None => {},
            EngineAction::SetStatus {
                id,
                status,
                clear_error,
            } => {
                if let Some(dl) = self.downloads.iter_mut().find(|d| d.id == id) {
                    dl.status = status;
                    if clear_error {
                        dl.error_message = None;
                    }
                }
            },
            EngineAction::Remove { id } => {
                self.downloads.retain(|d| d.id != id);
                self.forget_download(id);
            },
            EngineAction::SetPin { id, pinned } => {
                if let Some(dl) = self.downloads.iter_mut().find(|d| d.id == id) {
                    dl.pinned = pinned;
                }
                if pinned {
                    self.selection.remove(&id);
                    if self.selection_anchor == Some(id) {
                        self.selection_anchor = self.selection.iter().next().copied();
                    }
                }
            },
            EngineAction::PauseAll => {
                for dl in &mut self.downloads {
                    if matches!(
                        dl.status,
                        DownloadStatus::Downloading
                            | DownloadStatus::Starting
                            | DownloadStatus::FetchingMetadata
                            | DownloadStatus::Seeding
                    ) {
                        dl.status = DownloadStatus::Paused;
                    }
                }
            },
            EngineAction::ResumeAll => {
                for dl in &mut self.downloads {
                    if dl.status == DownloadStatus::Paused {
                        dl.status = DownloadStatus::Downloading;
                    }
                }
            },
            EngineAction::ApplyMetadata {
                id,
                filename,
                save_path,
            } => self.apply_edit_metadata(id, filename, save_path),
        }
    }

    fn apply_edit_metadata(&mut self, id: DownloadId, filename: String, save_path: PathBuf) {
        if let Some(d) = self.downloads.iter_mut().find(|d| d.id == id) {
            d.filename = filename;
            d.save_path = save_path;
        }
    }

    fn check_edit_conflict(&self, id: DownloadId) -> Task<Message> {
        let filename = self.edit_filename.clone();
        let save_path = self.edit_save_path.clone();
        let Some(download) = self.downloads.iter().find(|d| d.id == id).cloned() else {
            return Task::none();
        };
        Task::perform(
            async move {
                let conflict = edit_conflict_on_disk(&filename, &save_path, &download).await;
                EditConflictResult {
                    id,
                    filename,
                    save_path,
                    conflict,
                }
            },
            Message::EditConflictChecked,
        )
    }
}

async fn read_torrent_file_previews(paths: Vec<PathBuf>) -> Vec<Result<AddTorrentFile, String>> {
    let mut previews = Vec::with_capacity(paths.len());
    for path in paths {
        let bytes = match tokio::fs::read(&path).await {
            Ok(bytes) => bytes,
            Err(error) => {
                previews.push(Err(format!(
                    "failed to read {}: {error}",
                    path_label(&path)
                )));
                continue;
            },
        };
        previews.push(torrent_file_preview(path, bytes));
    }
    previews
}

async fn edit_conflict_on_disk(
    new_filename: &str,
    new_save_path: &str,
    original: &Download,
) -> bool {
    let new_filename_sanitized = shio_core::sanitize_filename(new_filename);
    if new_filename_sanitized.is_empty() {
        return false;
    }
    let new_path = PathBuf::from(new_save_path).join(&new_filename_sanitized);
    let old_path = original.save_path.join(&original.filename);
    if new_path == old_path {
        return false;
    }
    tokio::fs::metadata(&old_path).await.is_ok() && tokio::fs::metadata(&new_path).await.is_ok()
}

async fn rename_on_disk(old: PathBuf, new: PathBuf) -> Result<(), String> {
    if tokio::fs::metadata(&old).await.is_err() {
        return Ok(());
    }
    if tokio::fs::metadata(&new).await.is_ok() {
        return Err("destination already exists".to_string());
    }
    if let Some(parent) = new.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("create destination folder: {e}"))?;
    }
    tokio::fs::rename(&old, &new)
        .await
        .map_err(|e| format!("rename file: {e}"))
}

async fn pick_torrent_files() -> Option<Vec<PathBuf>> {
    rfd::AsyncFileDialog::new()
        .add_filter("torrent", &["torrent"])
        .set_title("choose torrent files")
        .pick_files()
        .await
        .map(|handles| {
            handles
                .into_iter()
                .map(|handle| handle.path().to_path_buf())
                .collect()
        })
}
