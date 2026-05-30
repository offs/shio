#![expect(
    clippy::unused_self,
    reason = "update handlers stay as Shio methods for dispatch consistency"
)]

use super::super::state::{Overlay, Shio};
use super::SETTINGS_SEARCH_INPUT_ID;
use crate::message::{Message, SeedPolicyChoice, SettingsCategory};
use crate::theme::{ThemeId, ThemeSelection};
use iced::Task;
use shio_core::{EngineCommand, WindowMaterialPreference};
use std::path::PathBuf;

const SECONDS_PER_DAY: u64 = 86_400;
const MAX_SEED_DAYS: u64 = 3_650;

impl Shio {
    pub(super) fn open_settings(&mut self) -> Task<Message> {
        self.overlay = Overlay::Settings;
        self.settings_search_focus()
    }

    pub(super) fn close_settings(&mut self) -> Task<Message> {
        self.overlay = Overlay::None;
        self.persist_config()
    }

    pub(super) fn settings_category_changed(
        &mut self,
        category: SettingsCategory,
    ) -> Task<Message> {
        self.settings_category = category;
        self.settings_search.clear();
        Task::none()
    }

    pub(super) fn settings_search_changed(&mut self, text: String) -> Task<Message> {
        self.settings_search = text;
        Task::none()
    }

    pub(super) fn settings_search_cleared(&mut self) -> Task<Message> {
        self.settings_search.clear();
        self.settings_search_focus()
    }

    pub(super) fn settings_search_focus(&self) -> Task<Message> {
        iced::widget::operation::focus(SETTINGS_SEARCH_INPUT_ID.clone())
    }

    pub(super) fn toggle_clipboard(&mut self) -> Task<Message> {
        self.config.clipboard_monitor = !self.config.clipboard_monitor;
        self.persist_config()
    }

    pub(super) fn toggle_notifications(&mut self) -> Task<Message> {
        self.config.notifications = !self.config.notifications;
        self.persist_config()
    }

    pub(super) fn default_create_subfolder_toggled(&mut self) -> Task<Message> {
        self.config.default_create_subfolder = !self.config.default_create_subfolder;
        self.persist_config()
    }

    pub(super) fn default_auto_extract_toggled(&mut self) -> Task<Message> {
        self.config.default_auto_extract = !self.config.default_auto_extract;
        self.persist_config()
    }

    pub(super) fn extract_to_subfolder_toggled(&mut self) -> Task<Message> {
        self.config.extract_to_subfolder = !self.config.extract_to_subfolder;
        self.persist_config()
    }

    pub(super) fn delete_archive_after_extract_toggled(&mut self) -> Task<Message> {
        self.config.delete_archive_after_extract = !self.config.delete_archive_after_extract;
        self.persist_config()
    }

    pub(super) fn close_to_tray_toggled(&mut self) -> Task<Message> {
        self.config.close_to_tray = !self.config.close_to_tray;
        self.persist_config()
    }

    pub(super) fn scroll_long_names_toggled(&mut self) -> Task<Message> {
        self.config.scroll_long_names = !self.config.scroll_long_names;
        self.persist_config()
    }

    pub(super) fn theme_changed(&mut self, id: ThemeId) -> Task<Message> {
        self.config.theme.id = id.into_string();
        self.refresh_theme()
    }

    pub(super) fn theme_material_changed(
        &mut self,
        material: WindowMaterialPreference,
    ) -> Task<Message> {
        self.config.window.material = material;
        Task::batch([
            self.persist_config(),
            iced::window::latest()
                .and_then(move |id| super::super::vibrancy::apply_vibrancy(id, material)),
        ])
    }

    fn refresh_theme(&mut self) -> Task<Message> {
        let selection = ThemeSelection::from_config(&self.config.theme);
        self.theme = self.theme_catalog.resolve(&selection);
        if self.theme.used_fallback {
            self.config.theme.id = self.theme.id.to_string();
        }
        tracing::debug!("theme resolved: {}", self.theme.id);
        let material = self.config.window.material;
        Task::batch([
            self.persist_config(),
            iced::window::latest()
                .and_then(move |id| super::super::vibrancy::apply_vibrancy(id, material)),
        ])
    }

    pub(super) fn speed_limit_changed(&mut self, limit: Option<u64>) -> Task<Message> {
        self.config.speed_limit = limit;
        Task::batch([
            self.persist_config(),
            self.send_engine_cmd_unacked(EngineCommand::SetSpeedLimit(limit)),
        ])
    }

    pub(super) fn max_concurrent_changed(&mut self, max: u8) -> Task<Message> {
        self.config.max_concurrent = max;
        Task::batch([
            self.persist_config(),
            self.send_engine_cmd_unacked(EngineCommand::SetMaxConcurrent(max)),
        ])
    }

    pub(super) fn default_segments_changed(&mut self, n: u8) -> Task<Message> {
        self.config.default_segments = n;
        self.persist_config()
    }

    pub(super) fn torrent_port_input_changed(&mut self, input: String) -> Task<Message> {
        self.torrent_port_input = input;
        let parsed = parse_torrent_port(&self.torrent_port_input);
        let Ok(port) = parsed else {
            self.torrent_port_error = Some("Enter a port from 1 to 65535".to_string());
            return Task::none();
        };
        self.config.torrent.listen_port = port;
        self.torrent_port_error = None;
        self.persist_torrent_config()
    }

    pub(super) fn torrent_dht_toggled(&mut self, enabled: bool) -> Task<Message> {
        self.config.torrent.dht = enabled;
        Task::batch([
            self.persist_config(),
            self.send_engine_cmd_unacked(EngineCommand::SetTorrentConfig(
                self.config.torrent.clone(),
            )),
        ])
    }

    pub(super) fn torrent_upnp_toggled(&mut self, enabled: bool) -> Task<Message> {
        self.config.torrent.upnp = enabled;
        Task::batch([
            self.persist_config(),
            self.send_engine_cmd_unacked(EngineCommand::SetTorrentConfig(
                self.config.torrent.clone(),
            )),
        ])
    }

    pub(super) fn torrent_seed_policy_choice_changed(
        &mut self,
        choice: SeedPolicyChoice,
    ) -> Task<Message> {
        match choice {
            SeedPolicyChoice::StopAutomatically => self.save_automatic_seed_policy(),
            SeedPolicyChoice::SeedForever => {
                self.torrent_ratio_error = None;
                self.torrent_seed_days_error = None;
                self.config.torrent.seed_policy = shio_core::SeedPolicy::SeedForever;
                self.persist_torrent_config()
            },
            SeedPolicyChoice::NeverSeed => {
                self.torrent_ratio_error = None;
                self.torrent_seed_days_error = None;
                self.config.torrent.seed_policy = shio_core::SeedPolicy::NeverSeed;
                self.persist_torrent_config()
            },
        }
    }

    pub(super) fn torrent_seed_ratio_input_changed(&mut self, input: String) -> Task<Message> {
        self.torrent_ratio_input = input;
        self.save_automatic_seed_policy()
    }

    pub(super) fn torrent_seed_days_input_changed(&mut self, input: String) -> Task<Message> {
        self.torrent_seed_days_input = input;
        self.save_automatic_seed_policy()
    }

    fn save_automatic_seed_policy(&mut self) -> Task<Message> {
        let ratio = match parse_seed_ratio(&self.torrent_ratio_input) {
            Ok(ratio) => {
                self.torrent_ratio_error = None;
                ratio
            },
            Err(message) => {
                self.torrent_ratio_error = Some(message.to_string());
                return Task::none();
            },
        };
        let days = match parse_seed_days(&self.torrent_seed_days_input) {
            Ok(days) => {
                self.torrent_seed_days_error = None;
                days
            },
            Err(message) => {
                self.torrent_seed_days_error = Some(message.to_string());
                return Task::none();
            },
        };

        self.config.torrent.seed_policy = shio_core::SeedPolicy::RatioOrTime {
            ratio,
            seconds: days.saturating_mul(SECONDS_PER_DAY),
        };
        self.persist_torrent_config()
    }

    fn persist_torrent_config(&mut self) -> Task<Message> {
        Task::batch([
            self.persist_config(),
            self.send_engine_cmd_unacked(EngineCommand::SetTorrentConfig(
                self.config.torrent.clone(),
            )),
        ])
    }

    pub(super) fn pick_save_folder(&self) -> Task<Message> {
        Task::perform(
            pick_folder("choose download folder"),
            Message::SaveFolderPicked,
        )
    }

    pub(super) fn save_folder_picked(&mut self, path: Option<PathBuf>) -> Task<Message> {
        let Some(path) = path else {
            return Task::none();
        };
        if self.show_first_run() {
            self.config.download_dir = path;
            return self.persist_config();
        } else if self.show_add_dialog() {
            self.add_save_path = path.to_string_lossy().to_string();
        } else {
            self.config.download_dir = path;
            return self.persist_config();
        }
        Task::none()
    }

    pub(super) fn open_logs_folder(&self) -> Task<Message> {
        Task::perform(
            async {
                tokio::task::spawn_blocking(crate::diagnostics::open_logs_folder)
                    .await
                    .map_err(|e| e.to_string())?
            },
            Message::OpenLogsFolderCompleted,
        )
    }

    pub(super) fn open_logs_folder_completed(
        &mut self,
        result: Result<(), String>,
    ) -> Task<Message> {
        if let Err(message) = result {
            tracing::warn!("open logs folder failed: {message}");
            self.push_toast("open logs failed", super::super::state::ToastKind::Error);
        }
        Task::none()
    }

    pub(super) fn set_up_file_associations(&self) -> Task<Message> {
        Task::perform(
            async {
                tokio::task::spawn_blocking(|| {
                    crate::platform::complete_download_association_setup()
                        .map_err(|e| e.to_string())
                })
                .await
                .map_err(|e| e.to_string())?
            },
            Message::FileAssociationsRegistered,
        )
    }

    pub(super) fn file_associations_registered(
        &mut self,
        result: Result<(), String>,
    ) -> Task<Message> {
        match result {
            Ok(()) => self.push_toast(
                crate::platform::successful_association_message(),
                super::super::state::ToastKind::Info,
            ),
            Err(message) => self.push_toast(&message, super::super::state::ToastKind::Error),
        }
        Task::none()
    }

    pub(super) fn close_first_run(&mut self) -> Task<Message> {
        self.overlay = Overlay::None;
        self.persist_config()
    }

    pub(super) fn first_run_back(&mut self) -> Task<Message> {
        if let Some(step) = self.first_run_step.previous() {
            self.first_run_step = step;
        }
        Task::none()
    }

    pub(super) fn first_run_next(&mut self) -> Task<Message> {
        if let Some(step) = self.first_run_step.next() {
            self.first_run_step = step;
            return Task::none();
        }
        self.close_first_run()
    }

    pub(super) fn first_run_skip(&mut self) -> Task<Message> {
        self.close_first_run()
    }
}

fn parse_torrent_port(input: &str) -> Result<u16, &'static str> {
    let port = input
        .trim()
        .parse::<u16>()
        .map_err(|_| "Enter a port from 1 to 65535")?;
    if port == 0 {
        return Err("Enter a port from 1 to 65535");
    }
    Ok(port)
}

fn parse_seed_ratio(input: &str) -> Result<f32, &'static str> {
    let ratio = input
        .trim()
        .parse::<f32>()
        .map_err(|_| "Enter a number such as 1.00")?;
    if ratio.is_finite() && ratio >= 0.0 {
        return Ok(ratio);
    }
    Err("Ratio must be 0 or higher")
}

fn parse_seed_days(input: &str) -> Result<u64, &'static str> {
    let days = input
        .trim()
        .parse::<u64>()
        .map_err(|_| "Enter whole days")?;
    if days <= MAX_SEED_DAYS {
        return Ok(days);
    }
    Err("Use 3650 days or less")
}

async fn pick_folder(title: &'static str) -> Option<PathBuf> {
    rfd::AsyncFileDialog::new()
        .set_title(title)
        .pick_folder()
        .await
        .map(|h| h.path().to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn torrent_port_parser_accepts_valid_port() {
        assert_eq!(parse_torrent_port("6881"), Ok(6881));
    }

    #[test]
    fn torrent_port_parser_rejects_zero_and_out_of_range() {
        assert!(parse_torrent_port("0").is_err());
        assert!(parse_torrent_port("70000").is_err());
    }

    #[test]
    fn seed_ratio_parser_rejects_negative_and_non_finite() {
        assert!(parse_seed_ratio("-1").is_err());
        assert!(parse_seed_ratio("NaN").is_err());
        assert_eq!(parse_seed_ratio("1.5"), Ok(1.5));
    }

    #[test]
    fn seed_days_parser_accepts_whole_day_limit() {
        assert_eq!(parse_seed_days("7"), Ok(7));
        assert!(parse_seed_days("3651").is_err());
    }
}
