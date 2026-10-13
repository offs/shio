use super::super::state::{Overlay, SettingsDraft, Shio, ToastKind};
use crate::message::{Message, NoticeId};
use crate::theme::ThemeId;
use iced::Task;
use parking_lot::Mutex;
use shio_core::{AppConfig, EngineCommand};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

// Saves run on blocking threads in any order; only the newest may reach the disk.
static SAVE_GATE: Mutex<()> = Mutex::new(());
static LATEST_SAVE: AtomicU64 = AtomicU64::new(0);

impl Shio {
    pub(super) fn open_settings(&mut self) -> Task<Message> {
        if matches!(self.overlay, Overlay::None) {
            self.menu = None;
            self.overlay = Overlay::Settings(Box::new(SettingsDraft::new(&self.config)));
        }
        Task::none()
    }

    pub(super) fn edit_draft(&mut self, edit: impl FnOnce(&mut SettingsDraft)) -> Task<Message> {
        if let Overlay::Settings(draft) = &mut self.overlay {
            edit(draft);
        }
        Task::none()
    }

    pub(super) fn commit_draft(&mut self, edit: impl FnOnce(&mut SettingsDraft)) -> Task<Message> {
        if let Overlay::Settings(draft) = &mut self.overlay {
            edit(draft);
        }
        self.commit_settings()
    }

    pub(super) fn commit_settings(&mut self) -> Task<Message> {
        let Overlay::Settings(draft) = &mut self.overlay else {
            return Task::none();
        };
        let next = draft.apply(&self.config);
        self.apply_config(next)
    }

    pub(super) fn change_config(&mut self, change: impl FnOnce(&mut AppConfig)) -> Task<Message> {
        let mut next = self.config.clone();
        change(&mut next);
        self.apply_config(next)
    }

    fn apply_config(&mut self, next: AppConfig) -> Task<Message> {
        if next == self.config {
            return Task::none();
        }
        let config = next.clone();
        let engine = self.command(|reply| EngineCommand::SetConfig { config, reply });
        self.config = next;
        Task::batch([engine, self.save_config()])
    }

    pub(super) fn save_config(&mut self) -> Task<Message> {
        self.config_save += 1;
        let id = self.config_save;
        LATEST_SAVE.store(id, Ordering::Release);
        let config = self.config.clone();
        Task::perform(
            async move {
                let result = tokio::task::spawn_blocking(move || {
                    save_if_latest(id, || config.save().map_err(|error| error.to_string()))
                })
                .await
                .map_err(|error| error.to_string())
                .and_then(std::convert::identity);
                (id, result)
            },
            |(id, result)| Message::ConfigSaved { id, result },
        )
    }

    pub(super) fn config_saved(&mut self, id: u64, result: Result<(), String>) -> Task<Message> {
        if id == self.config_save {
            match result {
                Ok(()) => self
                    .notices
                    .retain(|notice| notice.id != NoticeId::ConfigSave),
                Err(reason) => {
                    tracing::warn!("saving settings failed: {reason}");
                    self.set_notice(NoticeId::ConfigSave, "settings could not be saved", reason);
                },
            }
        }
        Task::none()
    }

    pub(super) fn download_folder_picked(&mut self, path: PathBuf) -> Task<Message> {
        if let Err(problem) = super::super::path_text(&path) {
            self.push_toast(ToastKind::Error, problem);
            return Task::none();
        }
        self.change_config(|config| config.download_dir = path)
    }

    pub(super) fn theme_changed(&mut self, id: ThemeId) -> Task<Message> {
        let task = self.change_config(|config| config.theme.id = id.into_string());
        self.resolve_theme();
        task
    }

    pub(super) fn blur_toggled(&mut self, enabled: bool) -> Task<Message> {
        let save = self.change_config(|config| config.window.blur = enabled);
        Task::batch([save, self.apply_blur()])
    }

    pub(super) fn software_rendering(&mut self) -> Task<Message> {
        tracing::warn!("no usable gpu; rendering with tiny-skia");
        self.software_rendering = true;
        self.blur_active = false;
        self.resolve_theme();
        self.push_toast(
            ToastKind::Info,
            "no usable gpu found — using software rendering",
        );
        self.apply_blur()
    }
}

fn save_if_latest(id: u64, save: impl FnOnce() -> Result<(), String>) -> Result<(), String> {
    let _gate = SAVE_GATE.lock();
    if LATEST_SAVE.load(Ordering::Acquire) == id {
        save()
    } else {
        Ok(())
    }
}

pub(super) fn change_associations(register: bool) -> Task<Message> {
    Task::perform(
        async move {
            tokio::task::spawn_blocking(move || {
                if register {
                    crate::platform::register_associations()
                        .map(|()| crate::platform::ASSOCIATION_SUCCESS_MESSAGE)
                } else {
                    crate::platform::unregister_associations().map(|()| "file associations removed")
                }
                .map_err(|error| error.to_string())
            })
            .await
            .map_err(|error| error.to_string())?
        },
        Message::AssociationsChanged,
    )
}

#[cfg(target_os = "linux")]
pub(super) fn follow_noctalia(dir: PathBuf) -> Task<Message> {
    Task::perform(
        async move {
            tokio::task::spawn_blocking(move || crate::platform::noctalia::follow(&dir))
                .await
                .map_err(|error| error.to_string())?
        },
        Message::NoctaliaFollowed,
    )
}

#[cfg(not(target_os = "linux"))]
pub(super) fn follow_noctalia(_: PathBuf) -> Task<Message> {
    Task::none()
}

pub(super) fn open_logs_folder() -> Task<Message> {
    Task::perform(
        async {
            tokio::task::spawn_blocking(|| {
                crate::diagnostics::open_logs_folder().map_err(|error| error.to_string())
            })
            .await
            .map_err(|error| error.to_string())?
        },
        Message::Opened,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{SeedPolicyChoice, SpeedUnit};

    #[test]
    fn only_the_latest_config_save_writes() {
        LATEST_SAVE.store(2, Ordering::Release);
        let mut writes = Vec::new();
        for id in [1, 2] {
            save_if_latest(id, || {
                writes.push(id);
                Ok(())
            })
            .expect("save");
        }
        assert_eq!(writes, [2]);
    }

    #[test]
    fn settings_draft_commits_valid_values_and_keeps_the_rest() {
        let config = AppConfig::default();
        let mut draft = SettingsDraft::new(&config);
        draft.speed_limit_on = true;
        draft.speed_limit = "20".to_owned();
        draft.speed_unit = SpeedUnit::MiB;
        draft.port = "0".to_owned();
        draft.seed = SeedPolicyChoice::StopAutomatically;
        draft.ratio = "2".to_owned();
        draft.days = "3651".to_owned();

        let next = draft.apply(&config);

        assert_eq!(next.speed_limit, Some(20 * 1024 * 1024));
        assert_eq!(next.torrent.listen_port, config.torrent.listen_port);
        assert_eq!(next.torrent.seed_policy, config.torrent.seed_policy);
        assert!(draft.port_error.is_some() && draft.days_error.is_some());
        assert!(draft.speed_limit_error.is_none() && draft.ratio_error.is_none());

        draft.speed_limit = "99999999999999".to_owned();
        assert_eq!(draft.apply(&config).speed_limit, config.speed_limit);
        assert!(draft.speed_limit_error.is_some());
    }
}
