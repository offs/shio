mod dialogs;
mod downloads;
mod dragdrop;
mod keyboard;
mod progress;
mod search;
mod settings;
mod window;

use super::state::Shio;
use crate::message::Message;
use iced::Task;
use std::sync::LazyLock;

pub(crate) static SEARCH_INPUT_ID: LazyLock<iced::widget::Id> =
    LazyLock::new(iced::widget::Id::unique);
pub(crate) static SETTINGS_SEARCH_INPUT_ID: LazyLock<iced::widget::Id> =
    LazyLock::new(iced::widget::Id::unique);
pub(crate) static ADD_TORRENT_SEARCH_INPUT_ID: LazyLock<iced::widget::Id> =
    LazyLock::new(iced::widget::Id::unique);

impl Shio {
    pub(crate) fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::AddDownloadPressed => self.add_dialog_open(),
            Message::CancelAddDownload => self.add_dialog_cancel(),
            Message::ConfirmAddDownload => self.add_dialog_confirm(),
            Message::AddUrlsAction(action) => self.add_urls_action(action),
            Message::AddFilenameChanged(name) => self.add_filename_changed(name),
            Message::PickTorrentFiles => Self::pick_torrent_files(),
            Message::TorrentFilesPicked(paths) => self.torrent_files_picked(paths),
            Message::TorrentFilesDropped(paths) => self.torrent_files_dropped(paths),
            Message::AddTorrentSearchChanged(query) => self.add_torrent_search_changed(query),
            Message::AddTorrentSearchCleared => self.add_torrent_search_cleared(),
            Message::AddTorrentSearchFocus => Self::add_torrent_search_focus(),
            Message::AddTorrentFileToggled {
                torrent_index,
                file_index,
                selected,
            } => self.torrent_file_toggled(torrent_index, file_index, selected),
            Message::AddTorrentFilesSelectionChanged {
                torrent_index,
                selected,
            } => self.torrent_files_selection_changed(torrent_index, selected),
            Message::AddTorrentMatchingSelectionChanged {
                torrent_index,
                selected,
            } => self.torrent_matching_selection_changed(torrent_index, selected),
            Message::AddSourceSelected(source) => self.add_source_selected(source),
            Message::OpenAssociatedSources(launch) => self.open_associated_sources(launch),
            Message::MagnetPreviewResolved(result) => self.magnet_preview_resolved(result),
            Message::HttpPreviewResolved(result) => self.http_preview_resolved(result),
            Message::TorrentFilesIngested(results) => self.torrent_files_ingested(results),
            Message::AddSubfolderNameChanged(s) => self.add_subfolder_name_changed(s),

            Message::PauseDownload(id) => self.pause_download(id),
            Message::ResumeDownload(id) => self.resume_download(id),
            Message::CancelDownload(id) => self.cancel_download(id),
            Message::RetryDownload(id) => self.retry_download(id),
            Message::ForceRecheck(id) => self.force_recheck(id),
            Message::RetryExtract(id) => self.retry_extract(id),
            Message::StopSeeding(id) => self.stop_seeding(id),
            Message::RequestPassword(id) => self.request_password(id),
            Message::PasswordChanged(value) => self.password_changed(value),
            Message::ConfirmPassword(id) => self.confirm_password(id),
            Message::CancelPassword => self.cancel_password(),
            Message::RemoveDownload(id) => self.remove_download(id),
            Message::TogglePin(id) => self.toggle_pin(id),
            Message::PauseAll => self.pause_all(),
            Message::ResumeAll => self.resume_all(),

            Message::RequestDeleteWithFiles(id) => self.delete_dialog_open(id),
            Message::ConfirmDeleteFiles => self.delete_dialog_confirm_files(),
            Message::ConfirmRemoveFromList => self.delete_dialog_confirm_remove(),
            Message::CancelDeleteWithFiles => self.delete_dialog_cancel(),

            Message::RequestEdit(id) => self.edit_dialog_open(id),
            Message::EditFilenameChanged(s) => self.edit_filename_changed(s),
            Message::EditSavePathChanged(s) => self.edit_save_path_changed(s),
            Message::EditPickSavePath => Self::edit_pick_save_path(),
            Message::EditPickSavePathResult(path) => self.edit_pick_save_path_result(path),
            Message::ConfirmEdit(id) => self.edit_dialog_confirm(id),
            Message::CancelEdit => self.edit_dialog_cancel(),
            Message::EditConflictChecked(result) => self.edit_conflict_checked(result),
            Message::EditRenameCompleted(result) => self.edit_rename_completed(result),

            Message::SelectClicked(id) => self.select_clicked(id),
            Message::SelectAll => self.select_all(),
            Message::TabSelected(tab) => self.tab_selected(tab),
            Message::SearchTextChanged(text) => self.search_text_changed(text),
            Message::SearchFocus => self.search_focus(),
            Message::SearchApplySuggestion(value) => self.search_apply_suggestion(&value),
            Message::SortColumn(col) => self.sort_column(col),
            Message::ColumnResizeStart(column) => self.column_resize_start(column),
            Message::ColumnResizeMove(x) => self.column_resize_move(x),
            Message::ColumnResizeEnd => self.column_resize_end(),

            Message::ProgressTick(list) => self.progress_tick(list),
            Message::DownloadsQueued(result) => self.downloads_queued(result),
            Message::EngineCommandDelivered { action, result } => {
                self.engine_command_delivered(action, result)
            },
            Message::Frame(now) => self.frame_tick(now),

            Message::OpenFile(id) => self.open_file(id),
            Message::OpenFolder(id) => self.open_folder(id),
            Message::CopyUrl(id) => self.copy_url(id),
            Message::OpenFileCompleted(result) => self.open_file_completed(result),
            Message::OpenFolderCompleted(result) => self.open_folder_completed(result),
            Message::CopyUrlCompleted(result) => self.copy_url_completed(result),
            Message::ClipboardUrl(url) => self.clipboard_url(url),
            Message::FileDropped(path) => self.file_dropped(path),
            Message::DroppedShortcutRead(url) => self.dropped_shortcut_read(url),

            Message::OpenSettings => self.open_settings(),
            Message::CloseSettings => self.close_settings(),
            Message::SettingsCategoryChanged(c) => self.settings_category_changed(c),
            Message::SettingsSearchChanged(text) => self.settings_search_changed(text),
            Message::SettingsSearchCleared => self.settings_search_cleared(),
            Message::SettingsSearchFocus => self.settings_search_focus(),
            Message::ToggleClipboard => self.toggle_clipboard(),
            Message::ToggleNotifications => self.toggle_notifications(),
            Message::DefaultCreateSubfolderToggled => self.default_create_subfolder_toggled(),
            Message::DefaultAutoExtractToggled => self.default_auto_extract_toggled(),
            Message::ExtractToSubfolderToggled => self.extract_to_subfolder_toggled(),
            Message::DeleteArchiveAfterExtractToggled => {
                self.delete_archive_after_extract_toggled()
            },
            Message::CloseToTrayToggled => self.close_to_tray_toggled(),
            Message::ScrollLongNamesToggled => self.scroll_long_names_toggled(),
            Message::ThemeChanged(id) => self.theme_changed(id),
            Message::ThemeMaterialChanged(material) => self.theme_material_changed(material),
            Message::SpeedLimitChanged(limit) => self.speed_limit_changed(limit),
            Message::MaxConcurrentChanged(max) => self.max_concurrent_changed(max),
            Message::DefaultSegmentsChanged(n) => self.default_segments_changed(n),
            Message::TorrentPortInputChanged(input) => self.torrent_port_input_changed(input),
            Message::TorrentDhtToggled(enabled) => self.torrent_dht_toggled(enabled),
            Message::TorrentUpnpToggled(enabled) => self.torrent_upnp_toggled(enabled),
            Message::TorrentSeedPolicyChoiceChanged(choice) => {
                self.torrent_seed_policy_choice_changed(choice)
            },
            Message::TorrentSeedRatioInputChanged(input) => {
                self.torrent_seed_ratio_input_changed(input)
            },
            Message::TorrentSeedDaysInputChanged(input) => {
                self.torrent_seed_days_input_changed(input)
            },
            Message::PickSaveFolder => self.pick_save_folder(),
            Message::SaveFolderPicked(path) => self.save_folder_picked(path),
            Message::SetUpFileAssociations => self.set_up_file_associations(),
            Message::FileAssociationsRegistered(result) => {
                self.file_associations_registered(result)
            },
            Message::OpenLogsFolder => self.open_logs_folder(),
            Message::OpenLogsFolderCompleted(result) => self.open_logs_folder_completed(result),
            Message::DismissNotice(id) => self.dismiss_notice(id),
            Message::ConfigPersisted { id, result } => self.config_persisted(id, result),
            Message::FirstRunBack => self.first_run_back(),
            Message::FirstRunNext => self.first_run_next(),
            Message::FirstRunSkip => self.first_run_skip(),

            Message::WindowOpened { id, size } => self.window_opened(id, size),
            Message::WindowFocused(focused) => self.window_focused(focused),
            Message::WindowResized(size) => self.window_resized(size),
            Message::WindowMinimize => self.window_minimize(),
            Message::WindowMaximizeToggle => self.window_maximize_toggle(),
            Message::WindowClose => self.window_close(),
            Message::WindowDragStart => self.window_drag_start(),
            Message::AppExit | Message::TrayQuit => self.app_exit(),
            Message::TrayShow => self.tray_show(),

            Message::DragDrop(id) => self.drag_drop(id),
            Message::DragZonesFound(id, zones) => self.drag_zones_found(id, &zones),
            Message::DragUpdate(id, point) => self.drag_update(id, point),

            Message::KeyPressed(key, modifiers) => self.key_pressed(&key, modifiers),
            Message::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers;
                Task::none()
            },
        }
    }
}
