use crate::message::{
    DownloadColumn, DownloadColumnWidths, FirstRunStep, SettingsCategory, SortCol, SortDirection,
    Tab,
};
use crate::message::{EngineAction, Message, NoticeId};
use crate::theme::{ResolvedTheme, ThemeCatalog, ThemeSelection};
use iced::Task;
use iced::animation::Easing;
use iced::keyboard::Modifiers;
use iced::widget::text_editor;
use iced::{Animation, time::Duration};
use shio_core::{
    AppConfig, ArchivePackage, Download, DownloadId, DownloadKind, DownloadStatus, EngineCommand,
    HttpState, PackageExtractState,
};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

pub(super) const TOAST_VISIBLE: Duration = Duration::from_millis(3_500);
const NAME_CAROUSEL_STEP: Duration = Duration::from_millis(400);
static CONFIG_SAVE_GATE: Mutex<()> = Mutex::new(());
static LATEST_CONFIG_SAVE_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone)]
pub(crate) struct Toast {
    pub(crate) message: String,
    pub(crate) kind: ToastKind,
    pub(crate) shown: Animation<bool>,
    pub(crate) dismiss_at: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToastKind {
    Success,
    Error,
    Info,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NoticeAction {
    OpenLogs,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PersistentNotice {
    pub(crate) id: NoticeId,
    pub(crate) title: String,
    pub(crate) message: String,
    pub(crate) action: Option<NoticeAction>,
}

impl PersistentNotice {
    pub(crate) fn config_save_failed(message: String) -> Self {
        Self {
            id: NoticeId::ConfigSave,
            title: "settings could not be saved".to_string(),
            message,
            action: Some(NoticeAction::OpenLogs),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AddTorrentSource {
    File { path: PathBuf },
    Magnet { magnet: String, request_id: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AddMagnetPreviewState {
    Resolving,
    Failed { message: String },
}

#[derive(Debug, Clone)]
pub(crate) struct AddMagnetPreview {
    pub(crate) magnet: String,
    pub(crate) request_id: u64,
    pub(crate) state: AddMagnetPreviewState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AddHttpPreviewState {
    Resolving,
    Ready(shio_core::HttpPreview),
    Blocked { reason: String },
    Failed { message: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AddHttpPreview {
    pub(crate) url: String,
    pub(crate) request_id: u64,
    pub(crate) state: AddHttpPreviewState,
}

#[derive(Debug, Clone)]
pub(crate) struct AddTorrentPreview {
    pub(crate) source: AddTorrentSource,
    pub(crate) display_name: String,
    pub(crate) is_private: bool,
    pub(crate) files: Vec<shio_core::TorrentFile>,
    pub(crate) trackers: Vec<String>,
    pub(crate) metadata_bytes: Option<Vec<u8>>,
}

pub(crate) type AddTorrentFile = AddTorrentPreview;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AddSourceId {
    Url(usize),
    TorrentFile(PathBuf),
    MagnetPreview(u64),
}

impl AddTorrentPreview {
    pub(crate) const fn file_path(&self) -> Option<&PathBuf> {
        match &self.source {
            AddTorrentSource::File { path } => Some(path),
            AddTorrentSource::Magnet { .. } => None,
        }
    }

    pub(crate) fn magnet(&self) -> Option<&str> {
        match &self.source {
            AddTorrentSource::File { .. } => None,
            AddTorrentSource::Magnet { magnet, .. } => Some(magnet),
        }
    }

    pub(crate) fn selected_count(&self) -> usize {
        self.files.iter().filter(|file| file.selected).count()
    }

    pub(crate) fn selected_size(&self) -> u64 {
        self.files
            .iter()
            .filter(|file| file.selected)
            .map(|file| file.size)
            .sum()
    }

    pub(crate) fn has_selection(&self) -> bool {
        self.selected_count() > 0
    }
}

pub(crate) struct Shio {
    pub(super) engine_tx: tokio::sync::mpsc::Sender<EngineCommand>,
    pub(super) startup_problem: Option<StartupProblem>,
    pub(super) window: WindowState,
    pub(super) started_at: Instant,
    pub(super) now: Instant,
    pub(super) downloads: Vec<Download>,
    pub(super) packages: Vec<ArchivePackage>,
    pub(super) selection: HashSet<DownloadId>,
    pub(super) column_widths: DownloadColumnWidths,
    pub(super) column_resize: Option<ColumnResize>,
    pub(super) selection_anchor: Option<DownloadId>,
    pub(super) modifiers: Modifiers,
    pub(super) search_text: String,
    pub(super) search_query: String,
    pub(super) search_type: Option<crate::search::TypeFilter>,
    pub(super) search_size: crate::search::SizeFilter,
    pub(super) search_matcher: std::cell::RefCell<nucleo_matcher::Matcher>,
    pub(super) suggestion_index: Option<usize>,
    pub(super) active_tab: Tab,
    pub(super) sort_column: SortCol,
    pub(super) sort_direction: SortDirection,
    pub(super) manual_order: bool,
    pub(super) config: AppConfig,
    pub(super) theme_catalog: ThemeCatalog,
    pub(super) theme: ResolvedTheme,
    pub(super) overlay: Overlay,
    pub(super) first_run_step: FirstRunStep,
    pub(super) add_urls: text_editor::Content,
    pub(super) add_url_entries: Vec<String>,
    pub(super) add_url_preview: Vec<String>,
    pub(super) add_http_count: usize,
    pub(super) add_magnet_count: usize,
    pub(super) add_single_url_name: Option<String>,
    pub(super) add_has_archive_url: bool,
    pub(super) add_http_previews: Vec<AddHttpPreview>,
    pub(super) add_next_http_request_id: u64,
    pub(super) add_magnet_previews: Vec<AddMagnetPreview>,
    pub(super) add_next_magnet_request_id: u64,
    pub(super) add_torrent_files: Vec<AddTorrentFile>,
    pub(super) add_torrent_search: String,
    pub(super) add_selected_source: Option<AddSourceId>,
    pub(super) add_filename: String,
    pub(super) add_save_path: String,
    pub(super) add_create_subfolder: bool,
    pub(super) add_subfolder_name: String,
    pub(super) add_subfolder_name_dirty: bool,
    pub(super) add_auto_extract: bool,
    pub(super) settings_category: SettingsCategory,
    pub(super) settings_search: String,
    pub(super) torrent_port_input: String,
    pub(super) torrent_port_error: Option<String>,
    pub(super) torrent_ratio_input: String,
    pub(super) torrent_ratio_error: Option<String>,
    pub(super) torrent_seed_days_input: String,
    pub(super) torrent_seed_days_error: Option<String>,
    pub(super) password_input: String,
    pub(super) edit_filename: String,
    pub(super) edit_save_path: String,
    pub(super) edit_conflict: bool,
    pub(super) persistent_notices: Vec<PersistentNotice>,
    pub(super) config_save_id: u64,
    pub(super) toasts: Vec<Toast>,
    pub(super) last_clipboard_url: String,
    pub(super) drag_hover: Option<DragHover>,
}

pub(crate) struct ColumnResize {
    pub(crate) column: DownloadColumn,
    pub(crate) last_x: Option<f32>,
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct WindowState {
    pub(crate) id: Option<iced::window::Id>,
    pub(crate) focused: bool,
    pub(crate) size: Option<iced::Size>,
    pub(crate) maximized: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Overlay {
    None,
    FirstRun,
    AddDownload,
    Settings,
    CancelConfirm(Vec<DownloadId>),
    DeleteConfirm(Vec<DownloadId>),
    Password(DownloadId),
    Edit(DownloadId),
}

#[derive(Debug, Clone)]
pub(crate) struct StartupProblem {
    pub(crate) title: String,
    pub(crate) message: String,
    pub(crate) path_label: String,
    pub(crate) db_path: PathBuf,
    pub(crate) log_path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DropSide {
    Above,
    Below,
}

pub(crate) struct DragHover {
    pub(crate) source_id: shio_core::DownloadId,
    pub(crate) target_id: Option<shio_core::DownloadId>,
    pub(crate) side: DropSide,
    pub(crate) cursor_y: f32,
}

impl Shio {
    pub(crate) fn new() -> (Self, Task<Message>) {
        let startup_config = load_startup_config();
        let first_run = matches!(startup_config, StartupConfig::FirstRun(_));
        let mut config = startup_config.config().clone();
        let theme_catalog = ThemeCatalog::load(&AppConfig::theme_dir());
        let theme_selection = ThemeSelection::from_config(&config.theme);
        let theme = theme_catalog.resolve(&theme_selection);
        if theme.used_fallback {
            config.theme.id = theme.id.to_string();
        }
        tracing::debug!("theme resolved: {}", theme.id);

        let default_save_path = config.download_dir.to_string_lossy().to_string();
        let default_create_subfolder = config.default_create_subfolder;
        let default_auto_extract = config.default_auto_extract;
        let torrent_port_input = config.torrent.listen_port.to_string();
        let torrent_ratio_input = seed_policy_ratio_text(config.torrent.seed_policy);
        let torrent_seed_days_input = seed_policy_days_text(config.torrent.seed_policy);

        let data_dir = AppConfig::data_dir();
        let db_path = data_dir.join("shio.db");
        let startup = match startup_config {
            StartupConfig::Invalid { path, error } => {
                tracing::error!("failed to load config: {error}");
                let (engine_tx, _rx) = tokio::sync::mpsc::channel(1);
                (
                    engine_tx,
                    Vec::new(),
                    Vec::new(),
                    Some(StartupProblem {
                        title: "config could not be loaded".to_string(),
                        message: error.to_string(),
                        path_label: "config".to_string(),
                        db_path: path,
                        log_path: crate::diagnostics::log_file(),
                    }),
                )
            },
            StartupConfig::FirstRun(_) | StartupConfig::Loaded(_) => {
                match shio_core::DownloadEngine::new(config.clone(), &db_path) {
                    Ok((engine, progress_rx)) => {
                        let engine_tx = engine.command_sender();
                        let initial_packages = engine.packages();
                        let mut initial_downloads = engine.downloads();
                        sync_package_rows(&mut initial_downloads, &initial_packages);
                        tokio::spawn(async move {
                            engine.run().await;
                        });
                        super::subscription::install_progress_receiver(progress_rx);
                        (engine_tx, initial_downloads, initial_packages, None)
                    },
                    Err(e) => {
                        tracing::error!("failed to start download engine: {e}");
                        let (engine_tx, _rx) = tokio::sync::mpsc::channel(1);
                        (
                            engine_tx,
                            Vec::new(),
                            Vec::new(),
                            Some(StartupProblem {
                                title: "database could not be opened".to_string(),
                                message: e.to_string(),
                                path_label: "database".to_string(),
                                db_path: db_path.clone(),
                                log_path: crate::diagnostics::log_file(),
                            }),
                        )
                    },
                }
            },
        };
        let (engine_tx, initial_downloads, initial_packages, startup_problem) = startup;
        let persistent_notices = Vec::new();

        let now = Instant::now();
        let app = Self {
            engine_tx,
            startup_problem,
            window: WindowState::default(),
            started_at: now,
            now,
            downloads: initial_downloads,
            packages: initial_packages,
            selection: HashSet::new(),
            column_widths: DownloadColumnWidths::default(),
            column_resize: None,
            selection_anchor: None,
            modifiers: Modifiers::default(),
            search_text: String::new(),
            search_query: String::new(),
            search_type: None,
            search_size: crate::search::SizeFilter::Any,
            search_matcher: std::cell::RefCell::new(crate::search::make_matcher()),
            suggestion_index: None,
            active_tab: Tab::All,
            sort_column: SortCol::DateAdded,
            sort_direction: SortDirection::Descending,
            manual_order: false,
            config,
            theme_catalog,
            theme,
            overlay: if first_run {
                Overlay::FirstRun
            } else {
                Overlay::None
            },
            first_run_step: FirstRunStep::Look,
            add_urls: text_editor::Content::new(),
            add_url_entries: Vec::new(),
            add_url_preview: Vec::new(),
            add_http_count: 0,
            add_magnet_count: 0,
            add_single_url_name: None,
            add_has_archive_url: false,
            add_http_previews: Vec::new(),
            add_next_http_request_id: 1,
            add_magnet_previews: Vec::new(),
            add_next_magnet_request_id: 1,
            add_torrent_files: Vec::new(),
            add_torrent_search: String::new(),
            add_selected_source: None,
            add_filename: String::new(),
            add_save_path: default_save_path,
            add_create_subfolder: default_create_subfolder,
            add_subfolder_name: String::new(),
            add_subfolder_name_dirty: false,
            add_auto_extract: default_auto_extract,
            settings_category: SettingsCategory::General,
            settings_search: String::new(),
            torrent_port_input,
            torrent_port_error: None,
            torrent_ratio_input,
            torrent_ratio_error: None,
            torrent_seed_days_input,
            torrent_seed_days_error: None,
            password_input: String::new(),
            edit_filename: String::new(),
            edit_save_path: String::new(),
            edit_conflict: false,
            persistent_notices,
            config_save_id: 0,
            toasts: Vec::new(),
            last_clipboard_url: String::new(),
            drag_hover: None,
        };

        let launch = crate::platform::association_launch_from_args(std::env::args_os());
        let startup_task = if launch == crate::platform::AssociationLaunch::default() {
            Task::none()
        } else {
            Task::done(Message::OpenAssociatedSources(launch))
        };

        (app, startup_task)
    }

    pub(super) fn filtered_downloads(&self) -> Vec<(&Download, Option<Vec<u32>>)> {
        let mut matcher = self.search_matcher.borrow_mut();

        self.downloads
            .iter()
            .filter(|d| !self.is_package_child(d.id))
            .filter(|d| match self.active_tab {
                Tab::All => true,
                Tab::Active => matches!(
                    d.status,
                    DownloadStatus::Downloading
                        | DownloadStatus::Starting
                        | DownloadStatus::Extracting
                        | DownloadStatus::FetchingMetadata
                        | DownloadStatus::Seeding
                ),
                Tab::Completed => d.status == DownloadStatus::Completed,
                Tab::Queued => matches!(d.status, DownloadStatus::Queued | DownloadStatus::Pending),
                Tab::Errors => d.status.is_failed(),
            })
            .filter(|d| {
                self.search_type.is_none_or(|kind| kind.matches_download(d))
                    && self.search_size.matches(d.total_size)
            })
            .filter_map(|d| {
                if self.search_query.is_empty() {
                    return Some((d, None));
                }
                crate::search::match_download(&mut matcher, d, &self.search_query)
                    .map(|m| (d, Some(m.filename_indices)))
            })
            .collect()
    }

    pub(super) fn sorted_downloads<'a>(
        &self,
        downloads: Vec<(&'a Download, Option<Vec<u32>>)>,
    ) -> Vec<(&'a Download, Option<Vec<u32>>)> {
        if self.manual_order {
            return super::sort::pinned_first(downloads);
        }
        super::sort::sorted(downloads, self.sort_column, self.sort_direction)
    }

    pub(crate) const fn palette(&self) -> &crate::style::Palette {
        &self.theme.palette
    }

    pub(super) const fn show_first_run(&self) -> bool {
        matches!(self.overlay, Overlay::FirstRun)
    }

    pub(super) const fn show_add_dialog(&self) -> bool {
        matches!(self.overlay, Overlay::AddDownload)
    }

    pub(super) const fn show_settings(&self) -> bool {
        matches!(self.overlay, Overlay::Settings)
    }

    pub(super) fn cancel_confirm_targets(&self) -> Option<&[DownloadId]> {
        match &self.overlay {
            Overlay::CancelConfirm(targets) => Some(targets),
            _ => None,
        }
    }

    pub(super) fn delete_confirm_targets(&self) -> Option<&[DownloadId]> {
        match &self.overlay {
            Overlay::DeleteConfirm(targets) => Some(targets),
            _ => None,
        }
    }

    pub(super) const fn password_prompt(&self) -> Option<DownloadId> {
        match self.overlay {
            Overlay::Password(id) => Some(id),
            _ => None,
        }
    }

    pub(super) const fn edit_target(&self) -> Option<DownloadId> {
        match self.overlay {
            Overlay::Edit(id) => Some(id),
            _ => None,
        }
    }

    pub(super) fn reparse_search(&mut self) {
        self.suggestion_index = None;
        let parsed = crate::search::SearchQuery::parse(&self.search_text);
        self.search_query = parsed.text;
        self.search_type = parsed.type_filter;
        self.search_size = parsed.size_filter;
    }

    pub(super) fn reorder_download(
        &mut self,
        source_id: DownloadId,
        target_id: DownloadId,
        side: super::state::DropSide,
    ) {
        if source_id == target_id {
            return;
        }
        if self.is_pinned(source_id) != self.is_pinned(target_id) {
            return;
        }
        if let Some(src_pos) = self.downloads.iter().position(|d| d.id == source_id) {
            let item = self.downloads.remove(src_pos);
            let mut tgt_pos = self
                .downloads
                .iter()
                .position(|d| d.id == target_id)
                .unwrap_or(self.downloads.len());
            if side == super::state::DropSide::Below {
                tgt_pos = (tgt_pos + 1).min(self.downloads.len());
            }
            self.downloads.insert(tgt_pos, item);
            self.manual_order = true;
        }
    }

    pub(super) fn send_engine_cmd_unacked(&self, cmd: EngineCommand) -> Task<Message> {
        let tx = self.engine_tx.clone();
        Task::perform(
            async move {
                tx.send(cmd)
                    .await
                    .map_err(|e| format!("engine unavailable: {e}"))
            },
            |result| Message::EngineCommandDelivered {
                action: EngineAction::None,
                result,
            },
        )
    }

    pub(super) fn send_engine_cmd_with_action<F>(
        &self,
        build: F,
        action: EngineAction,
    ) -> Task<Message>
    where
        F: FnOnce(tokio::sync::oneshot::Sender<shio_core::Result<()>>) -> EngineCommand,
    {
        let tx = self.engine_tx.clone();
        let (reply, ack) = tokio::sync::oneshot::channel();
        let cmd = build(reply);
        Task::perform(
            async move {
                let result = match tx.send(cmd).await {
                    Ok(()) => match ack.await {
                        Ok(Ok(())) => Ok(()),
                        Ok(Err(error)) => Err(error.short_label()),
                        Err(_) => Err("engine command acknowledgement dropped".to_string()),
                    },
                    Err(error) => Err(format!("engine unavailable: {error}")),
                };
                (action, result)
            },
            |(action, result)| Message::EngineCommandDelivered { action, result },
        )
    }

    pub(super) fn push_toast(&mut self, message: &str, kind: ToastKind) {
        let now = Instant::now();
        self.now = now;
        let shown = Animation::new(false)
            .quick()
            .easing(Easing::EaseOut)
            .go(true, now);
        self.toasts.push(Toast {
            message: message.to_string(),
            kind,
            shown,
            dismiss_at: now + TOAST_VISIBLE,
        });
    }

    pub(super) fn tick_toasts(&mut self) -> bool {
        let now = self.now;
        for toast in &mut self.toasts {
            if toast.shown.value() && now >= toast.dismiss_at {
                toast.shown.go_mut(false, now);
            }
        }
        self.toasts
            .retain(|t| t.shown.value() || t.shown.is_animating(now));
        self.toasts.iter().any(|t| t.shown.is_animating(now))
    }

    pub(super) fn name_carousel_offset(&self) -> usize {
        name_carousel_offset(self.started_at, self.now)
    }

    pub(super) fn forget_download(&mut self, id: DownloadId) {
        self.selection.remove(&id);
        if self.selection_anchor == Some(id) {
            self.selection_anchor = self.selection.iter().next().copied();
        }
    }

    pub(super) fn persist_config(&mut self) -> Task<Message> {
        self.config_save_id = self.config_save_id.saturating_add(1);
        let id = self.config_save_id;
        LATEST_CONFIG_SAVE_ID.store(id, Ordering::Release);
        let config = self.config.clone();
        Task::perform(
            async move {
                let result = tokio::task::spawn_blocking(move || save_latest_config(id, &config))
                    .await
                    .map_err(|e| e.to_string())
                    .and_then(std::convert::identity);
                (id, result)
            },
            |(id, result)| Message::ConfigPersisted { id, result },
        )
    }

    pub(super) fn config_persisted(
        &mut self,
        id: u64,
        result: Result<(), String>,
    ) -> Task<Message> {
        if id != self.config_save_id {
            return Task::none();
        }
        match result {
            Ok(()) => dismiss_persistent_notice(&mut self.persistent_notices, NoticeId::ConfigSave),
            Err(message) => {
                tracing::warn!("failed to persist config: {message}");
                upsert_persistent_notice(
                    &mut self.persistent_notices,
                    PersistentNotice::config_save_failed(message),
                );
            },
        }
        Task::none()
    }

    pub(super) fn dismiss_notice(&mut self, id: NoticeId) -> Task<Message> {
        dismiss_persistent_notice(&mut self.persistent_notices, id);
        Task::none()
    }

    pub(super) fn is_pinned(&self, id: DownloadId) -> bool {
        self.downloads
            .iter()
            .find(|d| d.id == id)
            .is_some_and(|d| d.pinned)
    }

    pub(super) fn is_package_child(&self, id: DownloadId) -> bool {
        self.packages
            .iter()
            .any(|package| package.items.iter().any(|item| item.download_id == id))
    }

    pub(super) fn select_single(&mut self, id: DownloadId) {
        if self.is_pinned(id) {
            return;
        }
        self.selection.clear();
        self.selection.insert(id);
        self.selection_anchor = Some(id);
    }

    pub(super) fn toggle_selection(&mut self, id: DownloadId) {
        if self.is_pinned(id) {
            return;
        }
        if !self.selection.remove(&id) {
            self.selection.insert(id);
            self.selection_anchor = Some(id);
        } else if self.selection_anchor == Some(id) {
            self.selection_anchor = self.selection.iter().next().copied();
        }
    }

    pub(super) fn range_select(&mut self, target: DownloadId) {
        if self.is_pinned(target) {
            return;
        }
        let Some(anchor) = self.selection_anchor else {
            self.select_single(target);
            return;
        };
        let view = self.visible_order();
        let Some(a_idx) = view.iter().position(|id| *id == anchor) else {
            self.select_single(target);
            return;
        };
        let Some(t_idx) = view.iter().position(|id| *id == target) else {
            return;
        };
        let (lo, hi) = if a_idx <= t_idx {
            (a_idx, t_idx)
        } else {
            (t_idx, a_idx)
        };
        self.selection.clear();
        for id in &view[lo..=hi] {
            if !self.is_pinned(*id) {
                self.selection.insert(*id);
            }
        }
    }

    pub(super) fn select_all_visible(&mut self) {
        let view = self.visible_order();
        self.selection = view
            .iter()
            .copied()
            .filter(|id| !self.is_pinned(*id))
            .collect();
        self.selection_anchor = view.iter().copied().find(|id| !self.is_pinned(*id));
    }

    pub(super) fn clear_selection(&mut self) {
        self.selection.clear();
        self.selection_anchor = None;
    }

    pub(super) fn selected_ids(&self) -> Vec<DownloadId> {
        let order = self.visible_order();
        order
            .into_iter()
            .filter(|id| self.selection.contains(id))
            .collect()
    }

    pub(super) fn delete_targets_for(&self, id: DownloadId) -> Vec<DownloadId> {
        if self.selection.contains(&id) && self.selection.len() > 1 {
            self.selected_ids()
        } else {
            vec![id]
        }
    }

    fn visible_order(&self) -> Vec<DownloadId> {
        let filtered = self.filtered_downloads();
        let sorted = self.sorted_downloads(filtered);
        sorted.into_iter().map(|(d, _)| d.id).collect()
    }
}

pub(super) fn sync_package_rows(downloads: &mut Vec<Download>, packages: &[ArchivePackage]) {
    let package_ids: HashSet<DownloadId> = packages
        .iter()
        .map(|package| DownloadId(package.id.0))
        .collect();
    downloads.retain(|download| !package_ids.contains(&download.id));
    let child_snapshot = downloads.clone();
    downloads.extend(
        packages
            .iter()
            .map(|package| package_row(package, &child_snapshot)),
    );
}

fn package_row(package: &ArchivePackage, downloads: &[Download]) -> Download {
    let children: Vec<&Download> = package
        .items
        .iter()
        .filter_map(|item| {
            downloads
                .iter()
                .find(|download| download.id == item.download_id)
        })
        .collect();
    let downloaded = children.iter().map(|download| download.downloaded).sum();
    let total_size = children
        .iter()
        .map(|download| download.total_size)
        .collect::<Option<Vec<_>>>()
        .map(|sizes| sizes.into_iter().sum());
    let speed = children.iter().map(|download| download.speed).sum();
    let avg_speed = children.iter().map(|download| download.avg_speed).sum();
    let status = package_status(package, &children);
    Download {
        id: DownloadId(package.id.0),
        filename: package.name.clone(),
        save_path: package.save_path.clone(),
        total_size,
        downloaded,
        status,
        priority: 0,
        speed,
        avg_speed,
        error_message: package.error_message.clone(),
        created_at: package.created_at,
        started_at: children
            .iter()
            .filter_map(|download| download.started_at)
            .min(),
        completed_at: package.completed_at,
        retry_count: 0,
        max_retries: 0,
        pinned: package.pinned,
        kind: DownloadKind::Http(HttpState {
            url: String::new(),
            headers: Vec::new(),
            segments: 1,
            subfolder: None,
            auto_extract: package.auto_extract,
        }),
    }
}

fn package_status(package: &ArchivePackage, children: &[&Download]) -> DownloadStatus {
    match package.extract_state {
        PackageExtractState::Extracting => return DownloadStatus::Extracting,
        PackageExtractState::PasswordRequired => return DownloadStatus::PasswordRequired,
        PackageExtractState::Error => return DownloadStatus::ExtractError,
        PackageExtractState::Completed => return DownloadStatus::Completed,
        PackageExtractState::NotStarted => {},
    }
    if children.iter().any(|download| {
        matches!(
            download.status,
            DownloadStatus::Downloading | DownloadStatus::Starting
        )
    }) {
        return DownloadStatus::Downloading;
    }
    if children
        .iter()
        .any(|download| download.status == DownloadStatus::Queued)
    {
        return DownloadStatus::Queued;
    }
    if children.iter().any(|download| download.status.is_failed()) {
        return DownloadStatus::Error;
    }
    if !children.is_empty()
        && children
            .iter()
            .all(|download| download.status == DownloadStatus::Completed)
    {
        return DownloadStatus::Completed;
    }
    if children
        .iter()
        .all(|download| download.status == DownloadStatus::Paused)
    {
        return DownloadStatus::Paused;
    }
    if children
        .iter()
        .all(|download| download.status == DownloadStatus::Cancelled)
    {
        return DownloadStatus::Cancelled;
    }
    DownloadStatus::Queued
}

enum StartupConfig {
    FirstRun(AppConfig),
    Loaded(AppConfig),
    Invalid {
        path: PathBuf,
        error: shio_core::ShioError,
    },
}

impl StartupConfig {
    fn config(&self) -> &AppConfig {
        match self {
            Self::FirstRun(config) | Self::Loaded(config) => config,
            Self::Invalid { .. } => {
                static DEFAULT_CONFIG: std::sync::LazyLock<AppConfig> =
                    std::sync::LazyLock::new(AppConfig::default);
                &DEFAULT_CONFIG
            },
        }
    }
}

fn load_startup_config() -> StartupConfig {
    if !AppConfig::config_exists() {
        return StartupConfig::FirstRun(AppConfig::default());
    }

    match AppConfig::load() {
        Ok(config) => StartupConfig::Loaded(config),
        Err(error) => StartupConfig::Invalid {
            path: AppConfig::config_path(),
            error,
        },
    }
}

fn upsert_persistent_notice(notices: &mut Vec<PersistentNotice>, notice: PersistentNotice) {
    if let Some(existing) = notices.iter_mut().find(|existing| existing.id == notice.id) {
        *existing = notice;
        return;
    }
    notices.push(notice);
}

fn dismiss_persistent_notice(notices: &mut Vec<PersistentNotice>, id: NoticeId) {
    notices.retain(|notice| notice.id != id);
}

fn save_latest_config(id: u64, config: &AppConfig) -> Result<(), String> {
    let _guard = CONFIG_SAVE_GATE.lock().map_err(|e| e.to_string())?;
    if LATEST_CONFIG_SAVE_ID.load(Ordering::Acquire) != id {
        return Ok(());
    }
    config.save().map_err(|e| e.to_string())
}

fn seed_policy_ratio_text(policy: shio_core::SeedPolicy) -> String {
    match policy {
        shio_core::SeedPolicy::StopAtRatio { ratio }
        | shio_core::SeedPolicy::RatioOrTime { ratio, .. } => format!("{ratio:.2}"),
        _ => "1.00".to_string(),
    }
}

fn seed_policy_days_text(policy: shio_core::SeedPolicy) -> String {
    const SECONDS_PER_DAY: u64 = 86_400;
    match policy {
        shio_core::SeedPolicy::StopAtTime { seconds }
        | shio_core::SeedPolicy::RatioOrTime { seconds, .. } => {
            (seconds / SECONDS_PER_DAY).to_string()
        },
        _ => "7".to_string(),
    }
}

fn name_carousel_offset(started_at: Instant, now: Instant) -> usize {
    (now.duration_since(started_at).as_millis() / NAME_CAROUSEL_STEP.as_millis()) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_carousel_offset_advances_from_app_start() {
        let started_at = Instant::now();
        let now = started_at + NAME_CAROUSEL_STEP + NAME_CAROUSEL_STEP;

        assert_eq!(name_carousel_offset(started_at, now), 2);
    }

    #[test]
    fn config_save_error_upserts_persistent_notice() {
        let mut notices = vec![PersistentNotice {
            id: NoticeId::ConfigSave,
            title: "old".to_string(),
            message: "old".to_string(),
            action: None,
        }];

        upsert_persistent_notice(
            &mut notices,
            PersistentNotice::config_save_failed("permission denied".to_string()),
        );

        assert_eq!(
            notices,
            vec![PersistentNotice {
                id: NoticeId::ConfigSave,
                title: "settings could not be saved".to_string(),
                message: "permission denied".to_string(),
                action: Some(NoticeAction::OpenLogs),
            }]
        );
    }

    #[test]
    fn dismiss_notice_removes_matching_notice() {
        let mut notices = vec![PersistentNotice::config_save_failed("save".to_string())];

        dismiss_persistent_notice(&mut notices, NoticeId::ConfigSave);

        assert!(notices.is_empty());
    }
}
