use super::add_model::AddDialog;
use crate::message::{
    FirstRunStep, NoticeId, SeedPolicyChoice, SettingsSection, SortKey, SpeedUnit, Tab,
};
use crate::search::{Query, Suggestion};
use crate::theme::{ResolvedTheme, ThemeCatalog, ThemeId};
use iced::keyboard::Modifiers;
use iced::{Point, Size, window};
use nucleo_matcher::Matcher;
use shio_core::{
    AppConfig, ArchivePackage, Download, DownloadId, DownloadKind, DownloadStatus, EngineCommand,
    PackageExtractState, SeedPolicy, TorrentStats,
};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use tokio::sync::mpsc;

/// Seconds a toast stays up while the window is focused.
const TOAST_SECONDS: u8 = 5;

pub(crate) struct Shio {
    pub(super) engine: mpsc::Sender<EngineCommand>,
    pub(super) shutting_down: bool,
    pub(super) startup_problem: Option<StartupProblem>,
    pub(super) window: Window,
    pub(super) blur_active: bool,
    pub(super) software_rendering: bool,
    pub(super) noctalia: Option<PathBuf>,
    pub(super) config: AppConfig,
    pub(super) theme_catalog: ThemeCatalog,
    pub(super) theme: ResolvedTheme,
    pub(super) downloads: Vec<Download>,
    pub(super) packages: Vec<ArchivePackage>,
    pub(super) rows: Vec<Row>,
    pub(super) counts: TabCounts,
    pub(super) totals: Totals,
    pub(super) selection: HashSet<DownloadId>,
    pub(super) anchor: Option<DownloadId>,
    pub(super) modifiers: Modifiers,
    pub(super) search: String,
    pub(super) query: Query,
    /// `None` while the search field is unfocused.
    pub(super) search_focus: Option<SearchFocus>,
    pub(super) matcher: Matcher,
    pub(super) tab: Tab,
    pub(super) sort: SortKey,
    pub(super) sort_descending: bool,
    pub(super) menu: Option<Menu>,
    pub(super) overlay: Overlay,
    /// Sources waiting for the add dialog while another dialog is open.
    pub(super) pending: Pending,
    /// `None` until the first clipboard read, which only records a baseline.
    pub(super) clipboard_seen: Option<u64>,
    pub(super) clipboard_offer: Option<ClipboardOffer>,
    pub(super) notices: Vec<Notice>,
    pub(super) toast: Option<Toast>,
    pub(super) next_request: u64,
    pub(super) config_save: u64,
}

#[derive(Debug, Default, Clone, Copy)]
pub(super) struct SearchFocus {
    pub(super) highlighted: Option<usize>,
    pub(super) closed: bool,
}

#[derive(Default)]
pub(super) struct Pending {
    pub(super) links: String,
    pub(super) torrents: Vec<PathBuf>,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct Window {
    pub(super) id: Option<window::Id>,
    pub(super) focused: bool,
    pub(super) maximized: bool,
    pub(super) size: Size,
    /// Whether the window was opened with blur; see `blur::apply`.
    pub(super) blur: bool,
}

pub(super) struct StartupProblem {
    pub(super) title: &'static str,
    pub(super) message: String,
    pub(super) location: PathBuf,
}

pub(super) struct Notice {
    pub(super) id: NoticeId,
    pub(super) title: &'static str,
    pub(super) message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToastKind {
    Info,
    Success,
    Error,
}

pub(super) struct Toast {
    pub(super) message: String,
    pub(super) kind: ToastKind,
    pub(super) seconds_left: u8,
}

pub(super) struct ClipboardOffer {
    pub(super) links: String,
    pub(super) name: String,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct Row {
    pub(super) id: DownloadId,
    pub(super) source: RowSource,
    pub(super) status: DownloadStatus,
    pub(super) downloaded: u64,
    pub(super) total: Option<u64>,
    pub(super) speed: u64,
    pub(super) pinned: bool,
    pub(super) torrent: Option<TorrentStats>,
}

/// Index into `Shio::downloads` or `Shio::packages`, valid until the next update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RowSource {
    Download(usize),
    Package(usize),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct TabCounts {
    pub(super) all: usize,
    pub(super) active: usize,
    pub(super) seeding: usize,
    pub(super) done: usize,
    pub(super) failed: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Totals {
    pub(crate) downloads: usize,
    pub(crate) active: usize,
    pub(crate) down: u64,
    pub(crate) up: u64,
    pub(crate) can_pause_all: bool,
    pub(crate) can_resume_all: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Menu {
    Sort,
    Row { id: DownloadId, at: Point },
}

pub(super) enum Overlay {
    None,
    Welcome(FirstRunStep),
    Add(Box<AddDialog>),
    Settings(Box<SettingsDraft>),
    ConfirmRemove(Vec<DownloadId>),
    ConfirmCancel(Vec<DownloadId>),
    Password { id: DownloadId, input: String },
}

/// Text and slider edits; committed on release, submit or close.
pub(crate) struct SettingsDraft {
    pub(crate) section: SettingsSection,
    pub(crate) max_concurrent: u8,
    pub(crate) segments: u8,
    pub(crate) speed_limit_on: bool,
    pub(crate) speed_limit: String,
    pub(crate) speed_unit: SpeedUnit,
    pub(crate) port: String,
    pub(crate) seed: SeedPolicyChoice,
    pub(crate) ratio: String,
    pub(crate) days: String,
    pub(crate) speed_limit_error: Option<&'static str>,
    pub(crate) port_error: Option<&'static str>,
    pub(crate) ratio_error: Option<&'static str>,
    pub(crate) days_error: Option<&'static str>,
}

const SECONDS_PER_DAY: u64 = 86_400;
const MAX_SEED_DAYS: u64 = 3650;

impl SettingsDraft {
    pub(super) fn new(config: &AppConfig) -> Self {
        let limit = config.speed_limit.unwrap_or(SpeedUnit::MiB.bytes());
        let speed_unit = if limit.is_multiple_of(SpeedUnit::MiB.bytes()) {
            SpeedUnit::MiB
        } else {
            SpeedUnit::KiB
        };
        let (ratio, seconds) = match config.torrent.seed_policy {
            SeedPolicy::RatioOrTime { ratio, seconds } => (ratio, seconds),
            SeedPolicy::StopAtRatio { ratio } => (ratio, 7 * SECONDS_PER_DAY),
            SeedPolicy::StopAtTime { seconds } => (1.0, seconds),
            SeedPolicy::SeedForever | SeedPolicy::NeverSeed => (1.0, 7 * SECONDS_PER_DAY),
        };
        Self {
            section: SettingsSection::General,
            max_concurrent: config.max_concurrent,
            segments: config.default_segments,
            speed_limit_on: config.speed_limit.is_some(),
            speed_limit: (limit / speed_unit.bytes()).to_string(),
            speed_unit,
            port: config.torrent.listen_port.to_string(),
            seed: SeedPolicyChoice::of(config.torrent.seed_policy),
            ratio: format!("{ratio:.2}"),
            days: (seconds / SECONDS_PER_DAY).to_string(),
            speed_limit_error: None,
            port_error: None,
            ratio_error: None,
            days_error: None,
        }
    }

    pub(super) fn apply(&mut self, config: &AppConfig) -> AppConfig {
        let mut next = config.clone();
        next.max_concurrent = self.max_concurrent;
        next.default_segments = self.segments;

        self.speed_limit_error = None;
        if self.speed_limit_on {
            match self
                .speed_limit
                .trim()
                .parse::<u64>()
                .ok()
                .filter(|&amount| amount > 0)
                .and_then(|amount| amount.checked_mul(self.speed_unit.bytes()))
            {
                Some(limit) => next.speed_limit = Some(limit),
                None => self.speed_limit_error = Some("enter a whole number above 0"),
            }
        } else {
            next.speed_limit = None;
        }

        self.port_error = None;
        match self.port.trim().parse::<u16>() {
            Ok(port) if port > 0 => next.torrent.listen_port = port,
            _ => self.port_error = Some("enter a port from 1 to 65535"),
        }

        self.ratio_error = None;
        self.days_error = None;
        next.torrent.seed_policy = match self.seed {
            SeedPolicyChoice::SeedForever => SeedPolicy::SeedForever,
            SeedPolicyChoice::NeverSeed => SeedPolicy::NeverSeed,
            SeedPolicyChoice::StopAutomatically => {
                let ratio = self
                    .ratio
                    .trim()
                    .parse::<f32>()
                    .ok()
                    .filter(|ratio| ratio.is_finite() && *ratio >= 0.0);
                let days = self
                    .days
                    .trim()
                    .parse::<u64>()
                    .ok()
                    .filter(|days| *days <= MAX_SEED_DAYS);
                if ratio.is_none() {
                    self.ratio_error = Some("enter a ratio of 0 or more");
                }
                if days.is_none() {
                    self.days_error = Some("enter whole days, up to 3650");
                }
                match (ratio, days) {
                    (Some(ratio), Some(days)) => SeedPolicy::RatioOrTime {
                        ratio,
                        seconds: days * SECONDS_PER_DAY,
                    },
                    _ => config.torrent.seed_policy,
                }
            },
        };
        next
    }
}

impl Shio {
    pub(super) fn build(config: AppConfig, engine: mpsc::Sender<EngineCommand>) -> Self {
        let theme_catalog = ThemeCatalog::load(&AppConfig::theme_dir());
        let theme =
            theme_catalog.resolve(&ThemeId::parse(&config.theme.id).unwrap_or_default(), false);
        Self {
            engine,
            shutting_down: false,
            startup_problem: None,
            window: Window {
                id: None,
                focused: false,
                maximized: false,
                size: super::update::WINDOW_SIZE,
                blur: false,
            },
            blur_active: false,
            software_rendering: false,
            #[cfg(target_os = "linux")]
            noctalia: crate::platform::noctalia::dir(),
            #[cfg(not(target_os = "linux"))]
            noctalia: None,
            config,
            theme_catalog,
            theme,
            downloads: Vec::new(),
            packages: Vec::new(),
            rows: Vec::new(),
            counts: TabCounts::default(),
            totals: Totals::default(),
            selection: HashSet::new(),
            anchor: None,
            modifiers: Modifiers::default(),
            search: String::new(),
            query: Query::default(),
            search_focus: None,
            matcher: Matcher::default(),
            tab: Tab::All,
            sort: SortKey::Added,
            sort_descending: true,
            menu: None,
            overlay: Overlay::None,
            pending: Pending::default(),
            clipboard_seen: None,
            clipboard_offer: None,
            notices: Vec::new(),
            toast: None,
            next_request: 0,
            config_save: 0,
        }
    }

    pub(crate) fn theme(&self, _window: window::Id) -> iced::Theme {
        self.theme.iced_theme.clone()
    }

    pub(super) const fn palette(&self) -> &crate::style::Palette {
        &self.theme.palette
    }

    pub(super) fn row_name(&self, row: &Row) -> &str {
        match row.source {
            RowSource::Download(index) => &self.downloads[index].filename,
            RowSource::Package(index) => &self.packages[index].name,
        }
    }

    pub(super) fn row_error(&self, row: &Row) -> Option<&str> {
        match row.source {
            RowSource::Download(index) => self.downloads[index].error_message.as_deref(),
            RowSource::Package(index) => {
                let package = &self.packages[index];
                package.error_message.as_deref().or_else(|| {
                    package
                        .child_ids()
                        .filter_map(|id| self.download(id))
                        .find_map(|part| part.error_message.as_deref())
                })
            },
        }
    }

    pub(super) fn download(&self, id: DownloadId) -> Option<&Download> {
        self.downloads.iter().find(|download| download.id == id)
    }

    pub(super) fn row(&self, id: DownloadId) -> Option<&Row> {
        self.rows.iter().find(|row| row.id == id)
    }

    pub(super) fn suggestions(&self) -> Vec<Suggestion> {
        let open = self.search_focus.is_some_and(|focus| !focus.closed)
            && matches!(self.overlay, Overlay::None);
        if open {
            crate::search::completions(&self.search)
        } else {
            Vec::new()
        }
    }

    /// tiny-skia drops alpha, so a translucent palette would render dark and opaque.
    pub(super) const fn wants_blur(&self) -> bool {
        self.config.window.blur && !self.software_rendering
    }

    pub(super) fn resolve_theme(&mut self) {
        let id = ThemeId::parse(&self.config.theme.id).unwrap_or_default();
        self.theme = self.theme_catalog.resolve(&id, self.blur_active);
        if self.theme.id.as_str() != self.config.theme.id {
            self.config.theme.id = self.theme.id.as_str().to_owned();
        }
    }

    pub(super) fn push_toast(&mut self, kind: ToastKind, message: impl Into<String>) {
        self.toast = Some(Toast {
            message: message.into(),
            kind,
            seconds_left: TOAST_SECONDS,
        });
    }

    pub(super) fn set_notice(&mut self, id: NoticeId, title: &'static str, message: String) {
        self.notices.retain(|notice| notice.id != id);
        self.notices.push(Notice { id, title, message });
    }

    pub(super) fn refresh(&mut self) {
        let (mut rows, counts, totals) = derive(
            &self.downloads,
            &self.packages,
            &self.query,
            &mut self.matcher,
            self.tab,
        );
        super::sort::sort_rows(
            &mut rows,
            self.sort,
            self.sort_descending,
            |row| match row.source {
                RowSource::Download(index) => {
                    let download = &self.downloads[index];
                    (download.filename.as_str(), download.created_at)
                },
                RowSource::Package(index) => {
                    let package = &self.packages[index];
                    (package.name.as_str(), package.created_at)
                },
            },
        );
        if !self.selection.is_empty() || self.menu.is_some() {
            let visible: HashSet<DownloadId> = rows.iter().map(|row| row.id).collect();
            self.selection.retain(|id| visible.contains(id));
            if self.anchor.is_some_and(|id| !visible.contains(&id)) {
                self.anchor = None;
            }
            if let Some(Menu::Row { id, .. }) = self.menu
                && !visible.contains(&id)
            {
                self.menu = None;
            }
        }
        crate::tray::set_actions(totals.can_pause_all, totals.can_resume_all);
        self.rows = rows;
        self.counts = counts;
        self.totals = totals;
    }
}

pub(super) const fn is_pausable(status: DownloadStatus) -> bool {
    matches!(
        status,
        DownloadStatus::Queued
            | DownloadStatus::Starting
            | DownloadStatus::Downloading
            | DownloadStatus::FetchingMetadata
            | DownloadStatus::Seeding
    )
}

pub(super) fn is_single_file(kind: &DownloadKind) -> bool {
    match kind {
        DownloadKind::Http(_) => true,
        DownloadKind::Torrent(torrent) => {
            let selected = torrent.files.iter().filter(|file| file.selected).count();
            selected == 1 || (selected == 0 && torrent.files.len() == 1)
        },
    }
}

const fn tab_of(status: DownloadStatus) -> Option<Tab> {
    match status {
        DownloadStatus::Seeding => Some(Tab::Seeding),
        DownloadStatus::Completed => Some(Tab::Done),
        DownloadStatus::Error | DownloadStatus::ExtractError | DownloadStatus::PasswordRequired => {
            Some(Tab::Failed)
        },
        DownloadStatus::Cancelled => None,
        DownloadStatus::Pending
        | DownloadStatus::Queued
        | DownloadStatus::Starting
        | DownloadStatus::Downloading
        | DownloadStatus::FetchingMetadata
        | DownloadStatus::Extracting
        | DownloadStatus::Paused => Some(Tab::Active),
    }
}

fn package_status(package: &ArchivePackage, parts: &[&Download]) -> DownloadStatus {
    match package.extract_state {
        PackageExtractState::Extracting => return DownloadStatus::Extracting,
        PackageExtractState::PasswordRequired => return DownloadStatus::PasswordRequired,
        PackageExtractState::Error => return DownloadStatus::ExtractError,
        PackageExtractState::Completed => return DownloadStatus::Completed,
        PackageExtractState::NotStarted => {},
    }
    let any =
        |predicate: fn(DownloadStatus) -> bool| parts.iter().any(|part| predicate(part.status));
    if any(|status| {
        matches!(
            status,
            DownloadStatus::Starting
                | DownloadStatus::Downloading
                | DownloadStatus::FetchingMetadata
        )
    }) {
        DownloadStatus::Downloading
    } else if any(|status| matches!(status, DownloadStatus::Pending | DownloadStatus::Queued)) {
        DownloadStatus::Queued
    } else if any(DownloadStatus::is_failed) {
        DownloadStatus::Error
    } else if any(|status| status == DownloadStatus::Paused) {
        DownloadStatus::Paused
    } else if !parts.is_empty() && !any(|status| status != DownloadStatus::Completed) {
        DownloadStatus::Completed
    } else if any(|status| status == DownloadStatus::Cancelled) {
        DownloadStatus::Cancelled
    } else {
        DownloadStatus::Queued
    }
}

fn derive(
    downloads: &[Download],
    packages: &[ArchivePackage],
    query: &Query,
    matcher: &mut Matcher,
    tab: Tab,
) -> (Vec<Row>, TabCounts, Totals) {
    let by_id: HashMap<DownloadId, &Download> = if packages.is_empty() {
        HashMap::new()
    } else {
        downloads
            .iter()
            .map(|download| (download.id, download))
            .collect()
    };
    let package_parts: HashSet<DownloadId> = packages
        .iter()
        .flat_map(ArchivePackage::child_ids)
        .collect();
    let mut counts = TabCounts::default();
    let mut totals = Totals::default();
    let mut rows = Vec::with_capacity(downloads.len());
    for (index, download) in downloads.iter().enumerate() {
        let torrent = download.torrent().map(|torrent| torrent.stats);
        totals.down = totals.down.saturating_add(download.speed);
        totals.up = totals
            .up
            .saturating_add(torrent.map_or(0, |stats| stats.upload_speed));
        totals.can_pause_all |= is_pausable(download.status);
        totals.can_resume_all |= download.status == DownloadStatus::Paused;
        if package_parts.contains(&download.id) {
            continue;
        }
        rows.push(Row {
            id: download.id,
            source: RowSource::Download(index),
            status: download.status,
            downloaded: download.downloaded,
            total: download.total_size,
            speed: download.speed,
            pinned: download.pinned,
            torrent,
        });
    }
    for (index, package) in packages.iter().enumerate() {
        let parts: Vec<&Download> = package
            .child_ids()
            .filter_map(|id| by_id.get(&id).copied())
            .collect();
        rows.push(Row {
            id: package.id.row_id(),
            source: RowSource::Package(index),
            status: package_status(package, &parts),
            downloaded: parts
                .iter()
                .fold(0, |sum, part| sum.saturating_add(part.downloaded)),
            total: parts.iter().try_fold(0_u64, |sum, part| {
                part.total_size.map(|size| sum.saturating_add(size))
            }),
            speed: parts
                .iter()
                .fold(0, |sum, part| sum.saturating_add(part.speed)),
            pinned: package.pinned,
            torrent: None,
        });
    }

    rows.retain(|row| {
        counts.all += 1;
        totals.active += usize::from(row.status.is_active());
        let row_tab = tab_of(row.status);
        match row_tab {
            Some(Tab::Active) => counts.active += 1,
            Some(Tab::Seeding) => counts.seeding += 1,
            Some(Tab::Done) => counts.done += 1,
            Some(Tab::Failed) => counts.failed += 1,
            Some(Tab::All) | None => {},
        }
        if tab != Tab::All && row_tab != Some(tab) {
            return false;
        }
        match row.source {
            RowSource::Download(index) => {
                let download = &downloads[index];
                query.matches(
                    matcher,
                    &download.filename,
                    download.http().map(|http| http.url.as_str()),
                    download.total_size,
                    download.torrent().map_or(&[], |torrent| &torrent.files),
                )
            },
            RowSource::Package(index) => {
                query.matches(matcher, &packages[index].name, None, row.total, &[])
            },
        }
    });
    totals.downloads = counts.all;
    (rows, counts, totals)
}
