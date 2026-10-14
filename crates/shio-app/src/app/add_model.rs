use super::{MAX_NAME_TEXT_BYTES, MAX_PATH_TEXT_BYTES, truncate_text_bytes};
use crate::link::{is_link, is_magnet};
use iced::widget::text_editor;
use shio_core::{
    AppConfig, HttpArchivePartRequest, HttpArchiveSetRequest, HttpDownloadRequest,
    HttpPreviewResult, HttpPreviewState, MagnetPreviewResult, TorrentDownloadRequest, TorrentFile,
    TorrentSource,
};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

const MAX_LINKS: usize = 256;
const MAX_TORRENTS: usize = 32;
const HTTP_PREVIEW_LIMIT: usize = 8;
pub(super) const MAX_URL_TEXT_BYTES: usize = 64 * 1024;

pub(crate) struct AddDialog {
    pub(crate) urls: text_editor::Content,
    pub(crate) links: Vec<Link>,
    pub(crate) torrents: Vec<TorrentPick>,
    pub(crate) selected_torrent: Option<u64>,
    pub(crate) torrent_search: String,
    pub(crate) filename: String,
    pub(crate) save_path: String,
    pub(crate) create_subfolder: bool,
    pub(crate) subfolder: String,
    pub(crate) auto_extract: bool,
    pub(crate) submitting: bool,
    pub(crate) failures: Vec<(String, String)>,
    pub(crate) validation: AddValidation,
    subfolder_edited: bool,
    too_many: bool,
    segments: u8,
    plan: Vec<Item>,
    submitted: Vec<(String, Source)>,
}

pub(crate) struct Link {
    pub(crate) url: String,
    pub(crate) name: String,
    pub(crate) state: LinkState,
    pub(crate) mirror: bool,
    request: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LinkState {
    Unchecked,
    Resolving,
    Ready { size: Option<u64> },
    Blocked(String),
    Failed(String),
}

#[derive(Clone)]
pub(crate) struct TorrentPick {
    pub(crate) id: u64,
    pub(crate) name: String,
    pub(crate) files: Vec<TorrentFile>,
    source: PickSource,
    bytes: Vec<u8>,
}

#[derive(Clone)]
enum PickSource {
    File(PathBuf),
    Magnet(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AddValidation {
    Empty,
    Invalid(String),
    Ready(String),
}

enum Item {
    Http {
        url: String,
        name: String,
        mirrors: Vec<String>,
    },
    ArchiveSet {
        name: String,
        request: HttpArchiveSetRequest,
        urls: Vec<String>,
    },
    Magnet(String),
    Torrent(u64),
}

enum Source {
    Urls(Vec<String>),
    Torrent(u64),
}

pub(super) enum AddRequest {
    Http(HttpDownloadRequest),
    ArchiveSet(HttpArchiveSetRequest),
    Torrent(TorrentDownloadRequest),
}

#[derive(Default)]
pub(super) struct Previews {
    pub(super) http: Vec<(u64, String)>,
    pub(super) magnets: Vec<(u64, String)>,
    pub(super) cancel: Vec<u64>,
}

impl TorrentPick {
    pub(super) fn from_file(path: PathBuf, bytes: Vec<u8>) -> Result<Self, String> {
        let manifest = shio_core::parse_torrent_manifest(&bytes)
            .map_err(|error| format!("{}: {}", file_label(&path), error.short_label()))?;
        Ok(Self {
            id: 0,
            name: manifest.name,
            files: manifest.files,
            source: PickSource::File(path),
            bytes,
        })
    }

    pub(crate) fn selected_count(&self) -> usize {
        self.files.iter().filter(|file| file.selected).count()
    }

    pub(crate) fn selected_size(&self) -> u64 {
        self.files
            .iter()
            .filter(|file| file.selected)
            .fold(0, |total, file| total.saturating_add(file.size))
    }

    fn magnet(&self) -> Option<&str> {
        match &self.source {
            PickSource::Magnet(magnet) => Some(magnet),
            PickSource::File(_) => None,
        }
    }

    fn request(&self, save_path: PathBuf, auto_extract: bool) -> TorrentDownloadRequest {
        let request = match &self.source {
            PickSource::File(_) => {
                TorrentDownloadRequest::new(TorrentSource::File(self.bytes.clone()), save_path)
            },
            PickSource::Magnet(magnet) => {
                TorrentDownloadRequest::new(TorrentSource::Magnet(magnet.clone()), save_path)
                    .with_metadata_bytes(self.bytes.clone())
            },
        }
        .with_files(self.files.clone())
        .with_auto_extract(auto_extract);
        let mut selected = self.files.iter().filter(|file| file.selected);
        match (selected.next(), selected.next()) {
            (Some(only), None) => match only.path.file_name().and_then(|name| name.to_str()) {
                Some(name) => request.with_filename(shio_core::sanitize_filename(name)),
                None => request,
            },
            _ => request,
        }
    }
}

impl AddDialog {
    pub(super) fn new(config: &AppConfig) -> Self {
        Self {
            urls: text_editor::Content::new(),
            links: Vec::new(),
            torrents: Vec::new(),
            selected_torrent: None,
            torrent_search: String::new(),
            filename: String::new(),
            save_path: truncate_text_bytes(
                config.download_dir.to_string_lossy().into_owned(),
                MAX_PATH_TEXT_BYTES,
            ),
            create_subfolder: config.default_create_subfolder,
            subfolder: String::new(),
            auto_extract: config.default_auto_extract,
            submitting: false,
            failures: Vec::new(),
            validation: AddValidation::Empty,
            subfolder_edited: false,
            too_many: false,
            segments: config.default_segments,
            plan: Vec::new(),
            submitted: Vec::new(),
        }
    }

    pub(crate) fn single_http(&self) -> bool {
        self.torrents.is_empty() && matches!(self.links.as_slice(), [link] if !is_magnet(&link.url))
    }

    pub(super) fn resolving(&self) -> impl Iterator<Item = u64> + '_ {
        self.links
            .iter()
            .filter(|link| link.state == LinkState::Resolving)
            .filter_map(|link| link.request)
    }

    pub(super) fn append_text(&mut self, text: &str) {
        let mut urls = self.urls.text();
        if !urls.trim().is_empty() {
            urls.push('\n');
        }
        urls.push_str(text);
        self.urls = text_editor::Content::with_text(&truncate_text_bytes(urls, MAX_URL_TEXT_BYTES));
    }

    pub(super) fn set_subfolder(&mut self, name: String) {
        self.subfolder = truncate_text_bytes(name, MAX_NAME_TEXT_BYTES);
        self.subfolder_edited = true;
        self.revalidate();
    }

    pub(super) fn sync_links(&mut self, next_request: &mut u64) -> Previews {
        let mut text = self.urls.text();
        let too_long = text.len() > MAX_URL_TEXT_BYTES;
        if too_long {
            text = truncate_text_bytes(text, MAX_URL_TEXT_BYTES);
            self.urls = text_editor::Content::with_text(&text);
        }
        let mut seen = HashSet::new();
        let lines: Vec<&str> = text
            .lines()
            .map(str::trim)
            .filter(|line| is_link(line) && seen.insert(*line))
            .collect();
        self.torrents.retain(|torrent| {
            torrent
                .magnet()
                .is_none_or(|magnet| lines.contains(&magnet))
        });
        let preview_http =
            lines.iter().filter(|line| !is_magnet(line)).count() <= HTTP_PREVIEW_LIMIT;
        let mut previous = std::mem::take(&mut self.links);
        let kept = |line: &str| previous.iter().find(|link| link.url == line);
        let mut previewed_hosts: HashSet<String> = lines
            .iter()
            .filter_map(|line| kept(line))
            .filter(|link| {
                matches!(
                    link.state,
                    LinkState::Ready { .. } | LinkState::Blocked(_) | LinkState::Failed(_)
                )
            })
            .filter_map(|link| host(&link.url))
            .collect();
        // In-flight previews claim their hosts first, so an inserted line never restarts one.
        let mut in_flight = HashSet::new();
        for link in lines.iter().take(MAX_LINKS).filter_map(|line| kept(line)) {
            if link.state == LinkState::Resolving
                && (preview_http || host(&link.url).is_some_and(|h| previewed_hosts.insert(h)))
            {
                in_flight.extend(link.request);
            }
        }
        let mut preview_host = |line: &str| {
            preview_http
                || host(line).is_some_and(|host| {
                    previewed_hosts.len() < HTTP_PREVIEW_LIMIT && previewed_hosts.insert(host)
                })
        };

        let mut previews = Previews::default();
        for &line in lines.iter().take(MAX_LINKS) {
            if self
                .torrents
                .iter()
                .any(|torrent| torrent.magnet() == Some(line))
            {
                continue;
            }
            let magnet = is_magnet(line);
            if let Some(index) = previous.iter().position(|link| link.url == line) {
                let mut link = previous.swap_remove(index);
                if !magnet {
                    match link.state {
                        LinkState::Resolving
                            if !link.request.is_some_and(|id| in_flight.contains(&id)) =>
                        {
                            previews.cancel.extend(link.request.take());
                            link.state = LinkState::Unchecked;
                        },
                        LinkState::Unchecked if preview_host(line) => {
                            *next_request += 1;
                            link.request = Some(*next_request);
                            link.state = LinkState::Resolving;
                            previews.http.push((*next_request, line.to_owned()));
                        },
                        _ => {},
                    }
                }
                self.links.push(link);
                continue;
            }
            let request = (magnet || preview_host(line)).then(|| {
                *next_request += 1;
                *next_request
            });
            if let Some(id) = request {
                let queue = if magnet {
                    &mut previews.magnets
                } else {
                    &mut previews.http
                };
                queue.push((id, line.to_owned()));
            }
            self.links.push(Link {
                url: line.to_owned(),
                name: if magnet {
                    "magnet".to_owned()
                } else {
                    shio_core::extract_filename(line)
                },
                state: if request.is_some() {
                    LinkState::Resolving
                } else {
                    LinkState::Unchecked
                },
                mirror: false,
                request,
            });
        }
        previews.cancel.extend(
            previous
                .into_iter()
                .filter(|link| link.state == LinkState::Resolving)
                .filter_map(|link| link.request),
        );
        self.too_many = too_long || lines.len() > MAX_LINKS;
        self.revalidate();
        previews
    }

    pub(super) fn apply_http_preview(
        &mut self,
        result: HttpPreviewResult,
        next_request: &mut u64,
    ) -> Previews {
        let Some(link) = self.links.iter_mut().find(|link| {
            link.request == Some(result.request_id) && link.state == LinkState::Resolving
        }) else {
            return Previews::default();
        };
        let state = match result.state {
            HttpPreviewState::Ready(preview) => {
                if !preview.filename.is_empty() {
                    link.name = preview.filename;
                }
                LinkState::Ready {
                    size: preview.total_size,
                }
            },
            HttpPreviewState::Folder { files } => {
                link.state = LinkState::Ready { size: None };
                let url = link.url.clone();
                return self.expand_folder(&url, &files, next_request);
            },
            HttpPreviewState::Blocked { reason } => LinkState::Blocked(reason),
            HttpPreviewState::Error { message } => LinkState::Failed(message),
        };
        link.state = state;
        self.revalidate();
        Previews::default()
    }

    fn expand_folder(&mut self, url: &str, files: &[String], next_request: &mut u64) -> Previews {
        let Ok(folder) = url::Url::parse(url) else {
            return Previews::default();
        };
        let expanded: Vec<String> = files
            .iter()
            .map(|name| {
                let mut file = folder.clone();
                file.query_pairs_mut().append_pair("file", name);
                // `set_fragment` leaves `%` alone; the fragment only names the line.
                file.set_fragment(Some(&name.replace('%', "%25")));
                file.into()
            })
            .collect();
        let expanded = expanded.join("\n");
        let text = self.urls.text();
        let lines: Vec<&str> = text
            .lines()
            .map(|line| {
                if line.trim() == url {
                    expanded.as_str()
                } else {
                    line
                }
            })
            .collect();
        self.urls = text_editor::Content::with_text(&lines.join("\n"));
        self.sync_links(next_request)
    }

    fn mark_mirrors(&mut self) {
        let blocked_hosts: HashSet<String> = self
            .links
            .iter()
            .filter(|link| matches!(link.state, LinkState::Blocked(_)))
            .filter_map(|link| host(&link.url))
            .collect();
        let ranks: Vec<(bool, bool)> = self
            .links
            .iter()
            .map(|link| {
                let unpreviewed = matches!(link.state, LinkState::Unchecked | LinkState::Resolving);
                let blocked = matches!(link.state, LinkState::Blocked(_))
                    || (unpreviewed
                        && host(&link.url).is_some_and(|host| blocked_hosts.contains(&host)));
                (blocked, matches!(link.state, LinkState::Failed(_)))
            })
            .collect();
        let mut order: Vec<usize> = (0..self.links.len()).collect();
        order.sort_by_key(|&index| ranks[index]);
        let mut seen = HashSet::new();
        for index in order {
            let link = &mut self.links[index];
            link.mirror = !is_magnet(&link.url)
                && shio_core::parse_archive_part(&link.name)
                    .is_some_and(|part| !seen.insert((part.base_name, part.part_number)));
        }
    }

    pub(super) fn apply_magnet_preview(&mut self, result: MagnetPreviewResult) {
        let Some(index) = self.links.iter().position(|link| {
            link.request == Some(result.request_id)
                && link.url == result.magnet
                && link.state == LinkState::Resolving
        }) else {
            return;
        };
        match result.result {
            Ok(manifest) => {
                self.links.remove(index);
                self.torrents.push(TorrentPick {
                    id: result.request_id,
                    name: manifest.name,
                    files: manifest.files,
                    source: PickSource::Magnet(result.magnet),
                    bytes: manifest.metadata_bytes,
                });
                self.selected_torrent.get_or_insert(result.request_id);
            },
            Err(message) => self.links[index].state = LinkState::Failed(message),
        }
        self.revalidate();
    }

    pub(super) fn add_torrents(
        &mut self,
        picks: Vec<Result<TorrentPick, String>>,
        next_request: &mut u64,
    ) -> Vec<String> {
        let mut problems = Vec::new();
        for pick in picks {
            match pick {
                Ok(_) if self.torrents.len() == MAX_TORRENTS => {
                    problems.push(format!("up to {MAX_TORRENTS} torrents at once"));
                    break;
                },
                Ok(mut pick) => {
                    let duplicate =
                        self.torrents
                            .iter()
                            .any(|torrent| match (&torrent.source, &pick.source) {
                                (PickSource::File(a), PickSource::File(b)) => a == b,
                                _ => false,
                            });
                    if duplicate {
                        continue;
                    }
                    *next_request += 1;
                    pick.id = *next_request;
                    self.selected_torrent.get_or_insert(pick.id);
                    self.torrents.push(pick);
                },
                Err(problem) => problems.push(problem),
            }
        }
        self.revalidate();
        problems
    }

    pub(super) fn set_torrent_files(&mut self, torrent: u64, file: Option<usize>, selected: bool) {
        let search = self.torrent_search.trim().to_lowercase();
        let Some(pick) = self.torrents.iter_mut().find(|pick| pick.id == torrent) else {
            return;
        };
        for (index, entry) in pick.files.iter_mut().enumerate() {
            let target = match file {
                Some(file) => index == file,
                None => {
                    search.is_empty()
                        || entry
                            .path
                            .to_string_lossy()
                            .to_lowercase()
                            .contains(&search)
                },
            };
            if target {
                entry.selected = selected;
            }
        }
        self.revalidate();
    }

    pub(super) fn revalidate(&mut self) {
        self.mark_mirrors();
        if !self
            .selected_torrent
            .is_some_and(|id| self.torrents.iter().any(|pick| pick.id == id))
        {
            self.selected_torrent = self.torrents.first().map(|pick| pick.id);
        }
        if !self.subfolder_edited {
            let names: Vec<&str> = self
                .links
                .iter()
                .filter(|link| {
                    !is_magnet(&link.url)
                        && !link.mirror
                        && !matches!(link.state, LinkState::Blocked(_))
                })
                .map(|link| link.name.as_str())
                .chain(self.torrents.iter().map(|pick| pick.name.as_str()))
                .collect();
            let suggestion = shio_core::suggest_folder_name(&names)
                .or_else(|| names.first().map(|name| shio_core::sanitize_filename(name)))
                .unwrap_or_default();
            self.subfolder = truncate_text_bytes(suggestion, MAX_NAME_TEXT_BYTES);
        }
        let plan = if self.links.is_empty() && self.torrents.is_empty() {
            Err(AddValidation::Empty)
        } else if self.too_many {
            Err(AddValidation::Invalid(format!(
                "up to {MAX_LINKS} links at once"
            )))
        } else {
            self.build_plan().map_err(AddValidation::Invalid)
        };
        (self.plan, self.validation) = match plan {
            Ok(plan) if plan.is_empty() => (plan, AddValidation::Empty),
            Ok(plan) => {
                let summary = summary(&plan);
                (plan, AddValidation::Ready(summary))
            },
            Err(validation) => (Vec::new(), validation),
        };
    }

    fn build_plan(&self) -> Result<Vec<Item>, String> {
        let save_path = Path::new(self.save_path.trim());
        if !save_path.is_absolute() {
            return Err("choose a download folder".to_owned());
        }
        let mut items = Vec::new();
        let mut sets: BTreeMap<String, Vec<(&Link, u32)>> = BTreeMap::new();
        let mut mirrors: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for link in &self.links {
            if link.mirror {
                if let Some(part) = shio_core::parse_archive_part(&link.name) {
                    mirrors
                        .entry(part.base_name)
                        .or_default()
                        .push(link.url.clone());
                }
                continue;
            }
            if matches!(link.state, LinkState::Blocked(_)) {
                continue;
            }
            if is_magnet(&link.url) {
                items.push(Item::Magnet(link.url.clone()));
            } else if let Some(part) = shio_core::parse_archive_part(&link.name) {
                sets.entry(part.base_name)
                    .or_default()
                    .push((link, part.part_number));
            } else {
                items.push(Item::Http {
                    url: link.url.clone(),
                    name: link.name.clone(),
                    mirrors: Vec::new(),
                });
            }
        }
        for (base_name, parts) in sets {
            if let [(link, _)] = parts.as_slice() {
                items.push(Item::Http {
                    url: link.url.clone(),
                    name: link.name.clone(),
                    mirrors: mirrors.remove(&base_name).unwrap_or_default(),
                });
                continue;
            }
            let name = shio_core::subfolder_value(self.create_subfolder, &self.subfolder)
                .unwrap_or_else(|| {
                    let names: Vec<&str> =
                        parts.iter().map(|(link, _)| link.name.as_str()).collect();
                    shio_core::suggest_folder_name(&names).unwrap_or_else(|| base_name.clone())
                });
            let request = parts
                .iter()
                .map(|(link, number)| {
                    HttpArchivePartRequest::new(
                        link.url.clone(),
                        link.name.clone(),
                        *number,
                        self.segments,
                    )
                })
                .collect::<shio_core::Result<Vec<_>>>()
                .and_then(|parts| {
                    HttpArchiveSetRequest::new(
                        name.clone(),
                        save_path.to_path_buf(),
                        parts,
                        self.auto_extract,
                    )
                })
                .map_err(|error| format!("{base_name}: {}", error.short_label()))?;
            items.push(Item::ArchiveSet {
                name,
                request,
                urls: parts
                    .iter()
                    .map(|(link, _)| link.url.clone())
                    .chain(mirrors.remove(&base_name).unwrap_or_default())
                    .collect(),
            });
        }
        for pick in &self.torrents {
            if pick.selected_count() == 0 {
                return Err(format!("select at least one file in {}", pick.name));
            }
            items.push(Item::Torrent(pick.id));
        }
        Ok(items)
    }

    pub(super) fn submit(&mut self) -> Vec<AddRequest> {
        let save_path = PathBuf::from(self.save_path.trim());
        let subfolder = shio_core::subfolder_value(self.create_subfolder, &self.subfolder);
        let torrent_path = subfolder
            .as_ref()
            .map_or_else(|| save_path.clone(), |subfolder| save_path.join(subfolder));
        let rename = self.filename.trim();
        let rename = (self.single_http() && !rename.is_empty()).then_some(rename);
        self.submitting = true;
        self.failures.clear();
        self.submitted.clear();
        let mut requests = Vec::with_capacity(self.plan.len());
        for item in &self.plan {
            let (label, source, request) = match item {
                Item::Http { url, name, mirrors } => {
                    let mut request = HttpDownloadRequest::new(url.clone(), save_path.clone())
                        .with_segments(self.segments)
                        .with_auto_extract(self.auto_extract);
                    if let Some(subfolder) = &subfolder {
                        request = request.with_subfolder(subfolder.clone());
                    }
                    if let Some(rename) = rename {
                        request = request.with_filename(rename.to_owned());
                    }
                    (
                        name.clone(),
                        Source::Urls(
                            std::iter::once(url.clone())
                                .chain(mirrors.iter().cloned())
                                .collect(),
                        ),
                        AddRequest::Http(request),
                    )
                },
                Item::ArchiveSet {
                    name,
                    request,
                    urls,
                } => (
                    name.clone(),
                    Source::Urls(urls.clone()),
                    AddRequest::ArchiveSet(request.clone()),
                ),
                Item::Magnet(magnet) => (
                    "magnet link".to_owned(),
                    Source::Urls(vec![magnet.clone()]),
                    AddRequest::Torrent(
                        TorrentDownloadRequest::new(
                            TorrentSource::Magnet(magnet.clone()),
                            torrent_path.clone(),
                        )
                        .with_auto_extract(self.auto_extract),
                    ),
                ),
                Item::Torrent(id) => {
                    let Some(pick) = self.torrents.iter().find(|pick| pick.id == *id) else {
                        continue;
                    };
                    (
                        pick.name.clone(),
                        Source::Torrent(*id),
                        AddRequest::Torrent(pick.request(torrent_path.clone(), self.auto_extract)),
                    )
                },
            };
            self.submitted.push((label, source));
            requests.push(request);
        }
        requests
    }

    pub(super) fn finish_submit(
        &mut self,
        results: Vec<Result<(), String>>,
        next_request: &mut u64,
    ) -> (usize, Previews) {
        self.submitting = false;
        let mut added = 0;
        let mut added_urls = HashSet::new();
        for ((label, source), result) in self.submitted.drain(..).zip(results) {
            if let Err(reason) = result {
                self.failures.push((label, reason));
                continue;
            }
            added += 1;
            match source {
                Source::Urls(urls) => added_urls.extend(urls),
                Source::Torrent(id) => {
                    if let Some(index) = self.torrents.iter().position(|pick| pick.id == id)
                        && let PickSource::Magnet(magnet) = self.torrents.remove(index).source
                    {
                        added_urls.insert(magnet);
                    }
                },
            }
        }
        let text = self.urls.text();
        let remaining: Vec<&str> = text
            .lines()
            .filter(|line| !added_urls.contains(line.trim()))
            .collect();
        self.urls = text_editor::Content::with_text(&remaining.join("\n"));
        (added, self.sync_links(next_request))
    }
}

fn host(url: &str) -> Option<String> {
    url::Url::parse(url).ok()?.host_str().map(str::to_owned)
}

fn summary(plan: &[Item]) -> String {
    let (mut links, mut sets, mut torrents) = (0, 0, 0);
    for item in plan {
        match item {
            Item::Http { .. } | Item::Magnet(_) => links += 1,
            Item::ArchiveSet { .. } => sets += 1,
            Item::Torrent(_) => torrents += 1,
        }
    }
    [
        (links, "link", "links"),
        (sets, "archive set", "archive sets"),
        (torrents, "torrent", "torrents"),
    ]
    .into_iter()
    .filter(|(count, ..)| *count > 0)
    .map(|(count, one, many)| format!("{count} {}", if count == 1 { one } else { many }))
    .collect::<Vec<_>>()
    .join(" · ")
}

pub(super) fn file_label(path: &Path) -> String {
    path.file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAGNET: &str = "magnet:?xt=urn:btih:cab507494d02ebb1178b38f2e9d7be299c86b862";

    fn dialog(text: &str) -> (AddDialog, Previews) {
        let mut dialog = AddDialog::new(&AppConfig::default());
        dialog.save_path = std::env::temp_dir().to_string_lossy().into_owned();
        dialog.urls = text_editor::Content::with_text(text);
        let previews = dialog.sync_links(&mut 0);
        (dialog, previews)
    }

    #[test]
    fn links_group_into_archive_sets_and_validate_before_submit() {
        let many: Vec<String> = (0..=MAX_LINKS)
            .map(|n| format!("https://e.com/{n}.bin"))
            .collect();
        let many = many.join("\n");
        let mixed = format!(
            "https://e.com/a.part01.rar\nhttps://e.com/a.part02.rar\nhttps://e.com/x.iso\n{MAGNET}\nhttps://e.com/x.iso"
        );
        let cases = [
            ("", AddValidation::Empty),
            ("not a link\nftp://e.com/x", AddValidation::Empty),
            (
                mixed.as_str(),
                AddValidation::Ready("2 links · 1 archive set".to_owned()),
            ),
            (
                "https://e.com/b.part01.rar\nhttps://e.com/b.part03.rar",
                AddValidation::Invalid(String::new()),
            ),
            (many.as_str(), AddValidation::Invalid(String::new())),
        ];
        for (text, expected) in cases {
            let (dialog, _) = dialog(text);
            match (&dialog.validation, &expected) {
                (AddValidation::Invalid(_), AddValidation::Invalid(_)) => {},
                (actual, expected) => assert_eq!(actual, expected, "{text}"),
            }
        }

        let (mut relative, _) = dialog("https://e.com/x.iso");
        relative.save_path = "downloads".to_owned();
        relative.revalidate();
        assert!(matches!(relative.validation, AddValidation::Invalid(_)));
    }

    #[test]
    fn blocked_links_are_left_out_and_stale_previews_ignored() {
        let (mut dialog, previews) = dialog("https://e.com/a.bin\nhttps://e.com/b.bin");
        assert_eq!(previews.http.len(), 2);
        let blocked = |request_id| HttpPreviewResult {
            request_id,
            state: HttpPreviewState::Blocked {
                reason: "a web page".to_owned(),
            },
        };

        dialog.apply_http_preview(blocked(previews.http[0].0), &mut 2);
        dialog.apply_http_preview(blocked(99), &mut 2);
        assert_eq!(dialog.validation, AddValidation::Ready("1 link".to_owned()));

        dialog.apply_http_preview(blocked(previews.http[1].0), &mut 2);
        assert_eq!(dialog.validation, AddValidation::Empty);
    }

    #[test]
    fn failed_items_stay_in_the_form_with_their_reason() {
        let (mut dialog, _) = dialog("https://e.com/a.bin\nhttps://e.com/b.bin");
        assert_eq!(dialog.submit().len(), 2);
        assert!(dialog.submitting);

        let (added, _) =
            dialog.finish_submit(vec![Ok(()), Err("not found (404)".to_owned())], &mut 2);

        assert_eq!(added, 1);
        assert!(!dialog.submitting);
        assert_eq!(
            dialog.failures,
            [("b.bin".to_owned(), "not found (404)".to_owned())]
        );
        assert_eq!(dialog.urls.text().trim(), "https://e.com/b.bin");
        assert_eq!(dialog.links.len(), 1);
    }

    #[test]
    fn mirror_parts_are_skipped_and_cleared_on_submit() {
        const PART1: &str =
            "https://open.example/aaaaaaaaaaaa#Some_Game_--_uploader.example_--_.part1.rar";
        const PART2: &str =
            "https://open.example/bbbbbbbbbbbb#Some_Game_--_uploader.example_--_.part2.rar";
        const MIRROR: &str =
            "https://captcha.example/cccccccccccc/Some_Game_--_uploader.example_--_.part1.rar";
        let one_set = AddValidation::Ready("1 archive set".to_owned());

        let (mut dialog, _) = dialog(&format!("{PART1}\n{PART2}\n{MIRROR}"));
        assert_eq!(dialog.validation, one_set);
        assert!(dialog.links[2].mirror);
        assert_eq!(dialog.subfolder, "Some Game");
        assert_eq!(dialog.submit().len(), 1);
        dialog.finish_submit(vec![Ok(())], &mut 3);
        assert_eq!(dialog.urls.text().trim(), "");

        let (mut dialog, previews) = self::dialog(&format!("{MIRROR}\n{PART1}\n{PART2}"));
        let captcha = HttpPreviewResult {
            request_id: previews.http[0].0,
            state: HttpPreviewState::Blocked {
                reason: "needs a captcha".to_owned(),
            },
        };
        dialog.apply_http_preview(captcha, &mut 3);
        assert!(dialog.links[0].mirror);
        assert_eq!(dialog.validation, one_set);
    }

    #[test]
    fn crowded_pastes_preview_one_link_per_host() {
        let blocked_host =
            (1..=5).map(|n| format!("https://captcha.example/aaaa1111/G.part{n}.rar"));
        let open_host = (1..=5).map(|n| format!("https://open.example/bbbbbb{n}#G.part{n}.rar"));
        let text = blocked_host.chain(open_host).collect::<Vec<_>>().join("\n");
        let (mut dialog, previews) = dialog(&text);
        assert_eq!(previews.http.len(), 2);
        assert!(previews.http[0].1.contains("captcha.example"));
        assert!(previews.http[1].1.contains("open.example"));

        dialog.urls = text_editor::Content::with_text(&format!(
            "https://open.example/bbbbbb6#G.part6.rar\n{text}"
        ));
        let again = dialog.sync_links(&mut 100);
        assert_eq!(again.cancel, Vec::<u64>::new());
        assert_eq!(again.http, Vec::<(u64, String)>::new());
        assert_eq!(dialog.resolving().count(), 2);

        let captcha = HttpPreviewResult {
            request_id: previews.http[0].0,
            state: HttpPreviewState::Blocked {
                reason: "needs a captcha".to_owned(),
            },
        };
        dialog.apply_http_preview(captcha, &mut 100);
        for link in &dialog.links {
            assert_eq!(
                link.mirror,
                link.url.contains("captcha.example"),
                "{}",
                link.url
            );
        }
        assert_eq!(
            dialog.validation,
            AddValidation::Ready("1 archive set".to_owned())
        );
    }

    #[test]
    fn folder_previews_expand_into_one_link_per_file() {
        let (mut dialog, previews) = dialog("https://files.example/d/abcd");
        let mut next = previews.http[0].0;
        let folder = HttpPreviewResult {
            request_id: previews.http[0].0,
            state: HttpPreviewState::Folder {
                files: vec!["a b.mkv".to_owned(), "c.zip".to_owned()],
            },
        };
        let expanded = dialog.apply_http_preview(folder, &mut next);

        assert_eq!(
            dialog.urls.text().lines().collect::<Vec<_>>(),
            [
                "https://files.example/d/abcd?file=a+b.mkv#a%20b.mkv",
                "https://files.example/d/abcd?file=c.zip#c.zip",
            ]
        );
        let names: Vec<&str> = dialog.links.iter().map(|link| link.name.as_str()).collect();
        assert_eq!(names, ["a b.mkv", "c.zip"]);
        assert_eq!(
            dialog.validation,
            AddValidation::Ready("2 links".to_owned())
        );
        assert_eq!(expanded.http.len(), 2, "each file is previewed on its own");
    }
}
