#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod diagnostics;
mod instance;
mod link;
mod message;
mod platform;
mod search;
mod style;
mod theme;
mod tray;
mod views;
mod widgets;

use iced::Font;

const INTER: Font = Font::with_name("Inter");
const INTER_BYTES: &[u8] = include_bytes!("../../../assets/fonts/Inter-Variable.ttf");

fn main() -> iced::Result {
    prefer_integrated_gpu();
    if std::env::args_os().any(|arg| arg == platform::UNREGISTER_FLAG) {
        if let Err(error) = platform::unregister_associations() {
            eprintln!("could not remove file associations: {error}");
            std::process::exit(1);
        }
        return Ok(());
    }

    let launch = platform::association_launch_from_args(std::env::args_os());
    let _instance = match instance::InstanceGuard::acquire(&launch) {
        Ok(guard) => guard,
        Err(instance::InstanceGuardError::Forwarded) => return Ok(()),
        Err(error) => {
            report_startup_failure(&error.to_string());
            return Ok(());
        },
    };

    diagnostics::init_logging();
    std::panic::set_hook(Box::new(|info| {
        let backtrace = std::backtrace::Backtrace::force_capture();
        tracing::error!("panic: {info}\n{backtrace}");
        eprintln!("{info}\n{backtrace}");
    }));
    #[cfg(windows)]
    platform::register_app_id();
    tray::init();

    iced::daemon(
        move || app::Shio::new(launch.clone()),
        app::Shio::update,
        app::Shio::view,
    )
    .settings(iced::Settings {
        antialiasing: false,
        vsync: true,
        ..iced::Settings::default()
    })
    .title("shio")
    .subscription(app::Shio::subscription)
    .theme(app::Shio::theme)
    .style(|_, theme| iced::theme::Style {
        background_color: iced::Color::TRANSPARENT,
        text_color: theme.extended_palette().background.base.text,
    })
    .font(INTER_BYTES)
    .font(iced_fonts::BOOTSTRAP_FONT_BYTES)
    .default_font(INTER)
    .run()
}

// iced asks wgpu for the high-performance adapter, which wakes a discrete GPU on hybrid laptops.
fn prefer_integrated_gpu() {
    #![allow(unsafe_code)]
    if std::env::var_os("WGPU_POWER_PREF").is_none() {
        // SAFETY: called first in `main`, before any other thread exists.
        unsafe { std::env::set_var("WGPU_POWER_PREF", "low") };
    }
}

// Logging is not set up yet; rfd needs zenity on Linux, so a notification is the fallback.
fn report_startup_failure(error: &str) {
    eprintln!("shio could not start: {error}");
    let shown = rfd::MessageDialog::new()
        .set_title("shio could not start")
        .set_description(error)
        .set_level(rfd::MessageLevel::Error)
        .set_buttons(rfd::MessageButtons::Ok)
        .show();
    if shown != rfd::MessageDialogResult::Ok {
        app::show_notification("shio could not start", error);
    }
}
