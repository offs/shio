#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod clipboard;
mod diagnostics;
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
    diagnostics::init_logging();
    std::panic::set_hook(Box::new(|info| {
        tracing::error!("panic: {info}");
        eprintln!("{info}");
        std::process::exit(1);
    }));

    platform::register_app_id();
    tray::init();

    iced::application(app::Shio::new, app::Shio::update, app::Shio::view)
        .settings(iced::Settings {
            antialiasing: false,
            vsync: true,
            ..iced::Settings::default()
        })
        .title("shio")
        .subscription(app::Shio::subscription)
        .theme(app::Shio::theme)
        .style(|_state, theme| iced::theme::Style {
            background_color: iced::Color::TRANSPARENT,
            text_color: theme.extended_palette().background.base.text,
        })
        .window_size((960.0, 640.0))
        .font(INTER_BYTES)
        .font(iced_fonts::BOOTSTRAP_FONT_BYTES)
        .default_font(INTER)
        .window(iced::window::Settings {
            transparent: true,
            decorations: false,
            icon: platform::window_icon(),
            exit_on_close_request: false,
            ..Default::default()
        })
        .run()
}
