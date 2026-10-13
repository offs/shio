mod add;
mod engine;
mod keyboard;
mod list;
mod settings;
mod window;

use super::state::{Menu, Overlay, SearchFocus, Shio, ToastKind};
use crate::message::Message;
use iced::Task;
use shio_core::EngineCommand;
use tokio::sync::oneshot;

pub(super) use window::WINDOW_SIZE;

const ENGINE_STOPPED: &str = "the download engine stopped";

impl Shio {
    pub(crate) fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Engine(events) => self.apply_engine_events(events),
            Message::EngineReply(result) => {
                if let Err(reason) = result {
                    self.push_toast(ToastKind::Error, reason);
                }
                Task::none()
            },
            Message::EngineStopped(result) => self.engine_stopped(result),

            Message::SearchChanged(text) => self.set_search(text),
            Message::SearchFocused(focused) => {
                self.search_focus = focused.then(SearchFocus::default);
                Task::none()
            },
            Message::HighlightSuggestion { down } => self.highlight_suggestion(down),
            Message::AcceptSuggestion => self.accept_suggestion(),
            Message::ApplySuggestion(value) => self.apply_suggestion(value),
            Message::TabSelected(tab) => {
                self.tab = tab;
                self.refresh();
                Task::none()
            },
            Message::ToggleSortMenu => {
                self.menu = match self.menu {
                    Some(Menu::Sort) => None,
                    _ => Some(Menu::Sort),
                };
                Task::none()
            },
            Message::SortBy(key) => self.sort_by(key),
            Message::RowPressed(id) => self.row_pressed(id),
            Message::RowDoubleClicked(id) => self.row_double_clicked(id),
            Message::OpenRowMenu(id, at) => self.open_row_menu(id, at),
            Message::CloseMenu => {
                self.menu = None;
                Task::none()
            },

            Message::Pause(id) => self.pause(id),
            Message::Resume(id) => self.resume(id),
            Message::Retry(id) => self.retry(id),
            Message::RetryExtract(id) => self.retry_extract(id),
            Message::RequestPassword(id) => self.request_password(id),
            Message::ForceRecheck(id) => self.force_recheck(id),
            Message::StopSeeding(id) => self.stop_seeding(id),
            Message::TogglePin(id) => self.toggle_pin(id),
            Message::OpenFile(id) => self.open_file(id),
            Message::OpenFolder(id) => self.open_folder(id),
            Message::CopyUrl(id) => self.copy_url(id),
            Message::RequestRemove(id) => self.request_remove(id),
            Message::RequestCancel(id) => self.request_cancel(id),
            Message::PauseAll => self.command(|reply| EngineCommand::PauseAll { reply }),
            Message::ResumeAll => self.command(|reply| EngineCommand::ResumeAll { reply }),

            Message::CloseOverlay => self.close_overlay(),
            Message::ConfirmRemove { delete_files } => self.confirm_remove(delete_files),
            Message::ConfirmCancel => self.confirm_cancel(),
            Message::PasswordChanged(value) => {
                if let Overlay::Password { input, .. } = &mut self.overlay {
                    *input = super::truncate_text_bytes(value, super::MAX_PASSWORD_TEXT_BYTES);
                }
                Task::none()
            },
            Message::SubmitPassword => self.submit_password(),

            Message::OpenAdd => self.open_add(""),
            Message::AddUrlsAction(action) => self.add_urls_action(action),
            Message::AddSubmit => self.add_submit(),
            Message::AddSubmitted(results) => self.add_submitted(results),
            Message::PickTorrentFiles => add::pick_torrent_files(),
            Message::TorrentFilesPicked(paths) => self.queue_sources("", paths),
            Message::TorrentFilesRead(picks) => self.torrent_files_read(picks),
            Message::AddPickFolder => add::pick_folder(Message::AddFolderPicked),
            Message::AddFolderPicked(path) => self.add_folder_picked(&path),
            Message::AddSavePathChanged(path) => self.edit_add(|dialog| {
                dialog.save_path = super::truncate_text_bytes(path, super::MAX_PATH_TEXT_BYTES);
                dialog.revalidate();
            }),
            Message::AddFilenameChanged(name) => self.edit_add(|dialog| {
                dialog.filename = super::truncate_text_bytes(name, super::MAX_NAME_TEXT_BYTES);
            }),
            Message::AddSubfolderToggled(enabled) => self.edit_add(|dialog| {
                dialog.create_subfolder = enabled;
                dialog.revalidate();
            }),
            Message::AddSubfolderNameChanged(name) => {
                self.edit_add(|dialog| dialog.set_subfolder(name))
            },
            Message::AddAutoExtractToggled(enabled) => self.edit_add(|dialog| {
                dialog.auto_extract = enabled;
                dialog.revalidate();
            }),
            Message::AddTorrentSelected(id) => {
                self.edit_add(|dialog| dialog.selected_torrent = Some(id))
            },
            Message::AddTorrentFileToggled {
                torrent,
                file,
                selected,
            } => self.edit_add(|dialog| dialog.set_torrent_files(torrent, Some(file), selected)),
            Message::AddTorrentFilesToggled { torrent, selected } => {
                self.edit_add(|dialog| dialog.set_torrent_files(torrent, None, selected))
            },
            Message::AddTorrentSearchChanged(search) => self.edit_add(|dialog| {
                dialog.torrent_search =
                    super::truncate_text_bytes(search, super::MAX_SEARCH_TEXT_BYTES);
            }),
            Message::MagnetPreviewed(result) => {
                self.edit_add(|dialog| dialog.apply_magnet_preview(result))
            },
            Message::HttpPreviewed(result) => {
                self.edit_add(|dialog| dialog.apply_http_preview(result))
            },
            Message::ClipboardBaseline(text) => {
                self.note_clipboard(text.as_deref());
                Task::none()
            },
            Message::ClipboardRead(text) => {
                self.clipboard_read(text);
                Task::none()
            },
            Message::AddFromClipboard => match self.clipboard_offer.take() {
                Some(offer) => self.open_add(&offer.links),
                None => Task::none(),
            },
            Message::DismissClipboardOffer => {
                self.clipboard_offer = None;
                Task::none()
            },
            Message::Pasted(text) => {
                self.note_clipboard(text.as_deref());
                self.open_add(text.as_deref().unwrap_or_default())
            },
            Message::FileDropped(path) => self.file_dropped(path),
            Message::ShortcutRead(url) => url.map_or_else(Task::none, |url| self.open_add(&url)),

            Message::OpenSettings => self.open_settings(),
            Message::SettingsSection(section) => {
                if let Overlay::Settings(draft) = &mut self.overlay {
                    draft.section = section;
                }
                Task::none()
            },
            Message::ToggleNotifications => self.change_config(|config| {
                config.notifications = !config.notifications;
            }),
            Message::ToggleCloseToTray => self.change_config(|config| {
                config.close_to_tray = !config.close_to_tray;
            }),
            Message::ToggleClipboard => {
                self.clipboard_seen = None;
                self.clipboard_offer = None;
                self.change_config(|config| config.clipboard_monitor = !config.clipboard_monitor)
            },
            Message::DefaultCreateSubfolderToggled => self.change_config(|config| {
                config.default_create_subfolder = !config.default_create_subfolder;
            }),
            Message::DefaultAutoExtractToggled => self.change_config(|config| {
                config.default_auto_extract = !config.default_auto_extract;
            }),
            Message::DeleteArchiveAfterExtractToggled => self.change_config(|config| {
                config.delete_archive_after_extract = !config.delete_archive_after_extract;
            }),
            Message::PickDownloadFolder => add::pick_folder(Message::DownloadFolderPicked),
            Message::DownloadFolderPicked(path) => self.download_folder_picked(path),
            Message::MaxConcurrentChanged(value) => self.edit_draft(|draft| {
                draft.max_concurrent = value;
            }),
            Message::SegmentsChanged(value) => self.edit_draft(|draft| draft.segments = value),
            Message::SpeedLimitToggled(enabled) => {
                self.commit_draft(|draft| draft.speed_limit_on = enabled)
            },
            Message::SpeedLimitInput(text) => self.edit_draft(|draft| {
                draft.speed_limit = super::truncate_text_bytes(text, super::MAX_NUMERIC_TEXT_BYTES);
            }),
            Message::SpeedLimitUnit(unit) => self.commit_draft(|draft| draft.speed_unit = unit),
            Message::TorrentPortInput(text) => self.edit_draft(|draft| {
                draft.port = super::truncate_text_bytes(text, super::MAX_NUMERIC_TEXT_BYTES);
            }),
            Message::TorrentDhtToggled(enabled) => {
                self.change_config(|config| config.torrent.dht = enabled)
            },
            Message::TorrentUpnpToggled(enabled) => {
                self.change_config(|config| config.torrent.upnp = enabled)
            },
            Message::SeedPolicyChanged(choice) => self.commit_draft(|draft| draft.seed = choice),
            Message::SeedRatioInput(text) => self.edit_draft(|draft| {
                draft.ratio = super::truncate_text_bytes(text, super::MAX_NUMERIC_TEXT_BYTES);
            }),
            Message::SeedDaysInput(text) => self.edit_draft(|draft| {
                draft.days = super::truncate_text_bytes(text, super::MAX_NUMERIC_TEXT_BYTES);
            }),
            Message::SettingsCommit => self.commit_settings(),
            Message::ThemeChanged(id) => self.theme_changed(id),
            Message::BlurToggled(enabled) => self.blur_toggled(enabled),
            Message::BlurActive(active) => {
                self.blur_active = active && !self.software_rendering;
                self.resolve_theme();
                Task::none()
            },
            Message::SoftwareRendering => self.software_rendering(),
            Message::FollowNoctalia => self
                .noctalia
                .clone()
                .map_or_else(Task::none, settings::follow_noctalia),
            #[cfg(target_os = "linux")]
            Message::NoctaliaFollowed(result) => match result {
                Ok((catalog, id)) => {
                    self.theme_catalog = catalog;
                    self.theme_changed(id)
                },
                Err(reason) => {
                    self.push_toast(ToastKind::Error, reason);
                    Task::none()
                },
            },
            Message::RegisterAssociations => settings::change_associations(true),
            Message::UnregisterAssociations => settings::change_associations(false),
            Message::AssociationsChanged(result) => {
                match result {
                    Ok(message) => self.push_toast(ToastKind::Success, message),
                    Err(reason) => self.push_toast(ToastKind::Error, reason),
                }
                Task::none()
            },
            Message::OpenLogsFolder => settings::open_logs_folder(),
            Message::OpenUrl(url) => window::open_path(url),
            Message::OpenStartupLocation => self
                .startup_problem
                .as_ref()
                .map_or_else(Task::none, |problem| {
                    window::open_path(problem.location.clone())
                }),
            Message::Opened(result) => {
                if let Err(reason) = result {
                    tracing::warn!("open failed: {reason}");
                    self.push_toast(ToastKind::Error, "could not open it");
                }
                Task::none()
            },
            Message::ConfigSaved { id, result } => self.config_saved(id, result),
            Message::DismissNotice(id) => {
                self.notices.retain(|notice| notice.id != id);
                Task::none()
            },
            Message::ToastTick => {
                if let Some(toast) = &mut self.toast {
                    toast.seconds_left = toast.seconds_left.saturating_sub(1);
                    if toast.seconds_left == 0 {
                        self.toast = None;
                    }
                }
                Task::none()
            },
            Message::ThemesChanged(catalog) => {
                // A half-written theme file must not reach `resolve_theme`, which would
                // replace the configured id with the fallback; its next write reloads.
                if catalog.find(&self.theme.id).is_some() {
                    self.theme_catalog = catalog;
                    self.resolve_theme();
                }
                Task::none()
            },
            Message::FirstRunBack => {
                if let Overlay::Welcome(step) = &mut self.overlay {
                    *step = step.previous().unwrap_or(*step);
                }
                Task::none()
            },
            Message::FirstRunNext => match &mut self.overlay {
                Overlay::Welcome(step) => match step.next() {
                    Some(next) => {
                        *step = next;
                        Task::none()
                    },
                    None => self.close_overlay(),
                },
                _ => Task::none(),
            },

            Message::KeyPressed {
                key,
                modifiers,
                captured,
            } => self.key_pressed(&key, modifiers, captured),
            Message::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers;
                Task::none()
            },

            Message::WindowOpened => self.window_opened(),
            Message::WindowFocused(focused) => self.window_focused(focused),
            Message::WindowResized(size) => self.window_resized(size),
            Message::Maximized(maximized) => {
                self.window.maximized = maximized;
                Task::none()
            },
            Message::WindowDrag => self.with_window(iced::window::drag),
            Message::WindowResize(direction) => {
                self.with_window(move |id| iced::window::drag_resize(id, direction))
            },
            Message::WindowMinimize => self.with_window(|id| iced::window::minimize(id, true)),
            Message::WindowMaximizeToggle => self.with_window(|id| {
                iced::window::toggle_maximize(id)
                    .chain(iced::window::is_maximized(id).map(Message::Maximized))
            }),
            Message::WindowClose => self.window_close(),
            Message::WindowClosed(id) => self.window_closed(id),
            Message::TrayShow => self.show_window(None),
            Message::InstanceActivated(activation) => Task::batch([
                self.show_window(activation.token),
                self.open_sources(activation.launch),
            ]),
            Message::Quit => self.quit(),
            Message::ShutdownComplete => {
                crate::tray::shutdown();
                iced::exit()
            },
        }
    }

    // try_send first keeps commands in call order; outcomes arrive as engine events.
    fn command(
        &self,
        build: impl FnOnce(oneshot::Sender<shio_core::Result<()>>) -> EngineCommand,
    ) -> Task<Message> {
        let (reply, outcome) = oneshot::channel();
        let unsent = self
            .engine
            .try_send(build(reply))
            .err()
            .map(tokio::sync::mpsc::error::TrySendError::into_inner);
        let engine = self.engine.clone();
        Task::perform(
            async move {
                if let Some(command) = unsent {
                    engine
                        .send(command)
                        .await
                        .map_err(|_| ENGINE_STOPPED.to_owned())?;
                }
                outcome
                    .await
                    .map_err(|_| ENGINE_STOPPED.to_owned())?
                    .map_err(|error| error.short_label())
            },
            Message::EngineReply,
        )
    }
}
