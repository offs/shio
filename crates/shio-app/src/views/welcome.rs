use super::settings::{behavior_settings, blur_setting, download_folder, theme_picker};
use crate::message::{FirstRunStep, Message};
use crate::platform::ASSOCIATION_SETUP_SUPPORTED;
use crate::style::{self, Button, Palette, SPACE_L, SPACE_S, SPACE_XL, SPACE_XS};
use crate::theme::ThemeCatalog;
use crate::widgets::{pill, setting, text_button};
use iced::widget::{Space, column, container, row, text};
use iced::{Alignment, Element, Length};
use shio_core::AppConfig;

pub(crate) fn view<'a>(
    step: FirstRunStep,
    config: &'a AppConfig,
    catalog: &'a ThemeCatalog,
    blur_active: bool,
    noctalia: bool,
    p: &Palette,
) -> Element<'a, Message> {
    let (title, lead, body) = match step {
        FirstRunStep::Look => (
            "welcome to shio",
            "pick a look. you can change it any time in settings.",
            column![
                theme_picker(catalog, &config.theme.id, noctalia, p),
                blur_setting(config.window.blur, blur_active, p),
            ],
        ),
        FirstRunStep::Folder => (
            "where should downloads go?",
            "new downloads are saved here by default.",
            column![download_folder(&config.download_dir, p)].push(
                ASSOCIATION_SETUP_SUPPORTED.then(|| {
                    setting(
                        "torrent and magnet links",
                        Some("open them with shio"),
                        text_button(
                            "set up",
                            Button::Secondary,
                            Some(Message::RegisterAssociations),
                            p,
                        ),
                        p,
                    )
                }),
            ),
        ),
        FirstRunStep::Behavior => (
            "a few preferences",
            "these can be changed later in settings.",
            column(behavior_settings(config, p)),
        ),
    };

    let steps = row([
        FirstRunStep::Look,
        FirstRunStep::Folder,
        FirstRunStep::Behavior,
    ]
    .map(|each| {
        if each == step {
            pill(p.accent, 18.0, 6.0)
        } else {
            pill(p.pressed, 6.0, 6.0)
        }
    }))
    .spacing(SPACE_XS);

    let actions = row![
        text_button("skip", Button::Ghost, Some(Message::CloseOverlay), p),
        Space::new().width(Length::Fill),
    ]
    .push(
        step.previous()
            .map(|_| text_button("back", Button::Ghost, Some(Message::FirstRunBack), p)),
    )
    .push(text_button(
        if step.next().is_some() {
            "continue"
        } else {
            "start using shio"
        },
        Button::Primary,
        Some(Message::FirstRunNext),
        p,
    ))
    .spacing(SPACE_S)
    .align_y(Alignment::Center);

    container(
        column![
            steps,
            column![
                text(title).size(style::TEXT_TITLE).color(p.text_primary),
                text(lead).size(style::TEXT_BODY).color(p.text_secondary),
            ]
            .spacing(SPACE_XS),
            body.spacing(SPACE_L),
            actions,
        ]
        .spacing(SPACE_XL),
    )
    .padding(SPACE_XL)
    .max_width(560)
    .style(style::dialog(*p))
    .into()
}
