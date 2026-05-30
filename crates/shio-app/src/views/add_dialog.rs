use crate::app::{
    AddHttpPreview, AddHttpPreviewState, AddMagnetPreview, AddMagnetPreviewState, AddSourceId,
    AddTorrentFile,
};
use crate::message::Message;
use crate::style;
use iced::widget::text::Wrapping;
use iced::widget::{
    Space, button, center, checkbox, column, container, mouse_area, opaque, row, scrollable, stack,
    text, text_editor, text_input,
};
use iced::{Element, Length, Padding};
use std::path::Path;

const MODAL_WIDTH: f32 = 880.0;
const MODAL_HEIGHT: f32 = 600.0;
const LEFT_PANE_WIDTH: f32 = 330.0;
const RIGHT_PANE_WIDTH: f32 = 488.0;
const URL_INPUT_HEIGHT: f32 = 88.0;
const REVIEW_LIST_HEIGHT: f32 = 370.0;
const SOURCE_INBOX_HEIGHT: f32 = 96.0;
const TORRENT_FILE_RENDER_LIMIT: usize = 500;
const TORRENT_SEARCH_THRESHOLD: usize = 20;

pub(crate) struct AddDialogViewModel<'a> {
    pub(crate) urls: &'a text_editor::Content,
    pub(crate) url_entries: &'a [String],
    pub(crate) url_preview: &'a [String],
    pub(crate) url_count: usize,
    pub(crate) http_count: usize,
    pub(crate) magnet_count: usize,
    pub(crate) addable_url_count: usize,
    pub(crate) single_url_name: Option<&'a str>,
    pub(crate) has_archive_url: bool,
    pub(crate) http_previews: &'a [AddHttpPreview],
    pub(crate) magnet_previews: &'a [AddMagnetPreview],
    pub(crate) torrent_files: &'a [AddTorrentFile],
    pub(crate) torrent_search: &'a str,
    pub(crate) selected_source: Option<&'a AddSourceId>,
    pub(crate) filename: &'a str,
    pub(crate) save_path: &'a str,
    pub(crate) create_subfolder: bool,
    pub(crate) subfolder_name: &'a str,
}

fn divider(p: &crate::style::Palette) -> Element<'_, Message> {
    container(Space::new().height(1))
        .style(style::separator(p))
        .height(1)
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

fn label_text<'a>(label: &'a str, p: &'a crate::style::Palette) -> Element<'a, Message> {
    text(label).size(12).color(p.text_primary).into()
}

fn helper_text<'a>(copy: &'a str, p: &'a crate::style::Palette) -> Element<'a, Message> {
    text(copy).size(11).color(p.text_tertiary).into()
}

fn settings_row<'a>(
    title: &'a str,
    description: Element<'a, Message>,
    control: Element<'a, Message>,
    p: &'a crate::style::Palette,
) -> Element<'a, Message> {
    let left = column![
        text(title).size(12).color(p.text_primary),
        Space::new().height(4),
        description,
    ]
    .width(Length::Fill);

    container(row![left, Space::new().width(14), control].align_y(iced::Alignment::Center))
        .padding([12, 0])
        .height(Length::Fixed(58.0))
        .width(Length::Fill)
        .into()
}

fn source_summary(http_count: usize, magnet_count: usize, torrent_file_count: usize) -> String {
    match (http_count, magnet_count, torrent_file_count) {
        (0, 0, 0) => "no sources".to_string(),
        (http, magnets, torrents) => {
            let mut parts = Vec::new();
            if http > 0 {
                parts.push(format!("{http} url{}", if http == 1 { "" } else { "s" }));
            }
            if magnets > 0 {
                parts.push(format!(
                    "{magnets} magnet{}",
                    if magnets == 1 { "" } else { "s" }
                ));
            }
            if torrents > 0 {
                parts.push(format!(
                    "{torrents} torrent{}",
                    if torrents == 1 { "" } else { "s" }
                ));
            }
            parts.join(" · ")
        },
    }
}

fn add_disabled_reason(
    total_sources: usize,
    urls: &[String],
    http_previews: &[AddHttpPreview],
    torrent_files: &[AddTorrentFile],
) -> Option<String> {
    if total_sources == 0 {
        return Some("add at least one source".to_string());
    }
    if let Some(reason) = archive_set_validation_error(urls, http_previews) {
        return Some(reason);
    }
    if torrent_files.iter().any(|torrent| !torrent.has_selection()) {
        return Some("select at least one file".to_string());
    }
    None
}

fn selected_torrent_count(torrent_files: &[AddTorrentFile]) -> usize {
    torrent_files
        .iter()
        .map(AddTorrentFile::selected_count)
        .sum()
}

fn effective_destination(save_path: &str, create_subfolder: bool, subfolder_name: &str) -> String {
    shio_core::subfolder_value(create_subfolder, subfolder_name).map_or_else(
        || save_path.to_string(),
        |subfolder| Path::new(save_path).join(subfolder).display().to_string(),
    )
}

fn result_summary(
    total_sources: usize,
    archive_sets: usize,
    archive_files: usize,
    torrent_files: &[AddTorrentFile],
    save_path: &str,
    create_subfolder: bool,
    subfolder_name: &str,
) -> String {
    let destination = effective_destination(save_path, create_subfolder, subfolder_name);
    if archive_sets > 0 {
        let normal_sources = total_sources.saturating_sub(archive_files);
        let set_label = if archive_sets == 1 {
            "archive set"
        } else {
            "archive sets"
        };
        let file_label = if archive_files == 1 { "file" } else { "files" };
        if normal_sources > 0 {
            return format!(
                "adds {archive_sets} {set_label} with {archive_files} {file_label} and {normal_sources} downloads to {destination}"
            );
        }
        return format!(
            "adds {archive_sets} {set_label} with {archive_files} {file_label} to {destination}"
        );
    }
    if !torrent_files.is_empty() {
        let url_sources = total_sources.saturating_sub(torrent_files.len());
        let selected = selected_torrent_count(torrent_files);
        let source_label = if url_sources == 1 {
            "source"
        } else {
            "sources"
        };
        let selected_file_label = if selected == 1 { "file" } else { "files" };
        if url_sources > 0 {
            return format!(
                "adds {url_sources} {source_label} and {selected} selected {selected_file_label} to {destination}"
            );
        }
        return format!("adds {selected} selected {selected_file_label} to {destination}");
    }

    match total_sources {
        0 => String::new(),
        1 => format!("adds 1 source to {destination}"),
        n => format!("adds {n} sources to {destination}"),
    }
}

fn add_button_label_for_context(
    addable_url_count: usize,
    archive_sets: usize,
    torrent_count: usize,
    selected_torrent_files: usize,
) -> String {
    if archive_sets > 0 {
        let normal_downloads = addable_url_count.saturating_sub(archive_sets);
        if normal_downloads > 0 || torrent_count > 0 {
            return "add archive set + downloads".to_string();
        }
        return "add archive set".to_string();
    }
    if torrent_count > 0 && addable_url_count == 0 {
        if selected_torrent_files > 0 {
            return "add selected files".to_string();
        }
        return "add torrent".to_string();
    }
    if addable_url_count == 1 && torrent_count == 0 {
        return "add download".to_string();
    }
    let total = addable_url_count + torrent_count;
    match total {
        0 | 1 => "add download".to_string(),
        n => format!("add {n} downloads"),
    }
}

fn archive_set_summary(urls: &[String], http_previews: &[AddHttpPreview]) -> (usize, usize) {
    let mut groups = std::collections::BTreeMap::<String, usize>::new();
    for url in urls {
        let filename = add_dialog_http_preview_name(url, http_previews)
            .unwrap_or_else(|| shio_core::extract_filename(url, None));
        if let Some(part) = shio_core::parse_archive_part(&filename) {
            *groups.entry(part.base_name).or_default() += 1;
        }
    }
    groups
        .values()
        .filter(|count| **count >= 2)
        .fold((0, 0), |(sets, files), count| (sets + 1, files + count))
}

struct ArchiveSetView {
    name: String,
    count: usize,
    urls: std::collections::HashSet<String>,
}

fn archive_sets_for_view(urls: &[String], http_previews: &[AddHttpPreview]) -> Vec<ArchiveSetView> {
    let mut groups = std::collections::BTreeMap::<String, Vec<String>>::new();
    for url in urls {
        let filename = add_dialog_http_preview_name(url, http_previews)
            .unwrap_or_else(|| shio_core::extract_filename(url, None));
        if let Some(part) = shio_core::parse_archive_part(&filename) {
            groups.entry(part.base_name).or_default().push(url.clone());
        }
    }
    groups
        .into_iter()
        .filter_map(|(name, urls)| {
            (urls.len() >= 2).then(|| ArchiveSetView {
                name,
                count: urls.len(),
                urls: urls.into_iter().collect(),
            })
        })
        .collect()
}

fn add_dialog_http_preview_name(url: &str, http_previews: &[AddHttpPreview]) -> Option<String> {
    http_previews.iter().find_map(|preview| {
        if preview.url != url {
            return None;
        }
        match &preview.state {
            AddHttpPreviewState::Ready(value) if !value.filename.is_empty() => {
                Some(value.filename.clone())
            },
            _ => None,
        }
    })
}

fn archive_set_validation_error(
    urls: &[String],
    http_previews: &[AddHttpPreview],
) -> Option<String> {
    let mut groups = std::collections::BTreeMap::<String, Vec<u32>>::new();
    let mut filenames_by_network_url = std::collections::BTreeMap::<String, String>::new();
    for url in urls {
        let filename = add_dialog_http_preview_name(url, http_previews)
            .unwrap_or_else(|| shio_core::extract_filename(url, None));
        let network_url = url_without_fragment(url);
        if let Some(existing) = filenames_by_network_url.insert(network_url, filename.clone())
            && existing != filename
        {
            return Some("multiple filenames point to the same URL".to_string());
        }
        if let Some(part) = shio_core::parse_archive_part(&filename) {
            groups
                .entry(part.base_name)
                .or_default()
                .push(part.part_number);
        }
    }
    for mut parts in groups.into_values().filter(|parts| parts.len() >= 2) {
        parts.sort_unstable();
        for (index, part) in parts.into_iter().enumerate() {
            let expected = u32::try_from(index + 1).ok()?;
            if part != expected {
                return Some(format!("missing part{expected:02}.rar"));
            }
        }
    }
    None
}

fn url_without_fragment(url: &str) -> String {
    match url::Url::parse(url) {
        Ok(mut parsed) => {
            parsed.set_fragment(None);
            parsed.to_string()
        },
        Err(_) => url.to_string(),
    }
}

fn torrent_file_matches(file: &shio_core::TorrentFile, query: &str) -> bool {
    let query = query.trim();
    if query.is_empty() {
        return true;
    }

    let query = query.to_ascii_lowercase();
    file.path
        .to_string_lossy()
        .to_ascii_lowercase()
        .contains(&query)
}

fn filtered_file_indices(torrent: &AddTorrentFile, query: &str) -> Vec<usize> {
    torrent
        .files
        .iter()
        .enumerate()
        .filter_map(|(index, file)| torrent_file_matches(file, query).then_some(index))
        .collect()
}

fn torrent_source_id(torrent: &AddTorrentFile) -> AddSourceId {
    match &torrent.source {
        crate::app::AddTorrentSource::File { path } => AddSourceId::TorrentFile(path.clone()),
        crate::app::AddTorrentSource::Magnet { request_id, .. } => {
            AddSourceId::MagnetPreview(*request_id)
        },
    }
}

fn torrent_meta_summary(torrents: &[(usize, &AddTorrentFile)]) -> Option<String> {
    let [(_, torrent)] = torrents else {
        return None;
    };
    let privacy = if torrent.is_private {
        "private"
    } else {
        "public"
    };
    let trackers = torrent.trackers.len();
    Some(format!(
        "{privacy} · {trackers} tracker{}",
        if trackers == 1 { "" } else { "s" }
    ))
}

fn torrent_review<'a>(
    torrent_files: &'a [AddTorrentFile],
    list_height: f32,
    search_query: &'a str,
    selected_source: Option<&'a AddSourceId>,
    source_filter: Option<&'a AddSourceId>,
    p: &'a crate::style::Palette,
) -> Element<'a, Message> {
    let visible_torrents = torrent_files
        .iter()
        .enumerate()
        .filter(|(_, torrent)| {
            source_filter.is_none_or(|source| torrent_source_id(torrent) == *source)
        })
        .collect::<Vec<_>>();
    let selected_count = visible_torrents
        .iter()
        .map(|(_, torrent)| torrent.selected_count())
        .sum::<usize>();
    let total_count = visible_torrents
        .iter()
        .map(|(_, torrent)| torrent.files.len())
        .sum::<usize>();
    let selected_size = bytesize::ByteSize(
        visible_torrents
            .iter()
            .map(|(_, torrent)| torrent.selected_size())
            .sum::<u64>(),
    )
    .to_string();

    let meta = torrent_meta_summary(&visible_torrents);
    let mut header = column![
        text("files").size(12).color(p.text_primary),
        Space::new().height(4),
        text(format!(
            "{selected_count}/{total_count} selected · {selected_size}"
        ))
        .size(11)
        .color(p.text_tertiary),
    ];
    if let Some(meta) = meta {
        header = header.push(text(meta).size(11).color(p.text_tertiary));
    }

    let search: Element<'_, Message> =
        if total_count > TORRENT_SEARCH_THRESHOLD || !search_query.trim().is_empty() {
            let input = text_input("filter files", search_query)
                .id(crate::app::ADD_TORRENT_SEARCH_INPUT_ID.clone())
                .on_input(Message::AddTorrentSearchChanged)
                .style(style::input(p))
                .size(12)
                .padding([6, 10])
                .width(Length::Fill);

            if search_query.is_empty() {
                row![input].into()
            } else {
                row![
                    input,
                    button(iced_fonts::bootstrap::x_lg().size(10))
                        .style(style::btn_icon(p))
                        .on_press(Message::AddTorrentSearchCleared)
                        .padding(6),
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center)
                .into()
            }
        } else {
            Space::new().height(0).into()
        };

    let mut groups = column![].spacing(12);
    for (torrent_index, torrent) in visible_torrents {
        let source_id = torrent_source_id(torrent);
        let title_color = if selected_source == Some(&source_id) {
            p.accent
        } else {
            p.text_primary
        };
        let header_controls = row![
            text(format!(
                "{}/{}",
                torrent.selected_count(),
                torrent.files.len()
            ))
            .size(11)
            .color(p.text_tertiary),
            button(text("all").size(11))
                .style(style::btn_ghost(p))
                .on_press(Message::AddTorrentFilesSelectionChanged {
                    torrent_index,
                    selected: true,
                })
                .padding([4, 8]),
            button(text("none").size(11))
                .style(style::btn_ghost(p))
                .on_press(Message::AddTorrentFilesSelectionChanged {
                    torrent_index,
                    selected: false,
                })
                .padding([4, 8]),
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center);

        let torrent_header = row![
            text(&torrent.display_name)
                .size(12)
                .color(title_color)
                .wrapping(Wrapping::WordOrGlyph)
                .width(Length::Fill),
            header_controls,
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center);

        let mut files =
            column![mouse_area(torrent_header).on_press(Message::AddSourceSelected(source_id,))]
                .spacing(4);
        if !search_query.trim().is_empty() {
            files = files.push(
                row![
                    Space::new().width(Length::Fill),
                    button(text("select matching").size(11))
                        .style(style::btn_ghost(p))
                        .on_press(Message::AddTorrentMatchingSelectionChanged {
                            torrent_index,
                            selected: true,
                        })
                        .padding([4, 8]),
                    button(text("clear matching").size(11))
                        .style(style::btn_ghost(p))
                        .on_press(Message::AddTorrentMatchingSelectionChanged {
                            torrent_index,
                            selected: false,
                        })
                        .padding([4, 8]),
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center),
            );
        }

        let filtered_indices = filtered_file_indices(torrent, search_query);
        let rendered_indices = filtered_indices
            .iter()
            .copied()
            .take(TORRENT_FILE_RENDER_LIMIT)
            .collect::<Vec<_>>();

        for file_index in rendered_indices {
            let Some(file) = torrent.files.get(file_index) else {
                continue;
            };
            let path = file.path.to_string_lossy();
            files = files.push(
                row![
                    checkbox(file.selected)
                        .on_toggle(move |selected| Message::AddTorrentFileToggled {
                            torrent_index,
                            file_index,
                            selected,
                        })
                        .size(14),
                    text(path)
                        .size(11)
                        .color(p.text_secondary)
                        .wrapping(Wrapping::WordOrGlyph)
                        .width(Length::Fill),
                    text(bytesize::ByteSize(file.size).to_string())
                        .size(11)
                        .color(p.text_tertiary),
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center),
            );
        }

        if filtered_indices.is_empty() {
            files = files.push(
                text(format!("no files match \"{}\"", search_query.trim()))
                    .size(11)
                    .color(p.text_tertiary),
            );
        } else if filtered_indices.len() > TORRENT_FILE_RENDER_LIMIT {
            let noun = if search_query.trim().is_empty() {
                "files"
            } else {
                "matches"
            };
            files = files.push(
                text(format!(
                    "showing first {} of {} {noun}",
                    TORRENT_FILE_RENDER_LIMIT,
                    filtered_indices.len()
                ))
                .size(11)
                .color(p.text_tertiary),
            );
        }

        groups = groups.push(files);
    }

    column![
        container(header).padding(Padding::default().bottom(10)),
        search,
        scrollable(container(groups).padding([10, 12]))
            .height(Length::Fixed(list_height))
            .style(style::scrollable_style(p)),
    ]
    .spacing(0)
    .into()
}

fn sources_pane<'a>(
    urls: &'a text_editor::Content,
    p: &'a crate::style::Palette,
) -> Element<'a, Message> {
    let url_input = text_editor(urls)
        .on_action(Message::AddUrlsAction)
        .placeholder("urls or magnets, one per line")
        .style(style::text_editor_style(p))
        .size(13)
        .padding([8, 12])
        .height(URL_INPUT_HEIGHT);

    let torrent_picker = button(
        row![
            iced_fonts::bootstrap::plus().size(12),
            text(".torrent").size(12)
        ]
        .spacing(6)
        .align_y(iced::Alignment::Center),
    )
    .style(style::btn_secondary(p))
    .on_press(Message::PickTorrentFiles)
    .padding([7, 10]);

    column![
        label_text("sources", p),
        url_input,
        row![
            helper_text("one per line, or drop torrent files.", p),
            Space::new().width(Length::Fill),
            torrent_picker,
        ]
        .align_y(iced::Alignment::Center),
    ]
    .spacing(6)
    .into()
}

fn filename_row<'a>(
    single_http_source: bool,
    single_url_name: Option<&'a str>,
    filename: &'a str,
    p: &'a crate::style::Palette,
) -> Option<Element<'a, Message>> {
    if !single_http_source {
        return None;
    }

    let placeholder = single_url_name.unwrap_or("detected name");
    let name_input = text_input(placeholder, filename)
        .on_input(Message::AddFilenameChanged)
        .style(style::input(p))
        .size(13)
        .padding([7, 10])
        .width(Length::Fill);
    Some(settings_row(
        "name",
        helper_text("optional filename.", p),
        name_input.into(),
        p,
    ))
}

fn folder_name_row<'a>(
    total_sources: usize,
    create_subfolder: bool,
    subfolder_name: &'a str,
    p: &'a crate::style::Palette,
) -> Option<Element<'a, Message>> {
    if total_sources == 0 || !create_subfolder {
        return None;
    }

    let folder_input = text_input("folder name", subfolder_name)
        .on_input(Message::AddSubfolderNameChanged)
        .style(style::input(p))
        .size(13)
        .padding([7, 10])
        .width(Length::Fill);

    Some(settings_row(
        "folder name",
        helper_text("optional name override.", p),
        folder_input.into(),
        p,
    ))
}

const fn source_row_text_color(selected: bool, p: &crate::style::Palette) -> iced::Color {
    if selected { p.accent } else { p.text_secondary }
}

fn source_inbox<'a>(
    url_entries: &'a [String],
    url_preview: &'a [String],
    http_previews: &'a [AddHttpPreview],
    magnet_previews: &'a [AddMagnetPreview],
    torrent_files: &'a [AddTorrentFile],
    selected_source: Option<&'a AddSourceId>,
    p: &'a crate::style::Palette,
) -> Element<'a, Message> {
    let mut rows = column![].spacing(6);
    let archive_sets = archive_sets_for_view(url_entries, http_previews);
    let grouped_urls: std::collections::HashSet<&str> = archive_sets
        .iter()
        .flat_map(|set| set.urls.iter().map(String::as_str))
        .collect();

    for set in &archive_sets {
        let name = set.name.clone();
        let count = set.count;
        rows = rows.push(
            row![
                text("archive set")
                    .size(10)
                    .color(p.text_tertiary)
                    .width(Length::Fixed(82.0)),
                text(name)
                    .size(12)
                    .color(p.text_secondary)
                    .wrapping(Wrapping::WordOrGlyph)
                    .width(Length::Fill),
                text(format!("{count} parts"))
                    .size(10)
                    .color(p.text_tertiary),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center),
        );
    }

    for (index, url) in url_entries.iter().enumerate() {
        if grouped_urls.contains(url.as_str()) {
            continue;
        }
        let name = url_preview
            .get(index)
            .cloned()
            .unwrap_or_else(|| url_display_name(url, url_entries, url_preview));
        let source_id = AddSourceId::Url(index);
        let selected = selected_source == Some(&source_id);
        rows = rows.push(
            mouse_area(
                row![
                    text(source_status_label(url, http_previews, magnet_previews))
                        .size(10)
                        .color(p.text_tertiary)
                        .width(Length::Fixed(82.0)),
                    text(name)
                        .size(12)
                        .color(source_row_text_color(selected, p))
                        .wrapping(Wrapping::WordOrGlyph)
                        .width(Length::Fill),
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center),
            )
            .on_press(Message::AddSourceSelected(source_id)),
        );
    }

    for torrent in torrent_files {
        let source_id = torrent_source_id(torrent);
        let selected = selected_source == Some(&source_id);
        rows = rows.push(
            mouse_area(
                row![
                    text("torrent")
                        .size(10)
                        .color(p.text_tertiary)
                        .width(Length::Fixed(82.0)),
                    text(&torrent.display_name)
                        .size(12)
                        .color(source_row_text_color(selected, p))
                        .wrapping(Wrapping::WordOrGlyph)
                        .width(Length::Fill),
                    text(format!(
                        "{}/{} files",
                        torrent.selected_count(),
                        torrent.files.len()
                    ))
                    .size(10)
                    .color(p.text_tertiary),
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center),
            )
            .on_press(Message::AddSourceSelected(source_id)),
        );
    }

    if url_entries.is_empty() && torrent_files.is_empty() {
        rows = rows.push(text("no sources").size(11).color(p.text_tertiary));
    }

    column![
        label_text("source inbox", p),
        scrollable(container(rows).padding([8, 10]))
            .height(Length::Fixed(SOURCE_INBOX_HEIGHT))
            .style(style::scrollable_style(p)),
    ]
    .spacing(8)
    .into()
}

fn source_status_label(
    url: &str,
    http_previews: &[AddHttpPreview],
    magnet_previews: &[AddMagnetPreview],
) -> &'static str {
    if url.trim_start().to_ascii_lowercase().starts_with("magnet:") {
        return magnet_previews
            .iter()
            .find(|preview| preview.magnet == url)
            .map_or("ready", |preview| match &preview.state {
                AddMagnetPreviewState::Resolving => "fetching metadata",
                AddMagnetPreviewState::Failed { .. } => "file list unavailable",
            });
    }

    http_previews
        .iter()
        .find(|preview| preview.url == url)
        .map_or("ready", |preview| match &preview.state {
            AddHttpPreviewState::Resolving => "checking",
            AddHttpPreviewState::Ready(_) => "ready",
            AddHttpPreviewState::Blocked { .. } => "blocked",
            AddHttpPreviewState::Failed { .. } => "preview failed",
        })
}

fn url_display_name(url: &str, url_entries: &[String], url_preview: &[String]) -> String {
    url_entries
        .iter()
        .position(|entry| entry == url)
        .and_then(|index| url_preview.get(index))
        .cloned()
        .unwrap_or_else(|| {
            if url.trim_start().to_ascii_lowercase().starts_with("magnet:") {
                "magnet link".to_string()
            } else {
                shio_core::extract_filename(url, None)
            }
        })
}

fn url_status_detail(
    url: &str,
    http_previews: &[AddHttpPreview],
    magnet_previews: &[AddMagnetPreview],
) -> String {
    if url.trim_start().to_ascii_lowercase().starts_with("magnet:") {
        return magnet_previews
            .iter()
            .find(|preview| preview.magnet == url)
            .map_or_else(
                || "ready".to_string(),
                |preview| match &preview.state {
                    AddMagnetPreviewState::Resolving => "fetching metadata".to_string(),
                    AddMagnetPreviewState::Failed { message } if message.is_empty() => {
                        "file list unavailable".to_string()
                    },
                    AddMagnetPreviewState::Failed { message } => {
                        format!("file list unavailable: {message}")
                    },
                },
            );
    }

    http_previews
        .iter()
        .find(|preview| preview.url == url)
        .map_or_else(
            || "ready".to_string(),
            |preview| match &preview.state {
                AddHttpPreviewState::Resolving => "checking".to_string(),
                AddHttpPreviewState::Ready(_) => "ready".to_string(),
                AddHttpPreviewState::Blocked { reason } if reason.is_empty() => {
                    "blocked".to_string()
                },
                AddHttpPreviewState::Blocked { reason } => format!("blocked: {reason}"),
                AddHttpPreviewState::Failed { message } if message.is_empty() => {
                    "preview failed".to_string()
                },
                AddHttpPreviewState::Failed { message } => format!("preview failed: {message}"),
            },
        )
}

fn url_source_inspector<'a>(
    url_entries: &'a [String],
    url_preview: &'a [String],
    url: &'a str,
    http_previews: &'a [AddHttpPreview],
    magnet_previews: &'a [AddMagnetPreview],
    p: &'a crate::style::Palette,
) -> Element<'a, Message> {
    let name = url_display_name(url, url_entries, url_preview);
    let status = url_status_detail(url, http_previews, magnet_previews);
    let kind = if url.trim_start().to_ascii_lowercase().starts_with("magnet:") {
        "magnet"
    } else {
        "url"
    };
    let preview_detail: Element<'_, Message> = http_previews
        .iter()
        .find(|preview| preview.url == url)
        .and_then(|preview| match &preview.state {
            AddHttpPreviewState::Ready(preview) => {
                let size = preview.total_size.map_or_else(
                    || "unknown size".to_string(),
                    |size| bytesize::ByteSize(size).to_string(),
                );
                let mode = if preview.accept_ranges {
                    "range requests"
                } else {
                    "single stream"
                };
                let content_type = preview
                    .content_type
                    .as_deref()
                    .filter(|value| !value.is_empty())
                    .unwrap_or("unknown type");
                Some(
                    column![
                        Space::new().height(12),
                        text("preview").size(10).color(p.text_tertiary),
                        text(format!("{size} · {mode} · {content_type}"))
                            .size(11)
                            .color(p.text_secondary)
                            .wrapping(Wrapping::WordOrGlyph),
                    ]
                    .spacing(2)
                    .into(),
                )
            },
            _ => None,
        })
        .unwrap_or_else(|| Space::new().height(0).into());

    column![
        text("source").size(12).color(p.text_primary),
        Space::new().height(4),
        text(status).size(11).color(p.text_tertiary),
        Space::new().height(16),
        text(kind).size(10).color(p.text_tertiary),
        text(name)
            .size(13)
            .color(p.text_secondary)
            .wrapping(Wrapping::WordOrGlyph),
        Space::new().height(12),
        text("address").size(10).color(p.text_tertiary),
        text(url)
            .size(11)
            .color(p.text_secondary)
            .wrapping(Wrapping::WordOrGlyph),
        preview_detail,
    ]
    .spacing(2)
    .into()
}

fn empty_inspector(p: &crate::style::Palette) -> Element<'_, Message> {
    column![
        text("inspector").size(12).color(p.text_primary),
        Space::new().height(4),
        text("select a source").size(11).color(p.text_tertiary),
    ]
    .into()
}

fn review_pane<'a>(
    url_entries: &'a [String],
    url_preview: &'a [String],
    http_previews: &'a [AddHttpPreview],
    magnet_previews: &'a [AddMagnetPreview],
    torrent_files: &'a [AddTorrentFile],
    torrent_search: &'a str,
    selected_source: Option<&'a AddSourceId>,
    p: &'a crate::style::Palette,
) -> Element<'a, Message> {
    let content = match selected_source {
        Some(AddSourceId::Url(index)) => url_entries.get(*index).map_or_else(
            || empty_inspector(p),
            |url| {
                url_source_inspector(
                    url_entries,
                    url_preview,
                    url,
                    http_previews,
                    magnet_previews,
                    p,
                )
            },
        ),
        Some(source @ (AddSourceId::TorrentFile(_) | AddSourceId::MagnetPreview(_))) => {
            torrent_review(
                torrent_files,
                REVIEW_LIST_HEIGHT,
                torrent_search,
                selected_source,
                Some(source),
                p,
            )
        },
        None if !torrent_files.is_empty() => torrent_review(
            torrent_files,
            REVIEW_LIST_HEIGHT,
            torrent_search,
            selected_source,
            None,
            p,
        ),
        None => empty_inspector(p),
    };

    container(content)
        .padding(Padding::default().top(18).right(18).bottom(18).left(18))
        .width(Length::Fixed(RIGHT_PANE_WIDTH))
        .height(Length::Fill)
        .into()
}

#[expect(
    clippy::too_many_arguments,
    reason = "add-dialog view composition keeps state explicit"
)]
fn setup_pane<'a>(
    urls: &'a text_editor::Content,
    url_entries: &'a [String],
    url_preview: &'a [String],
    http_previews: &'a [AddHttpPreview],
    magnet_previews: &'a [AddMagnetPreview],
    torrent_files: &'a [AddTorrentFile],
    selected_source: Option<&'a AddSourceId>,
    total_sources: usize,
    single_http_source: bool,
    single_url_name: Option<&'a str>,
    filename: &'a str,
    create_subfolder: bool,
    subfolder_name: &'a str,
    p: &'a crate::style::Palette,
) -> Element<'a, Message> {
    let mut form = column![
        sources_pane(urls, p),
        source_inbox(
            url_entries,
            url_preview,
            http_previews,
            magnet_previews,
            torrent_files,
            selected_source,
            p,
        ),
        divider(p),
    ]
    .spacing(10)
    .width(Length::Fixed(LEFT_PANE_WIDTH))
    .height(Length::Fill)
    .padding(Padding::default().top(14).right(20).bottom(14).left(20));

    if let Some(row) = filename_row(single_http_source, single_url_name, filename, p) {
        form = form.push(row);
    }

    if let Some(row) = folder_name_row(total_sources, create_subfolder, subfolder_name, p) {
        form = form.push(row);
    }

    form.into()
}

pub(crate) fn view<'a>(
    model: &AddDialogViewModel<'a>,
    p: &'a crate::style::Palette,
    material: shio_core::WindowMaterialPreference,
    base: Element<'a, Message>,
) -> Element<'a, Message> {
    let total_sources = model.url_count + model.torrent_files.len();
    let addable_sources = model.addable_url_count + model.torrent_files.len();
    let single_http_source =
        addable_sources == 1 && model.addable_url_count == 1 && model.torrent_files.is_empty();
    let source_summary = source_summary(
        model.http_count,
        model.magnet_count,
        model.torrent_files.len(),
    );

    let header = row![
        column![
            text("add download").size(18).color(p.text_primary),
            text(source_summary).size(11).color(p.text_tertiary),
        ]
        .spacing(3)
        .width(Length::Fill),
        button(iced_fonts::bootstrap::x_lg().size(12))
            .style(style::btn_icon(p))
            .on_press(Message::CancelAddDownload)
            .padding(6),
    ]
    .align_y(iced::Alignment::Center);

    let setup = setup_pane(
        model.urls,
        model.url_entries,
        model.url_preview,
        model.http_previews,
        model.magnet_previews,
        model.torrent_files,
        model.selected_source,
        addable_sources,
        single_http_source,
        model.single_url_name,
        model.filename,
        model.create_subfolder,
        model.subfolder_name,
        p,
    );

    let review = review_pane(
        model.url_entries,
        model.url_preview,
        model.http_previews,
        model.magnet_previews,
        model.torrent_files,
        model.torrent_search,
        model.selected_source,
        p,
    );

    let body = row![setup, vertical_divider(p), review]
        .height(Length::Fill)
        .width(Length::Fill);

    let cancel = button(text("cancel").size(13))
        .style(style::btn_ghost(p))
        .on_press(Message::CancelAddDownload)
        .padding([8, 16]);

    let (archive_sets, archive_files) = archive_set_summary(model.url_entries, model.http_previews);
    let add_label = add_button_label_for_context(
        model.addable_url_count,
        archive_sets,
        model.torrent_files.len(),
        selected_torrent_count(model.torrent_files),
    );
    let disabled_reason = add_disabled_reason(
        addable_sources,
        model.url_entries,
        model.http_previews,
        model.torrent_files,
    );
    let can_add = disabled_reason.is_none();
    let mut add_btn = button(text(add_label).size(13))
        .style(style::btn_primary(p))
        .padding([8, 16]);
    if can_add {
        add_btn = add_btn.on_press(Message::ConfirmAddDownload);
    }

    let mut footer_status = column![].spacing(3).width(Length::Fill);
    let summary = result_summary(
        addable_sources,
        archive_sets,
        archive_files,
        model.torrent_files,
        model.save_path,
        model.create_subfolder,
        model.subfolder_name,
    );
    if !summary.is_empty() {
        footer_status = footer_status.push(text(summary).size(11).color(p.text_tertiary));
    }
    if model.has_archive_url {
        footer_status = footer_status.push(
            text("archive settings apply")
                .size(11)
                .color(p.text_tertiary),
        );
    }
    if total_sources > 0
        && let Some(reason) = disabled_reason
    {
        footer_status = footer_status.push(text(reason).size(11).color(p.error));
    }

    let buttons = row![footer_status, cancel, Space::new().width(8), add_btn]
        .align_y(iced::Alignment::Center);

    let footer = container(buttons)
        .padding(Padding::default().top(14).right(20).bottom(18).left(20))
        .width(Length::Fill);

    let form = column![
        container(header)
            .padding(Padding::default().top(18).right(20).bottom(14).left(22))
            .width(Length::Fill),
        divider(p),
        container(body).height(Length::Fill).width(Length::Fill),
        divider(p),
        footer,
    ]
    .width(Length::Fill)
    .height(Length::Fill);

    let card = container(form)
        .style(style::modal_card(p, material))
        .width(MODAL_WIDTH)
        .height(MODAL_HEIGHT);

    let overlay = mouse_area(center(opaque(card))).on_press(Message::CancelAddDownload);

    let backdrop = container(overlay)
        .style(style::modal_backdrop(p))
        .width(Length::Fill)
        .height(Length::Fill);

    stack![base, backdrop].into()
}

#[cfg(test)]
mod tests {
    use super::{effective_destination, result_summary};

    #[test]
    fn effective_destination_appends_subfolder_when_enabled() {
        let result = effective_destination("D:\\Downloads", true, "archive");

        assert_eq!(result, "D:\\Downloads\\archive");
    }

    #[test]
    fn effective_destination_sanitizes_subfolder_when_enabled() {
        let result = effective_destination("D:\\Downloads", true, " bad/name ");

        assert_eq!(result, "D:\\Downloads\\bad_name");
    }

    #[test]
    fn effective_destination_omits_empty_subfolder() {
        let result = effective_destination("D:\\Downloads", true, " ");

        assert_eq!(result, "D:\\Downloads");
    }

    #[test]
    fn result_summary_is_empty_without_sources() {
        let result = result_summary(0, 0, 0, &[], "D:\\Downloads", false, "");

        assert_eq!(result, "");
    }
}
