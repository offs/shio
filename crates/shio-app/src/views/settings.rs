use crate::app::SettingsDraft;
use crate::message::{Message, SeedPolicyChoice, SettingsSection, SpeedUnit};
use crate::platform::ASSOCIATION_SETUP_SUPPORTED;
use crate::style::{
    self, Button, Palette, RADIUS_FIELD, SPACE_L, SPACE_M, SPACE_S, SPACE_XL, SPACE_XS,
};
use crate::theme::ThemeCatalog;
use crate::widgets::{icon_button, pill, setting, switch, text_button};
use iced::widget::text::Wrapping;
use iced::widget::{
    button, column, container, pick_list, row, scrollable, slider, text, text_input,
};
use iced::{Alignment, Border, Color, Element, Length, Padding, Theme};
use iced_fonts::bootstrap;
use shio_core::AppConfig;
use std::ops::RangeInclusive;
use std::path::Path;

const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");
const COMPACT_PADDING: [f32; 2] = [6.0, SPACE_S];

pub(crate) fn view<'a>(
    draft: &'a SettingsDraft,
    config: &'a AppConfig,
    catalog: &'a ThemeCatalog,
    blur_active: bool,
    noctalia: bool,
    p: &Palette,
) -> Element<'a, Message> {
    let nav = column(SettingsSection::ALL.iter().map(|&section| {
        button(text(section.label()).size(style::TEXT_BODY))
            .width(Length::Fill)
            .padding([5.0, SPACE_M])
            .style(style::button(
                *p,
                Button::Nav {
                    active: section == draft.section,
                },
            ))
            .on_press(Message::SettingsSection(section))
            .into()
    }))
    .spacing(2)
    .width(160);

    let page = match draft.section {
        SettingsSection::General => general(config, p),
        SettingsSection::Downloads => downloads(draft, config, p),
        SettingsSection::Torrents => torrents(draft, config, p),
        SettingsSection::Appearance => appearance(config, catalog, blur_active, noctalia, p),
        SettingsSection::About => about(p),
    };

    let header = row![
        text("settings")
            .size(style::TEXT_TITLE)
            .color(p.text_primary)
            .width(Length::Fill),
        icon_button(bootstrap::x_lg(), "close", Some(Message::CloseOverlay), p),
    ]
    .align_y(Alignment::Center)
    .padding(Padding::new(SPACE_M).left(SPACE_XL));

    container(column![
        header,
        row![
            container(nav).padding(Padding::from([SPACE_XS, SPACE_M]).bottom(SPACE_M)),
            scrollable(container(page).padding(Padding::new(SPACE_XL).top(SPACE_S).left(SPACE_L)))
                .width(Length::Fill)
                .style(style::scrollable(*p)),
        ]
        .height(Length::Fill),
    ])
    .max_width(780)
    .max_height(560)
    .style(style::dialog(*p))
    .into()
}

fn page<'a>(items: impl IntoIterator<Item = Element<'a, Message>>) -> Element<'a, Message> {
    column(items).spacing(SPACE_L + SPACE_XS).into()
}

fn general<'a>(config: &'a AppConfig, p: &Palette) -> Element<'a, Message> {
    let [clipboard, notifications, tray] = behavior_settings(config, p);
    let associations = ASSOCIATION_SETUP_SUPPORTED.then(|| {
        setting(
            "torrent and magnet links",
            Some("open them with shio"),
            row![
                text_button(
                    "remove",
                    Button::Ghost,
                    Some(Message::UnregisterAssociations),
                    p
                ),
                text_button(
                    "set up",
                    Button::Secondary,
                    Some(Message::RegisterAssociations),
                    p
                ),
            ]
            .spacing(SPACE_S),
            p,
        )
    });
    page([
        download_folder(&config.download_dir, p),
        notifications,
        tray,
        clipboard,
        associations.into(),
    ])
}

fn downloads<'a>(
    draft: &'a SettingsDraft,
    config: &'a AppConfig,
    p: &Palette,
) -> Element<'a, Message> {
    let speed = row![
        switch(draft.speed_limit_on, Message::SpeedLimitToggled, p),
        number_input(
            &draft.speed_limit,
            draft.speed_limit_on.then_some(Message::SpeedLimitInput),
            p
        ),
        choice(
            &SpeedUnit::ALL,
            draft.speed_unit,
            Message::SpeedLimitUnit,
            p
        ),
    ]
    .spacing(SPACE_S)
    .align_y(Alignment::Center);

    page([
        setting(
            "downloads at once",
            None,
            stepper(
                draft.max_concurrent,
                1..=20,
                Message::MaxConcurrentChanged,
                p,
            ),
            p,
        ),
        setting(
            "connections per download",
            Some("more connections can speed up slow servers"),
            stepper(
                draft.segments,
                1..=shio_core::MAX_SEGMENTS,
                Message::SegmentsChanged,
                p,
            ),
            p,
        ),
        with_error(
            setting("speed limit", None, speed, p),
            draft.speed_limit_error,
            p,
        ),
        setting(
            "put each download in its own folder",
            None,
            switch(
                config.default_create_subfolder,
                |_| Message::DefaultCreateSubfolderToggled,
                p,
            ),
            p,
        ),
        setting(
            "extract archives",
            Some("unpack zip, 7z and rar files when they finish"),
            switch(
                config.default_auto_extract,
                |_| Message::DefaultAutoExtractToggled,
                p,
            ),
            p,
        ),
        setting(
            "delete archives after extracting",
            None,
            switch(
                config.delete_archive_after_extract,
                |_| Message::DeleteArchiveAfterExtractToggled,
                p,
            ),
            p,
        ),
    ])
}

fn torrents<'a>(
    draft: &'a SettingsDraft,
    config: &'a AppConfig,
    p: &Palette,
) -> Element<'a, Message> {
    let mut items = vec![
        with_error(
            setting(
                "listening port",
                Some("applies after restart"),
                number_input(&draft.port, Some(Message::TorrentPortInput), p),
                p,
            ),
            draft.port_error,
            p,
        ),
        setting(
            "dht",
            Some("find peers without trackers · applies after restart"),
            switch(config.torrent.dht, Message::TorrentDhtToggled, p),
            p,
        ),
        setting(
            "upnp",
            Some("open the port on your router · applies after restart"),
            switch(config.torrent.upnp, Message::TorrentUpnpToggled, p),
            p,
        ),
        setting(
            "after downloading",
            None,
            choice(
                &SeedPolicyChoice::ALL,
                draft.seed,
                Message::SeedPolicyChanged,
                p,
            ),
            p,
        ),
    ];
    if draft.seed == SeedPolicyChoice::StopAutomatically {
        items.extend([
            with_error(
                setting(
                    "stop at ratio",
                    None,
                    number_input(&draft.ratio, Some(Message::SeedRatioInput), p),
                    p,
                ),
                draft.ratio_error,
                p,
            ),
            with_error(
                setting(
                    "or after days",
                    None,
                    number_input(&draft.days, Some(Message::SeedDaysInput), p),
                    p,
                ),
                draft.days_error,
                p,
            ),
        ]);
    }
    page(items)
}

fn appearance<'a>(
    config: &'a AppConfig,
    catalog: &'a ThemeCatalog,
    blur_active: bool,
    noctalia: bool,
    p: &Palette,
) -> Element<'a, Message> {
    page([
        column![
            text("theme").size(style::TEXT_BODY).color(p.text_primary),
            theme_picker(catalog, &config.theme.id, noctalia, p),
        ]
        .spacing(SPACE_M)
        .into(),
        blur_setting(config.window.blur, blur_active, p),
    ])
}

fn about<'a>(p: &Palette) -> Element<'a, Message> {
    page([
        column![
            text("shio").size(style::TEXT_DISPLAY).color(p.text_primary),
            text(concat!("version ", env!("CARGO_PKG_VERSION")))
                .size(style::TEXT_SMALL)
                .color(p.text_secondary),
        ]
        .spacing(SPACE_XS)
        .into(),
        setting(
            "source code",
            Some(REPOSITORY),
            text_button(
                "open",
                Button::Secondary,
                Some(Message::OpenUrl(REPOSITORY)),
                p,
            ),
            p,
        ),
        setting(
            "logs",
            Some("useful when reporting a problem"),
            text_button(
                "open folder",
                Button::Secondary,
                Some(Message::OpenLogsFolder),
                p,
            ),
            p,
        ),
    ])
}

pub(super) fn behavior_settings<'a>(config: &AppConfig, p: &Palette) -> [Element<'a, Message>; 3] {
    [
        setting(
            "watch the clipboard",
            Some("offer to add links you copy"),
            switch(config.clipboard_monitor, |_| Message::ToggleClipboard, p),
            p,
        ),
        setting(
            "notifications",
            Some("when a download finishes or fails"),
            switch(config.notifications, |_| Message::ToggleNotifications, p),
            p,
        ),
        setting(
            "close to tray",
            Some("keep downloading after the window closes"),
            switch(config.close_to_tray, |_| Message::ToggleCloseToTray, p),
            p,
        ),
    ]
}

pub(super) fn blur_setting<'a>(on: bool, active: bool, p: &Palette) -> Element<'a, Message> {
    setting(
        "background blur",
        Some(if on && !active {
            "not available here"
        } else {
            "let the desktop show through the window"
        }),
        switch(on, Message::BlurToggled, p),
        p,
    )
}

pub(super) fn theme_picker<'a>(
    catalog: &'a ThemeCatalog,
    current: &str,
    noctalia: bool,
    p: &Palette,
) -> Element<'a, Message> {
    let themes = catalog.themes().iter().map(|theme| {
        theme_tile(
            &theme.name,
            theme.swatch(),
            theme.id.as_str() == current,
            Message::ThemeChanged(theme.id.clone()),
            p,
        )
    });
    let follow = (noctalia
        && !catalog
            .themes()
            .iter()
            .any(|theme| theme.id.as_str() == "noctalia"))
    .then(|| {
        theme_tile(
            "noctalia",
            [p.bg_base, p.text_primary, p.accent],
            false,
            Message::FollowNoctalia,
            p,
        )
    });
    row(themes.chain(follow))
        .spacing(SPACE_S)
        .wrap()
        .vertical_spacing(SPACE_S)
        .into()
}

fn theme_tile<'a>(
    name: &'a str,
    [background, foreground, accent]: [Color; 3],
    active: bool,
    on_press: Message,
    p: &Palette,
) -> Element<'a, Message> {
    let preview = container(
        column![
            row![pill(accent, 6.0, 6.0), pill(foreground, 36.0, 4.0)]
                .spacing(SPACE_XS)
                .align_y(Alignment::Center),
            pill(accent, 48.0, 4.0),
        ]
        .spacing(6),
    )
    .width(Length::Fill)
    .padding([SPACE_S + 2.0, SPACE_S])
    .style(move |_: &Theme| container::Style {
        background: Some(background.into()),
        border: Border {
            color: style::alpha(foreground, 0.15),
            width: 1.0,
            radius: RADIUS_FIELD.into(),
        },
        ..container::Style::default()
    });
    button(
        column![
            preview,
            text(name).size(style::TEXT_SMALL).wrapping(Wrapping::None),
        ]
        .spacing(6)
        .width(96),
    )
    .padding(6)
    .style(style::button(*p, Button::Nav { active }))
    .on_press(on_press)
    .into()
}

pub(super) fn download_folder<'a>(dir: &'a Path, p: &Palette) -> Element<'a, Message> {
    let field = row![
        container(
            text(dir.to_string_lossy())
                .size(style::TEXT_BODY)
                .color(p.text_primary)
                .wrapping(Wrapping::None)
        )
        .width(260)
        .clip(true)
        .padding(style::CONTROL_PADDING)
        .style(style::rounded(p.hover, RADIUS_FIELD)),
        text_button(
            "change…",
            Button::Secondary,
            Some(Message::PickDownloadFolder),
            p
        ),
    ]
    .spacing(SPACE_S)
    .align_y(Alignment::Center);
    setting("download folder", None, field, p)
}

fn choice<'a, T>(
    options: &'a [T],
    selected: T,
    on_select: fn(T) -> Message,
    p: &Palette,
) -> Element<'a, Message>
where
    T: ToString + PartialEq + Clone,
{
    pick_list(options, Some(selected), on_select)
        .text_size(style::TEXT_BODY)
        .padding(COMPACT_PADDING)
        .style(style::pick_list(*p))
        .menu_style(style::pick_list_menu(*p))
        .into()
}

fn number_input<'a>(
    value: &'a str,
    on_input: Option<impl Fn(String) -> Message + 'a>,
    p: &Palette,
) -> Element<'a, Message> {
    text_input("", value)
        .on_input_maybe(on_input)
        .on_submit(Message::SettingsCommit)
        .size(style::TEXT_BODY)
        .padding(COMPACT_PADDING)
        .width(88)
        .style(style::text_input(*p))
        .into()
}

fn stepper<'a>(
    value: u8,
    range: RangeInclusive<u8>,
    on_change: fn(u8) -> Message,
    p: &Palette,
) -> Element<'a, Message> {
    row![
        slider(range, value, on_change)
            .on_release(Message::SettingsCommit)
            .width(180)
            .style(style::slider(*p)),
        text(value)
            .size(style::TEXT_BODY)
            .color(p.text_primary)
            .width(24)
            .align_x(Alignment::End),
    ]
    .spacing(SPACE_M)
    .align_y(Alignment::Center)
    .into()
}

fn with_error<'a>(
    content: Element<'a, Message>,
    error: Option<&'static str>,
    p: &Palette,
) -> Element<'a, Message> {
    match error {
        Some(error) => column![
            content,
            container(text(error).size(style::TEXT_SMALL).color(p.error)).align_right(Length::Fill),
        ]
        .spacing(SPACE_XS)
        .into(),
        None => content,
    }
}
