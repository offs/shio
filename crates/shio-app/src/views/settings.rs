use crate::message::{Message, SeedPolicyChoice, SettingsCategory};
use crate::style;
use crate::theme::ThemeCatalog;
use iced::widget::{
    Space, button, center, column, container, mouse_area, opaque, pick_list, row, scrollable,
    slider, stack, text, text_input, toggler,
};
use iced::{Border, Element, Length, Padding, Theme};
use shio_core::{AppConfig, WindowMaterialPreference};

const MODAL_WIDTH: f32 = 940.0;
const MODAL_HEIGHT: f32 = 640.0;
const SIDEBAR_WIDTH: f32 = 232.0;
const CONTROL_WIDTH: f32 = 276.0;
const PATH_CONTROL_WIDTH: f32 = 360.0;
const ROW_HEIGHT: f32 = 58.0;
const APPEARANCE_CONTROL_WIDTH: f32 = 248.0;
const APPEARANCE_PANEL_WIDTH: f32 = 520.0;

#[derive(Debug, Clone, Copy)]
pub(crate) struct TorrentSettingsInputs<'a> {
    pub(crate) port: &'a str,
    pub(crate) port_error: Option<&'a str>,
    pub(crate) ratio: &'a str,
    pub(crate) ratio_error: Option<&'a str>,
    pub(crate) seed_days: &'a str,
    pub(crate) seed_days_error: Option<&'a str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SettingKey {
    DownloadDirectory,
    MaxConcurrentDownloads,
    DefaultSegments,
    DefaultCreateSubfolder,
    DefaultAutoExtract,
    ExtractToSubfolder,
    DeleteArchiveAfterExtract,
    CloseToTray,
    SpeedLimit,
    ClipboardMonitoring,
    TorrentPort,
    TorrentPortForwarding,
    TorrentDht,
    TorrentSeedPolicy,
    FileAssociations,
    DesktopNotifications,
    AppTheme,
    ScrollLongNames,
    Version,
    GitHub,
    Logs,
}

#[derive(Debug, Clone, Copy)]
struct SettingMeta {
    category: SettingsCategory,
    section: &'static str,
    title: &'static str,
    description: &'static str,
    key: SettingKey,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SearchResult {
    category: SettingsCategory,
    section: &'static str,
    title: String,
    key: SettingKey,
}

const DOWNLOADS_SECTION: &str = "Downloads";
const ADD_DOWNLOADS_SECTION: &str = "Add downloads";
const BANDWIDTH_SECTION: &str = "Bandwidth";
const CAPTURE_SECTION: &str = "Capture";
const TORRENT_SECTION: &str = "Torrents";
const SYSTEM_SECTION: &str = "System";
const THEME_SECTION: &str = "Theme";
const APP_SECTION: &str = "App";

const STATIC_SETTINGS: &[SettingMeta] = &[
    SettingMeta {
        category: SettingsCategory::General,
        section: DOWNLOADS_SECTION,
        title: "Download directory",
        description: "Default save location for downloaded files",
        key: SettingKey::DownloadDirectory,
    },
    SettingMeta {
        category: SettingsCategory::General,
        section: DOWNLOADS_SECTION,
        title: "Max concurrent downloads",
        description: "How many downloads can run at the same time",
        key: SettingKey::MaxConcurrentDownloads,
    },
    SettingMeta {
        category: SettingsCategory::General,
        section: DOWNLOADS_SECTION,
        title: "Default segments",
        description: "Number of parallel chunks per download",
        key: SettingKey::DefaultSegments,
    },
    SettingMeta {
        category: SettingsCategory::General,
        section: ADD_DOWNLOADS_SECTION,
        title: "Create subfolder by default",
        description: "Put new downloads into a named subfolder inside the save path",
        key: SettingKey::DefaultCreateSubfolder,
    },
    SettingMeta {
        category: SettingsCategory::General,
        section: ADD_DOWNLOADS_SECTION,
        title: "Auto-extract archives by default",
        description: "Extract zip, 7z, rar, tar.gz, or tar.zst archives after download completes",
        key: SettingKey::DefaultAutoExtract,
    },
    SettingMeta {
        category: SettingsCategory::General,
        section: ADD_DOWNLOADS_SECTION,
        title: "Extract to subfolder",
        description: "Extract into a subfolder named after the archive",
        key: SettingKey::ExtractToSubfolder,
    },
    SettingMeta {
        category: SettingsCategory::General,
        section: ADD_DOWNLOADS_SECTION,
        title: "Delete archive after extract",
        description: "Remove the archive once extraction succeeds",
        key: SettingKey::DeleteArchiveAfterExtract,
    },
    SettingMeta {
        category: SettingsCategory::General,
        section: ADD_DOWNLOADS_SECTION,
        title: "Close to tray",
        description: "Hide the window on close and keep running in the system tray",
        key: SettingKey::CloseToTray,
    },
    SettingMeta {
        category: SettingsCategory::Network,
        section: BANDWIDTH_SECTION,
        title: "Speed limit",
        description: "Maximum download speed in KB/s, 0 for unlimited",
        key: SettingKey::SpeedLimit,
    },
    SettingMeta {
        category: SettingsCategory::Network,
        section: CAPTURE_SECTION,
        title: "Clipboard monitoring",
        description: "Automatically detect URLs copied to clipboard",
        key: SettingKey::ClipboardMonitoring,
    },
    SettingMeta {
        category: SettingsCategory::Network,
        section: TORRENT_SECTION,
        title: "Torrent port",
        description: "Incoming peer port",
        key: SettingKey::TorrentPort,
    },
    SettingMeta {
        category: SettingsCategory::Network,
        section: TORRENT_SECTION,
        title: "Automatic port forwarding",
        description: "Ask the router to open the port",
        key: SettingKey::TorrentPortForwarding,
    },
    SettingMeta {
        category: SettingsCategory::Network,
        section: TORRENT_SECTION,
        title: "DHT",
        description: "Find public peers without trackers",
        key: SettingKey::TorrentDht,
    },
    SettingMeta {
        category: SettingsCategory::Network,
        section: TORRENT_SECTION,
        title: "After download finishes",
        description: "Control post-download seeding",
        key: SettingKey::TorrentSeedPolicy,
    },
    SettingMeta {
        category: SettingsCategory::Network,
        section: TORRENT_SECTION,
        title: "File associations",
        description: "Use Shio for .torrent files and magnet links",
        key: SettingKey::FileAssociations,
    },
    SettingMeta {
        category: SettingsCategory::Notifications,
        section: SYSTEM_SECTION,
        title: "Desktop notifications",
        description: "Show system notifications for download events",
        key: SettingKey::DesktopNotifications,
    },
    SettingMeta {
        category: SettingsCategory::Appearance,
        section: THEME_SECTION,
        title: "App theme",
        description: "Color theme for the UI",
        key: SettingKey::AppTheme,
    },
    SettingMeta {
        category: SettingsCategory::Appearance,
        section: THEME_SECTION,
        title: "Scroll long names",
        description: "Animate long collapsed filenames",
        key: SettingKey::ScrollLongNames,
    },
    SettingMeta {
        category: SettingsCategory::About,
        section: APP_SECTION,
        title: "Version",
        description: "Current Shio build",
        key: SettingKey::Version,
    },
    SettingMeta {
        category: SettingsCategory::About,
        section: APP_SECTION,
        title: "GitHub",
        description: "Project source repository",
        key: SettingKey::GitHub,
    },
    SettingMeta {
        category: SettingsCategory::About,
        section: APP_SECTION,
        title: "Logs",
        description: "Open the folder containing local diagnostic logs",
        key: SettingKey::Logs,
    },
];

fn normalized_query(query: &str) -> Option<String> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_lowercase())
    }
}

fn contains_normalized(field: &str, query: &str) -> bool {
    field.to_lowercase().contains(query)
}

fn setting_matches(query: &str, meta: &SettingMeta) -> bool {
    contains_normalized(meta.title, query)
        || contains_normalized(meta.description, query)
        || contains_normalized(meta.section, query)
}

fn category_matches(query: &str, category: SettingsCategory) -> bool {
    category.display_label().to_lowercase().starts_with(query)
}

fn static_setting_results(query: &str) -> Vec<SearchResult> {
    let Some(query) = normalized_query(query) else {
        return Vec::new();
    };

    STATIC_SETTINGS
        .iter()
        .filter(|meta| setting_matches(&query, meta) || category_matches(&query, meta.category))
        .map(|meta| SearchResult {
            category: meta.category,
            section: meta.section,
            title: meta.title.to_string(),
            key: meta.key,
        })
        .collect()
}

fn search_results(query: &str) -> Vec<SearchResult> {
    let Some(query) = normalized_query(query) else {
        return Vec::new();
    };
    let mut results = static_setting_results(&query);

    results.sort_by_key(|result| {
        (
            SettingsCategory::ALL
                .iter()
                .position(|category| *category == result.category)
                .unwrap_or(usize::MAX),
            section_order(result.section),
            setting_order(result.key),
        )
    });
    results
}

fn section_order(section: &str) -> usize {
    match section {
        DOWNLOADS_SECTION | BANDWIDTH_SECTION | SYSTEM_SECTION | THEME_SECTION | APP_SECTION => 0,
        CAPTURE_SECTION => 1,
        TORRENT_SECTION | ADD_DOWNLOADS_SECTION => 2,
        _ => usize::MAX,
    }
}

const fn setting_order(key: SettingKey) -> usize {
    match key {
        SettingKey::DownloadDirectory
        | SettingKey::SpeedLimit
        | SettingKey::ClipboardMonitoring
        | SettingKey::TorrentPort
        | SettingKey::DesktopNotifications
        | SettingKey::AppTheme
        | SettingKey::Version => 0,
        SettingKey::MaxConcurrentDownloads
        | SettingKey::TorrentPortForwarding
        | SettingKey::ScrollLongNames
        | SettingKey::GitHub => 1,
        SettingKey::DefaultSegments | SettingKey::TorrentDht | SettingKey::Logs => 2,
        SettingKey::TorrentSeedPolicy => 3,
        SettingKey::FileAssociations => 4,
        SettingKey::DefaultCreateSubfolder => 20,
        SettingKey::DefaultAutoExtract => 21,
        SettingKey::ExtractToSubfolder => 22,
        SettingKey::DeleteArchiveAfterExtract => 23,
        SettingKey::CloseToTray => 24,
    }
}

pub(crate) fn view<'a>(
    config: &'a AppConfig,
    category: SettingsCategory,
    settings_search: &'a str,
    torrent_inputs: TorrentSettingsInputs<'a>,
    theme_catalog: &'a ThemeCatalog,
    p: &'a crate::style::Palette,
    material: WindowMaterialPreference,
    base: Element<'a, Message>,
) -> Element<'a, Message> {
    let sidebar = sidebar_view(category, settings_search, p);
    let content = content_view(
        config,
        category,
        settings_search,
        torrent_inputs,
        theme_catalog,
        p,
    );

    let body_row = row![sidebar, vertical_divider(p), content].height(Length::Fill);

    let modal = container(body_row)
        .width(MODAL_WIDTH)
        .height(MODAL_HEIGHT)
        .style(style::modal_card(p, material));

    let overlay = mouse_area(center(opaque(modal))).on_press(Message::CloseSettings);

    let backdrop = container(overlay)
        .style(style::modal_backdrop(p))
        .width(Length::Fill)
        .height(Length::Fill);

    stack![base, backdrop].into()
}

fn sidebar_view<'a>(
    active: SettingsCategory,
    settings_search: &'a str,
    p: &'a crate::style::Palette,
) -> Element<'a, Message> {
    let search = settings_search_input(settings_search, p);
    let search_active = normalized_query(settings_search).is_some();

    let mut items: Vec<Element<'a, Message>> = Vec::with_capacity(SettingsCategory::ALL.len());
    for &cat in SettingsCategory::ALL {
        items.push(category_button(cat, cat == active && !search_active, p));
    }

    let list = column(items).spacing(3);

    container(column![
        container(search).padding(Padding::default().top(16).right(12).bottom(10).left(12)),
        container(list).padding([0, 10]),
        Space::new().height(Length::Fill),
        container(text("Ctrl+,").size(11).color(p.text_tertiary)).padding([10, 16]),
    ])
    .width(SIDEBAR_WIDTH)
    .height(Length::Fill)
    .into()
}

fn settings_search_input<'a>(
    settings_search: &'a str,
    p: &'a crate::style::Palette,
) -> Element<'a, Message> {
    let input = text_input("Search settings...", settings_search)
        .id(crate::app::SETTINGS_SEARCH_INPUT_ID.clone())
        .on_input(Message::SettingsSearchChanged)
        .style(style::input(p))
        .size(13)
        .padding([7, 10])
        .width(Length::Fill);

    let clear: Element<'_, Message> = if normalized_query(settings_search).is_some() {
        button(iced_fonts::bootstrap::x_lg().size(10))
            .style(style::btn_icon(p))
            .on_press(Message::SettingsSearchCleared)
            .padding([7, 9])
            .into()
    } else {
        Space::new().width(28).height(28).into()
    };

    row![input, clear]
        .spacing(6)
        .align_y(iced::Alignment::Center)
        .into()
}

fn category_button(
    cat: SettingsCategory,
    active: bool,
    p: &crate::style::Palette,
) -> Element<'_, Message> {
    let label = text(cat.display_label()).size(13);
    button(label)
        .style(style::sidebar_item(p, active))
        .on_press(Message::SettingsCategoryChanged(cat))
        .padding([9, 12])
        .width(Length::Fill)
        .into()
}

fn vertical_divider(p: &crate::style::Palette) -> Element<'_, Message> {
    container(Space::new().width(1).height(Length::Fill))
        .style(style::separator(p))
        .width(1)
        .height(Length::Fill)
        .into()
}

fn content_view<'a>(
    config: &'a AppConfig,
    category: SettingsCategory,
    settings_search: &'a str,
    torrent_inputs: TorrentSettingsInputs<'a>,
    theme_catalog: &'a ThemeCatalog,
    p: &'a crate::style::Palette,
) -> Element<'a, Message> {
    let query = normalized_query(settings_search);
    let results = query.as_deref().map(search_results).unwrap_or_default();

    let header = content_header(category, settings_search, results.len(), p);
    let body: Element<'a, Message> = if query.is_some() {
        search_panel(
            config,
            settings_search,
            &results,
            torrent_inputs,
            theme_catalog,
            p,
        )
    } else {
        match category {
            SettingsCategory::General => category_panel(
                config,
                SettingsCategory::General,
                torrent_inputs,
                theme_catalog,
                p,
            ),
            SettingsCategory::Network => category_panel(
                config,
                SettingsCategory::Network,
                torrent_inputs,
                theme_catalog,
                p,
            ),
            SettingsCategory::Notifications => category_panel(
                config,
                SettingsCategory::Notifications,
                torrent_inputs,
                theme_catalog,
                p,
            ),
            SettingsCategory::Appearance => category_panel(
                config,
                SettingsCategory::Appearance,
                torrent_inputs,
                theme_catalog,
                p,
            ),
            SettingsCategory::About => category_panel(
                config,
                SettingsCategory::About,
                torrent_inputs,
                theme_catalog,
                p,
            ),
        }
    };

    let padded = container(body).padding(Padding::default().right(24).bottom(24).left(24));
    let scrolled = scrollable(padded)
        .height(Length::Fill)
        .style(style::scrollable_style(p));

    container(column![
        header,
        container(scrolled).height(Length::Fill).width(Length::Fill),
    ])
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

fn content_header<'a>(
    category: SettingsCategory,
    settings_search: &'a str,
    result_count: usize,
    p: &'a crate::style::Palette,
) -> Element<'a, Message> {
    let search_active = normalized_query(settings_search).is_some();
    let title = if search_active {
        format!("Search results for \"{}\"", settings_search.trim())
    } else {
        category.display_label().to_string()
    };
    let description = if search_active {
        match result_count {
            1 => "1 setting".to_string(),
            n => format!("{n} settings"),
        }
    } else {
        category.description().to_string()
    };

    let close = button(iced_fonts::bootstrap::x_lg().size(12))
        .style(style::btn_icon(p))
        .on_press(Message::CloseSettings)
        .padding([8, 10]);

    container(
        row![
            column![
                text(title).size(20).color(p.text_primary),
                Space::new().height(5),
                text(description).size(12).color(p.text_tertiary),
            ]
            .width(Length::Fill),
            close,
        ]
        .align_y(iced::Alignment::Start),
    )
    .padding(Padding::default().top(14).right(14).bottom(18).left(24))
    .width(Length::Fill)
    .into()
}

fn category_panel<'a>(
    config: &'a AppConfig,
    category: SettingsCategory,
    torrent_inputs: TorrentSettingsInputs<'a>,
    theme_catalog: &'a ThemeCatalog,
    p: &'a crate::style::Palette,
) -> Element<'a, Message> {
    if category == SettingsCategory::Appearance {
        return appearance_panel(config, theme_catalog, p);
    }

    settings_list(
        config,
        &category_results(category),
        torrent_inputs,
        theme_catalog,
        false,
        p,
    )
}

fn search_panel<'a>(
    config: &'a AppConfig,
    query: &str,
    results: &[SearchResult],
    torrent_inputs: TorrentSettingsInputs<'a>,
    theme_catalog: &'a ThemeCatalog,
    p: &'a crate::style::Palette,
) -> Element<'a, Message> {
    if results.is_empty() {
        let clear = button(text("Clear search").size(12))
            .style(style::btn_secondary(p))
            .on_press(Message::SettingsSearchCleared)
            .padding([8, 12]);
        return column![
            Space::new().height(18),
            text("No settings found").size(15).color(p.text_primary),
            Space::new().height(5),
            text(format!("No settings match \"{}\".", query.trim()))
                .size(12)
                .color(p.text_tertiary),
            Space::new().height(12),
            row![clear],
        ]
        .into();
    }

    settings_list(config, results, torrent_inputs, theme_catalog, true, p)
}

fn category_results(category: SettingsCategory) -> Vec<SearchResult> {
    let mut results: Vec<SearchResult> = STATIC_SETTINGS
        .iter()
        .filter(|meta| meta.category == category)
        .map(|meta| SearchResult {
            category: meta.category,
            section: meta.section,
            title: meta.title.to_string(),
            key: meta.key,
        })
        .collect();

    results.sort_by_key(|result| (section_order(result.section), setting_order(result.key)));
    results
}

fn settings_list<'a>(
    config: &'a AppConfig,
    results: &[SearchResult],
    torrent_inputs: TorrentSettingsInputs<'a>,
    theme_catalog: &'a ThemeCatalog,
    show_categories: bool,
    p: &'a crate::style::Palette,
) -> Element<'a, Message> {
    let mut list = column![].spacing(0);
    let mut last_category: Option<SettingsCategory> = None;
    let mut last_section: Option<&'static str> = None;

    for result in results {
        if show_categories && last_category != Some(result.category) {
            list = list.push(category_heading(result.category, p));
            last_category = Some(result.category);
            last_section = None;
        }
        if last_section != Some(result.section) {
            list = list.push(section_heading(result.section, p));
            last_section = Some(result.section);
        }
        list = list.push(render_setting(
            config,
            result,
            torrent_inputs,
            theme_catalog,
            p,
        ));
    }

    list.into()
}

fn category_heading(category: SettingsCategory, p: &crate::style::Palette) -> Element<'_, Message> {
    column![
        Space::new().height(18),
        text(category.display_label())
            .size(15)
            .color(p.text_primary),
        Space::new().height(6),
        text(category.description()).size(12).color(p.text_tertiary),
        Space::new().height(4),
    ]
    .into()
}

fn render_setting<'a>(
    config: &'a AppConfig,
    result: &SearchResult,
    torrent_inputs: TorrentSettingsInputs<'a>,
    _theme_catalog: &'a ThemeCatalog,
    p: &'a crate::style::Palette,
) -> Element<'a, Message> {
    match result.key {
        SettingKey::DownloadDirectory => download_directory_row(config, p),
        SettingKey::MaxConcurrentDownloads => setting_row(
            "Max concurrent downloads",
            "How many downloads can run at the same time",
            slider_u8(
                1..=20,
                config.max_concurrent,
                Message::MaxConcurrentChanged,
                config.max_concurrent.to_string(),
                32,
                p,
            ),
            p,
        ),
        SettingKey::DefaultSegments => setting_row(
            "Default segments",
            "Number of parallel chunks per download",
            slider_u8(
                1..=32,
                config.default_segments,
                Message::DefaultSegmentsChanged,
                config.default_segments.to_string(),
                32,
                p,
            ),
            p,
        ),
        SettingKey::DefaultCreateSubfolder => toggle_row(
            "Create subfolder by default",
            "Put new downloads into a named subfolder inside the save path",
            config.default_create_subfolder,
            |_| Message::DefaultCreateSubfolderToggled,
            p,
        ),
        SettingKey::DefaultAutoExtract => toggle_row(
            "Auto-extract archives by default",
            "Extract zip, 7z, rar, tar.gz, or tar.zst archives after download completes",
            config.default_auto_extract,
            |_| Message::DefaultAutoExtractToggled,
            p,
        ),
        SettingKey::ExtractToSubfolder => toggle_row(
            "Extract to subfolder",
            "Extract into a subfolder named after the archive",
            config.extract_to_subfolder,
            |_| Message::ExtractToSubfolderToggled,
            p,
        ),
        SettingKey::DeleteArchiveAfterExtract => toggle_row(
            "Delete archive after extract",
            "Remove the archive once extraction succeeds",
            config.delete_archive_after_extract,
            |_| Message::DeleteArchiveAfterExtractToggled,
            p,
        ),
        SettingKey::CloseToTray => toggle_row(
            "Close to tray",
            "Hide the window on close and keep running in the system tray",
            config.close_to_tray,
            |_| Message::CloseToTrayToggled,
            p,
        ),
        SettingKey::SpeedLimit => speed_limit_row(config, p),
        SettingKey::ClipboardMonitoring => toggle_row(
            "Clipboard monitoring",
            "Automatically detect URLs copied to clipboard",
            config.clipboard_monitor,
            |_| Message::ToggleClipboard,
            p,
        ),
        SettingKey::TorrentPort => torrent_port_row(torrent_inputs, p),
        SettingKey::TorrentPortForwarding => toggle_row(
            "Automatic port forwarding",
            "Ask the router to open the port",
            config.torrent.upnp,
            Message::TorrentUpnpToggled,
            p,
        ),
        SettingKey::TorrentDht => toggle_row(
            "DHT",
            "Find public peers without trackers",
            config.torrent.dht,
            Message::TorrentDhtToggled,
            p,
        ),
        SettingKey::TorrentSeedPolicy => torrent_seed_policy_row(config, torrent_inputs, p),
        SettingKey::FileAssociations => {
            let associations_button = button(text("Set up").size(12))
                .style(style::btn_secondary(p))
                .on_press(Message::SetUpFileAssociations)
                .padding([7, 12]);
            setting_row(
                "File associations",
                "Register Shio for .torrent files and magnet links",
                associations_button.into(),
                p,
            )
        },
        SettingKey::DesktopNotifications => toggle_row(
            "Desktop notifications",
            "Show system notifications for download events",
            config.notifications,
            |_| Message::ToggleNotifications,
            p,
        ),
        SettingKey::AppTheme => {
            let open_appearance = button(text("Open").size(12))
                .style(style::btn_secondary(p))
                .on_press(Message::SettingsCategoryChanged(
                    SettingsCategory::Appearance,
                ))
                .padding([7, 12]);
            setting_row(
                "App theme",
                "Open Appearance to edit theme settings",
                open_appearance.into(),
                p,
            )
        },
        SettingKey::ScrollLongNames => toggle_row(
            "Scroll long names",
            "Animate long collapsed filenames",
            config.scroll_long_names,
            |_| Message::ScrollLongNamesToggled,
            p,
        ),
        SettingKey::Version => read_only_row(
            "Version",
            "Current Shio build",
            text(format!("shio {}", env!("CARGO_PKG_VERSION")))
                .size(13)
                .color(p.text_secondary)
                .into(),
            p,
        ),
        SettingKey::GitHub => {
            let repo = env!("CARGO_PKG_REPOSITORY");
            let repo_display = repo
                .trim_start_matches("https://")
                .trim_start_matches("http://");
            read_only_row(
                "GitHub",
                "Project source repository",
                text(repo_display.to_string())
                    .size(13)
                    .color(p.text_secondary)
                    .into(),
                p,
            )
        },
        SettingKey::Logs => {
            let logs_button = button(text("Open logs").size(12))
                .style(style::btn_secondary(p))
                .on_press(Message::OpenLogsFolder)
                .padding([7, 12]);
            setting_row(
                "Logs",
                "Open the folder containing local diagnostic logs",
                logs_button.into(),
                p,
            )
        },
    }
}

fn download_directory_row<'a>(
    config: &'a AppConfig,
    p: &'a crate::style::Palette,
) -> Element<'a, Message> {
    let dir_input = text_input("", &config.download_dir.to_string_lossy())
        .style(style::input(p))
        .size(13)
        .padding([7, 10])
        .width(Length::Fill);
    let dir_browse = button(text("Browse").size(12))
        .style(style::btn_secondary(p))
        .on_press(Message::PickSaveFolder)
        .padding([7, 12]);
    let dir_control: Element<'_, Message> = row![dir_input, Space::new().width(6), dir_browse]
        .width(Length::Fixed(PATH_CONTROL_WIDTH))
        .align_y(iced::Alignment::Center)
        .into();

    setting_row_with_control_width(
        "Download directory",
        "Default save location for downloaded files",
        dir_control,
        PATH_CONTROL_WIDTH,
        p,
    )
}

fn speed_limit_row<'a>(
    config: &'a AppConfig,
    p: &'a crate::style::Palette,
) -> Element<'a, Message> {
    let speed_bytes = config.speed_limit.unwrap_or(0);
    let speed_kbps = (speed_bytes / 1024).min(10240) as u16;
    let speed_display = if speed_bytes == 0 {
        "unlimited".to_string()
    } else {
        format!("{}/s", bytesize::ByteSize(speed_bytes))
    };

    setting_row(
        "Speed limit",
        "Maximum download speed in KB/s, 0 for unlimited",
        slider_u16(
            0..=10240,
            speed_kbps,
            |v| {
                Message::SpeedLimitChanged(if v == 0 {
                    None
                } else {
                    Some(u64::from(v) * 1024)
                })
            },
            speed_display,
            84,
            p,
        ),
        p,
    )
}

fn torrent_port_row<'a>(
    inputs: TorrentSettingsInputs<'a>,
    p: &'a crate::style::Palette,
) -> Element<'a, Message> {
    let input = text_input("", inputs.port)
        .on_input(Message::TorrentPortInputChanged)
        .style(style::input(p))
        .size(13)
        .padding([7, 10])
        .width(Length::Fixed(96.0));

    setting_row_with_error(
        "Torrent port",
        "Incoming peer port. Applies after restart.",
        input.into(),
        inputs.port_error,
        p,
    )
}

fn torrent_seed_policy_row<'a>(
    config: &'a AppConfig,
    inputs: TorrentSettingsInputs<'a>,
    p: &'a crate::style::Palette,
) -> Element<'a, Message> {
    let picker = pick_list(
        &SeedPolicyChoice::ALL[..],
        Some(SeedPolicyChoice::from_policy(config.torrent.seed_policy)),
        Message::TorrentSeedPolicyChoiceChanged,
    )
    .text_size(13)
    .padding([7, 12])
    .style(style::pick_list_style(p))
    .menu_style(style::menu_style(p));

    let choice = SeedPolicyChoice::from_policy(config.torrent.seed_policy);
    let mut body = column![setting_row(
        "After download finishes",
        "Control post-download seeding",
        picker.into(),
        p
    )]
    .spacing(0);

    match choice {
        SeedPolicyChoice::StopAutomatically => {
            body = body
                .push(seed_ratio_row(inputs, p))
                .push(seed_days_row(inputs, p))
                .push(
                    container(
                        text(seed_policy_summary(inputs.ratio, inputs.seed_days))
                            .size(12)
                            .color(p.text_tertiary),
                    )
                    .padding(Padding::default().left(0).bottom(8)),
                );
        },
        SeedPolicyChoice::SeedForever => {
            body = body.push(seed_policy_hint("Keep uploading until stopped.", p));
        },
        SeedPolicyChoice::NeverSeed => {
            body = body.push(seed_policy_hint("Complete after verification.", p));
        },
    }

    body.into()
}

fn seed_ratio_row<'a>(
    inputs: TorrentSettingsInputs<'a>,
    p: &'a crate::style::Palette,
) -> Element<'a, Message> {
    let input = text_input("", inputs.ratio)
        .on_input(Message::TorrentSeedRatioInputChanged)
        .style(style::input(p))
        .size(13)
        .padding([7, 10])
        .width(Length::Fixed(96.0));

    setting_row_with_error(
        "Share ratio",
        "Upload divided by selected size",
        input.into(),
        inputs.ratio_error,
        p,
    )
}

fn seed_days_row<'a>(
    inputs: TorrentSettingsInputs<'a>,
    p: &'a crate::style::Palette,
) -> Element<'a, Message> {
    let input = text_input("", inputs.seed_days)
        .on_input(Message::TorrentSeedDaysInputChanged)
        .style(style::input(p))
        .size(13)
        .padding([7, 10])
        .width(Length::Fixed(72.0));
    let control = row![input, text("days").size(12).color(p.text_secondary)]
        .spacing(8)
        .align_y(iced::Alignment::Center);

    setting_row_with_error(
        "Active seed time",
        "Time actively seeding",
        control.into(),
        inputs.seed_days_error,
        p,
    )
}

fn seed_policy_hint<'a>(copy: &'a str, p: &'a crate::style::Palette) -> Element<'a, Message> {
    container(text(copy).size(12).color(p.text_tertiary))
        .padding(Padding::default().bottom(8))
        .into()
}

fn seed_policy_summary(ratio: &str, days: &str) -> String {
    format!("Stops at ratio {} or {} days.", ratio.trim(), days.trim())
}

fn appearance_panel<'a>(
    config: &'a AppConfig,
    catalog: &'a ThemeCatalog,
    p: &'a crate::style::Palette,
) -> Element<'a, Message> {
    let content = column![
        text("Theme").size(15).color(p.text_primary),
        Space::new().height(4),
        text("Choose how Shio selects colors.")
            .size(12)
            .color(p.text_tertiary),
        Space::new().height(14),
        theme_picker_row(
            "Theme",
            catalog.ids(),
            crate::theme::ThemeId::parse(&config.theme.id).ok(),
            Message::ThemeChanged,
            p,
        ),
        Space::new().height(6),
        thin_separator(p),
        Space::new().height(8),
        material_picker_row(config.window.material, p),
        Space::new().height(6),
        compact_labeled_row(
            "Scroll long names",
            toggler(config.scroll_long_names)
                .on_toggle(|_| Message::ScrollLongNamesToggled)
                .size(18)
                .style(style::toggler_style(p))
                .into(),
            p,
        ),
        Space::new().height(14),
        theme_preview(p),
    ]
    .width(Length::Fixed(APPEARANCE_PANEL_WIDTH));

    column![section_heading(THEME_SECTION, p), content]
        .spacing(0)
        .into()
}

fn theme_picker_row<'a>(
    label: &'a str,
    options: Vec<crate::theme::ThemeId>,
    selected: Option<crate::theme::ThemeId>,
    on_select: impl Fn(crate::theme::ThemeId) -> Message + 'a,
    p: &'a crate::style::Palette,
) -> Element<'a, Message> {
    let picker = pick_list(options, selected, on_select)
        .text_size(13)
        .padding([7, 12])
        .width(Length::Fixed(APPEARANCE_CONTROL_WIDTH))
        .style(style::pick_list_style(p))
        .menu_style(style::menu_style(p));

    compact_labeled_row(label, picker.into(), p)
}

fn material_picker_row(
    current: WindowMaterialPreference,
    p: &crate::style::Palette,
) -> Element<'_, Message> {
    let picker = pick_list(
        &WindowMaterialPreference::ALL[..],
        Some(current),
        Message::ThemeMaterialChanged,
    )
    .text_size(13)
    .padding([7, 12])
    .width(Length::Fixed(APPEARANCE_CONTROL_WIDTH))
    .style(style::pick_list_style(p))
    .menu_style(style::menu_style(p));

    compact_labeled_row("Window material", picker.into(), p)
}

fn compact_labeled_row<'a>(
    label: &'a str,
    control: Element<'a, Message>,
    p: &'a crate::style::Palette,
) -> Element<'a, Message> {
    row![
        text(label)
            .size(13)
            .color(p.text_primary)
            .width(Length::Fill),
        control,
    ]
    .height(Length::Fixed(42.0))
    .align_y(iced::Alignment::Center)
    .into()
}

fn theme_preview(p: &crate::style::Palette) -> Element<'_, Message> {
    let base = preview_swatch("base", p.bg_base, p);
    let surface = preview_swatch("surface", p.bg_elevated, p);
    let accent = preview_swatch("accent", p.accent, p);

    container(
        row![base, surface, accent]
            .spacing(8)
            .align_y(iced::Alignment::Center),
    )
    .padding(8)
    .style(style::section(p))
    .width(Length::Fill)
    .into()
}

fn preview_swatch<'a>(
    label: &'a str,
    color: iced::Color,
    p: &'a crate::style::Palette,
) -> Element<'a, Message> {
    let swatch = container(Space::new().height(18))
        .style(move |_: &Theme| container::Style {
            background: Some(color.into()),
            border: Border {
                color: p.border_default,
                width: 1.0,
                radius: 4.0.into(),
            },
            ..container::Style::default()
        })
        .width(Length::Fill);

    column![
        swatch,
        Space::new().height(5),
        text(label).size(11).color(p.text_tertiary),
    ]
    .width(Length::Fill)
    .into()
}

fn thin_separator(p: &crate::style::Palette) -> Element<'_, Message> {
    container(Space::new().height(1))
        .style(style::separator(p))
        .width(Length::Fill)
        .height(1)
        .into()
}

fn toggle_row<'a>(
    title: &'a str,
    description: &'a str,
    value: bool,
    on_toggle: impl Fn(bool) -> Message + 'a,
    p: &'a crate::style::Palette,
) -> Element<'a, Message> {
    setting_row(
        title,
        description,
        toggler(value)
            .on_toggle(on_toggle)
            .size(18)
            .style(style::toggler_style(p))
            .into(),
        p,
    )
}

fn read_only_row<'a>(
    title: &'a str,
    description: &'a str,
    control: Element<'a, Message>,
    p: &'a crate::style::Palette,
) -> Element<'a, Message> {
    setting_row(title, description, control, p)
}

fn section_heading<'a>(label: &'a str, p: &'a crate::style::Palette) -> Element<'a, Message> {
    column![
        Space::new().height(16),
        text(label).size(11).color(p.text_tertiary),
        Space::new().height(8),
    ]
    .into()
}

fn setting_row<'a>(
    title: &'a str,
    description: &'a str,
    control: Element<'a, Message>,
    p: &crate::style::Palette,
) -> Element<'a, Message> {
    setting_row_with_control_width(title, description, control, CONTROL_WIDTH, p)
}

fn setting_row_with_error<'a>(
    title: &'a str,
    description: &'a str,
    control: Element<'a, Message>,
    error: Option<&'a str>,
    p: &crate::style::Palette,
) -> Element<'a, Message> {
    let title_text = text(title).size(13).color(p.text_primary);
    let desc_text = text(description).size(12).color(p.text_tertiary);
    let left = column![title_text, Space::new().height(4), desc_text].width(Length::Fill);
    let mut right = column![control].spacing(4).align_x(iced::Alignment::End);
    if let Some(error) = error {
        right = right.push(text(error).size(11).color(p.error));
    }

    container(
        row![
            left,
            Space::new().width(20),
            container(right)
                .width(Length::Fixed(CONTROL_WIDTH))
                .align_x(iced::alignment::Horizontal::Right),
        ]
        .align_y(iced::Alignment::Center),
    )
    .padding([4, 0])
    .width(Length::Fill)
    .into()
}

fn setting_row_with_control_width<'a>(
    title: &'a str,
    description: &'a str,
    control: Element<'a, Message>,
    control_width: f32,
    p: &crate::style::Palette,
) -> Element<'a, Message> {
    let title_text = text(title).size(13).color(p.text_primary);
    let desc_text = text(description).size(12).color(p.text_tertiary);

    let left = column![title_text, Space::new().height(4), desc_text].width(Length::Fill);
    let control_box = container(control)
        .width(Length::Fixed(control_width))
        .align_x(iced::alignment::Horizontal::Right);

    container(row![left, Space::new().width(20), control_box].align_y(iced::Alignment::Center))
        .height(Length::Fixed(ROW_HEIGHT))
        .width(Length::Fill)
        .into()
}

fn slider_u8<'a>(
    range: std::ops::RangeInclusive<u8>,
    value: u8,
    on_change: impl Fn(u8) -> Message + 'a,
    display: String,
    label_width: u16,
    p: &'a crate::style::Palette,
) -> Element<'a, Message> {
    let s = slider(range, value, on_change)
        .style(style::slider_style(p))
        .width(180);
    row![
        s,
        Space::new().width(12),
        text(display)
            .size(12)
            .color(p.text_secondary)
            .width(u32::from(label_width)),
    ]
    .align_y(iced::Alignment::Center)
    .into()
}

fn slider_u16<'a>(
    range: std::ops::RangeInclusive<u16>,
    value: u16,
    on_change: impl Fn(u16) -> Message + 'a,
    display: String,
    label_width: u16,
    p: &'a crate::style::Palette,
) -> Element<'a, Message> {
    let s = slider(range, value, on_change)
        .style(style::slider_style(p))
        .width(180);
    row![
        s,
        Space::new().width(12),
        text(display)
            .size(12)
            .color(p.text_secondary)
            .width(u32::from(label_width)),
    ]
    .align_y(iced::Alignment::Center)
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_finds_matching_rows_across_categories() {
        for (query, category, title) in [
            ("theme", SettingsCategory::Appearance, "App theme"),
            ("scroll", SettingsCategory::Appearance, "Scroll long names"),
            (
                "clipboard",
                SettingsCategory::Network,
                "Clipboard monitoring",
            ),
            (
                "association",
                SettingsCategory::Network,
                "File associations",
            ),
            ("torrent", SettingsCategory::Network, "Torrent port"),
            ("logs", SettingsCategory::About, "Logs"),
        ] {
            let results = search_results(query);
            assert!(
                results
                    .iter()
                    .any(|result| result.category == category && result.title == title),
                "{query}"
            );
        }
    }

    #[test]
    fn search_trims_and_matches_case_insensitively() {
        let results = search_results(" Theme ");

        assert_eq!(
            results.first().map(|result| result.title.as_str()),
            Some("App theme")
        );
    }

    #[test]
    fn whitespace_search_is_empty() {
        assert!(normalized_query(" \t ").is_none());
    }

    #[test]
    fn broad_settings_query_does_not_match_everything() {
        let results = search_results("settings");

        assert!(results.is_empty());
    }

    #[test]
    fn non_match_returns_no_results() {
        assert!(search_results("not-a-real-setting").is_empty());
    }
}
