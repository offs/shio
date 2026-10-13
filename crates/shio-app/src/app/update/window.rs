use super::super::blur;
use super::super::state::{Overlay, Shio, StartupProblem};
use super::super::subscription::install_engine_events;
use crate::message::{FirstRunStep, Message};
use crate::platform::AssociationLaunch;
use iced::{Size, Task, window};
use shio_core::{AppConfig, DownloadEngine, EngineCommand};
use std::path::PathBuf;
use std::time::Duration;

pub(in crate::app) const WINDOW_SIZE: Size = Size::new(960.0, 640.0);
const MIN_WINDOW_SIZE: Size = Size::new(720.0, 460.0);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(20);

impl Shio {
    pub(crate) fn new(launch: AssociationLaunch) -> (Self, Task<Message>) {
        let first_run = !AppConfig::config_exists();
        let loaded = if first_run {
            Ok(AppConfig::default())
        } else {
            AppConfig::load()
        };
        let (config, started) = match loaded {
            Ok(config) => {
                let data_dir = AppConfig::data_dir();
                let started = DownloadEngine::new(config.clone(), &data_dir).map_err(|error| {
                    StartupProblem {
                        title: "the download database could not be opened",
                        message: error.to_string(),
                        location: data_dir,
                    }
                });
                (config, started)
            },
            Err(error) => (
                AppConfig::default(),
                Err(StartupProblem {
                    title: "the settings file could not be loaded",
                    message: error.to_string(),
                    location: AppConfig::config_dir(),
                }),
            ),
        };

        let (engine, engine_task, startup_problem) = match started {
            Ok((engine, commands, events)) => {
                install_engine_events(events);
                let run = Task::perform(
                    async {
                        tokio::spawn(engine.run())
                            .await
                            .map_err(|error| error.to_string())
                    },
                    Message::EngineStopped,
                );
                (commands, run, None)
            },
            Err(problem) => {
                tracing::error!("{}: {}", problem.title, problem.message);
                crate::instance::reject_activations();
                (tokio::sync::mpsc::channel(1).0, Task::none(), Some(problem))
            },
        };

        let mut app = Self::build(config, engine);
        app.startup_problem = startup_problem;
        app.resolve_theme();
        if first_run {
            app.overlay = Overlay::Welcome(FirstRunStep::Look);
        }
        let open = app.open_window();
        let sources = app.open_sources(launch);
        (app, Task::batch([engine_task, open, sources]))
    }

    fn open_window(&mut self) -> Task<Message> {
        self.window.blur = self.wants_blur();
        let (id, open) = window::open(window_settings(self.window.blur));
        self.window.id = Some(id);
        self.window.maximized = false;
        open.map(|_| Message::WindowOpened)
    }

    pub(super) fn with_window(
        &self,
        task: impl FnOnce(window::Id) -> Task<Message>,
    ) -> Task<Message> {
        self.window.id.map_or_else(Task::none, task)
    }

    pub(super) fn window_opened(&mut self) -> Task<Message> {
        self.window.focused = true;
        crate::tray::activate();
        self.apply_blur()
    }

    pub(super) fn apply_blur(&self) -> Task<Message> {
        let (on, created) = (self.wants_blur(), self.window.blur);
        self.with_window(|id| blur::apply(id, on, created).map(Message::BlurActive))
    }

    // Only clipboard changes made while shio was unfocused were copied elsewhere.
    pub(super) fn window_focused(&mut self, focused: bool) -> Task<Message> {
        self.window.focused = focused;
        if !self.config.clipboard_monitor {
            return Task::none();
        }
        iced::clipboard::read().map(if focused {
            Message::ClipboardRead
        } else {
            Message::ClipboardBaseline
        })
    }

    pub(super) fn window_resized(&mut self, size: Size) -> Task<Message> {
        self.window.size = size;
        self.with_window(|id| window::is_maximized(id).map(Message::Maximized))
    }

    // Closing to the tray destroys the window because Wayland cannot hide one.
    pub(super) fn window_close(&mut self) -> Task<Message> {
        if self.config.close_to_tray && crate::tray::is_ready() && self.startup_problem.is_none() {
            self.menu = None;
            self.with_window(window::close)
        } else {
            self.quit()
        }
    }

    pub(super) fn window_closed(&mut self, id: window::Id) -> Task<Message> {
        if self.window.id == Some(id) {
            self.window.id = None;
            self.window.focused = false;
        }
        Task::none()
    }

    pub(super) fn show_window(&mut self, token: Option<String>) -> Task<Message> {
        if self.shutting_down {
            return Task::none();
        }
        let shown = match self.window.id {
            Some(id) => window::minimize(id, false).chain(window::gain_focus(id)),
            None => self.open_window(),
        };
        match (self.window.id, token) {
            (Some(id), Some(token)) => shown.chain(activate(id, token)),
            _ => shown,
        }
    }

    pub(super) fn quit(&mut self) -> Task<Message> {
        if self.shutting_down {
            return Task::none();
        }
        self.shutting_down = true;
        crate::instance::reject_activations();
        let engine = self.engine.clone();
        Task::perform(
            async move {
                let (reply, done) = tokio::sync::oneshot::channel();
                if engine.send(EngineCommand::Shutdown { reply }).await.is_ok()
                    && tokio::time::timeout(SHUTDOWN_TIMEOUT, done).await.is_err()
                {
                    tracing::error!("the engine did not stop within {SHUTDOWN_TIMEOUT:?}; exiting");
                }
            },
            |()| Message::ShutdownComplete,
        )
    }
}

fn window_settings(blur: bool) -> window::Settings {
    let mut settings = window::Settings {
        size: WINDOW_SIZE,
        min_size: Some(MIN_WINDOW_SIZE),
        icon: crate::platform::window_icon(),
        exit_on_close_request: false,
        transparent: cfg!(not(windows)),
        blur,
        decorations: cfg!(target_os = "macos") || runs_on_niri(),
        ..window::Settings::default()
    };
    #[cfg(target_os = "linux")]
    {
        settings.platform_specific.application_id = crate::platform::APP_ID.into();
    }
    #[cfg(target_os = "macos")]
    {
        settings.platform_specific.title_hidden = true;
        settings.platform_specific.titlebar_transparent = true;
        settings.platform_specific.fullsize_content_view = true;
    }
    #[cfg(windows)]
    {
        settings.platform_specific.corner_preference =
            window::settings::platform::CornerPreference::Round;
    }
    settings
}

// niri paints a solid backdrop behind client-decorated windows that aren't tiled, hiding the
// blur; it never draws titlebars, so asking it for server-side decorations only drops that fill.
fn runs_on_niri() -> bool {
    cfg!(target_os = "linux") && std::env::var_os("NIRI_SOCKET").is_some()
}

pub(super) fn open_path(path: impl Into<PathBuf>) -> Task<Message> {
    let path = path.into();
    Task::perform(
        async move {
            tokio::task::spawn_blocking(move || {
                open::that(&path).map_err(|error| error.to_string())
            })
            .await
            .map_err(|error| error.to_string())?
        },
        Message::Opened,
    )
}

#[cfg(target_os = "linux")]
fn activate(id: window::Id, token: String) -> Task<Message> {
    window::run(id, move |w| super::super::wayland::activate(w, token)).discard()
}

#[cfg(not(target_os = "linux"))]
fn activate(_: window::Id, _: String) -> Task<Message> {
    Task::none()
}
