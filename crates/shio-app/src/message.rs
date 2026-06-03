use crate::theme::ThemeId;
use shio_core::WindowMaterialPreference;
use shio_core::{ArchivePackage, Download, DownloadId, DownloadProgress, DownloadStatus};
use std::path::PathBuf;
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NoticeId {
    ConfigSave,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FirstRunStep {
    Look,
    Folder,
    Behavior,
}

impl FirstRunStep {
    pub(crate) const fn next(self) -> Option<Self> {
        match self {
            Self::Look => Some(Self::Folder),
            Self::Folder => Some(Self::Behavior),
            Self::Behavior => None,
        }
    }

    pub(crate) const fn previous(self) -> Option<Self> {
        match self {
            Self::Look => None,
            Self::Folder => Some(Self::Look),
            Self::Behavior => Some(Self::Folder),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct QueueDownloadsResult {
    pub(crate) downloads: Vec<Download>,
    pub(crate) packages: Vec<ArchivePackage>,
    pub(crate) error: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct EditConflictResult {
    pub(crate) id: DownloadId,
    pub(crate) filename: String,
    pub(crate) save_path: String,
    pub(crate) conflict: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct EditRenameResult {
    pub(crate) id: DownloadId,
    pub(crate) filename: String,
    pub(crate) save_path: PathBuf,
    pub(crate) result: Result<(), String>,
}

#[derive(Debug, Clone)]
pub(crate) enum EngineAction {
    None,
    SetStatus {
        id: DownloadId,
        status: DownloadStatus,
        clear_error: bool,
    },
    Remove {
        id: DownloadId,
    },
    SetPin {
        id: DownloadId,
        pinned: bool,
    },
    PauseAll,
    ResumeAll,
    ApplyMetadata {
        id: DownloadId,
        filename: String,
        save_path: PathBuf,
    },
}

#[derive(Debug, Clone)]
pub(crate) enum Message {
    AddDownloadPressed,
    CancelAddDownload,
    ConfirmAddDownload,
    AddUrlsAction(iced::widget::text_editor::Action),
    AddFilenameChanged(String),
    PickTorrentFiles,
    TorrentFilesPicked(Option<Vec<PathBuf>>),
    TorrentFilesDropped(Vec<PathBuf>),
    AddTorrentSearchChanged(String),
    AddTorrentSearchCleared,
    AddTorrentSearchFocus,
    AddTorrentFileToggled {
        torrent_index: usize,
        file_index: usize,
        selected: bool,
    },
    AddTorrentFilesSelectionChanged {
        torrent_index: usize,
        selected: bool,
    },
    AddTorrentMatchingSelectionChanged {
        torrent_index: usize,
        selected: bool,
    },
    AddSourceSelected(crate::app::AddSourceId),
    OpenAssociatedSources(crate::platform::AssociationLaunch),
    MagnetPreviewResolved(shio_core::MagnetPreviewResult),
    HttpPreviewResolved(shio_core::HttpPreviewResult),
    AddSubfolderNameChanged(String),

    PauseDownload(DownloadId),
    ResumeDownload(DownloadId),
    RequestCancelDownload(DownloadId),
    ConfirmCancelDownload,
    CancelCancelDownload,
    RetryDownload(DownloadId),
    ForceRecheck(DownloadId),
    RetryExtract(DownloadId),
    StopSeeding(DownloadId),
    RequestPassword(DownloadId),
    PasswordChanged(String),
    ConfirmPassword(DownloadId),
    CancelPassword,
    RequestDeleteWithFiles(DownloadId),
    ConfirmDeleteFiles,
    ConfirmRemoveFromList,
    CancelDeleteWithFiles,
    TogglePin(DownloadId),

    RequestEdit(DownloadId),
    EditFilenameChanged(String),
    EditSavePathChanged(String),
    EditPickSavePath,
    EditPickSavePathResult(Option<PathBuf>),
    ConfirmEdit(DownloadId),
    CancelEdit,
    EditConflictChecked(EditConflictResult),
    EditRenameCompleted(EditRenameResult),

    PauseAll,
    ResumeAll,

    SelectClicked(DownloadId),
    SelectAll,
    TabSelected(Tab),
    SearchTextChanged(String),
    SearchFocus,
    SearchApplySuggestion(String),
    SortColumn(SortCol),
    ColumnResizeStart(DownloadColumn),
    ColumnResizeMove(f32),
    ColumnResizeEnd,

    ProgressTick(Vec<DownloadProgress>),
    DownloadsQueued(QueueDownloadsResult),
    EngineCommandDelivered {
        action: EngineAction,
        result: Result<(), String>,
    },
    TorrentFilesIngested(Vec<Result<crate::app::AddTorrentFile, String>>),

    OpenFile(DownloadId),
    OpenFolder(DownloadId),
    OpenFileCompleted(Result<(), String>),
    OpenFolderCompleted(Result<(), String>),
    CopyUrlCompleted(Result<(), String>),

    ToggleClipboard,
    ClipboardUrl(String),

    OpenSettings,
    CloseSettings,
    SettingsCategoryChanged(SettingsCategory),
    SettingsSearchChanged(String),
    SettingsSearchCleared,
    SettingsSearchFocus,
    ToggleNotifications,
    DefaultCreateSubfolderToggled,
    DefaultAutoExtractToggled,
    ExtractToSubfolderToggled,
    DeleteArchiveAfterExtractToggled,
    CloseToTrayToggled,
    ScrollLongNamesToggled,
    ThemeChanged(ThemeId),
    ThemeMaterialChanged(WindowMaterialPreference),
    SpeedLimitChanged(Option<u64>),
    MaxConcurrentChanged(u8),
    DefaultSegmentsChanged(u8),
    TorrentPortInputChanged(String),
    TorrentDhtToggled(bool),
    TorrentUpnpToggled(bool),
    TorrentSeedPolicyChoiceChanged(SeedPolicyChoice),
    TorrentSeedRatioInputChanged(String),
    TorrentSeedDaysInputChanged(String),
    PickSaveFolder,
    SaveFolderPicked(Option<PathBuf>),
    SetUpFileAssociations,
    FileAssociationsRegistered(Result<(), String>),
    OpenLogsFolder,
    OpenLogsFolderCompleted(Result<(), String>),
    DismissNotice(NoticeId),
    ConfigPersisted {
        id: u64,
        result: Result<(), String>,
    },

    FirstRunBack,
    FirstRunNext,
    FirstRunSkip,

    KeyPressed(iced::keyboard::Key, iced::keyboard::Modifiers),
    ModifiersChanged(iced::keyboard::Modifiers),

    CopyUrl(DownloadId),

    FileDropped(PathBuf),
    DroppedShortcutRead(Option<String>),

    WindowOpened {
        id: iced::window::Id,
        size: iced::Size,
    },
    WindowFocused(bool),
    WindowResized(iced::Size),
    WindowMinimize,
    WindowMaximizeToggle,
    WindowClose,
    WindowDragStart,
    AppExit,

    TrayShow,
    TrayQuit,

    DragDrop(DownloadId),
    DragZonesFound(DownloadId, Vec<(iced::widget::Id, iced::Rectangle)>),
    DragUpdate(DownloadId, iced::Point),

    Frame(Instant),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SeedPolicyChoice {
    StopAutomatically,
    SeedForever,
    NeverSeed,
}

impl SeedPolicyChoice {
    pub(crate) const ALL: [Self; 3] = [Self::StopAutomatically, Self::SeedForever, Self::NeverSeed];

    pub(crate) const fn from_policy(policy: shio_core::SeedPolicy) -> Self {
        match policy {
            shio_core::SeedPolicy::SeedForever => Self::SeedForever,
            shio_core::SeedPolicy::NeverSeed => Self::NeverSeed,
            shio_core::SeedPolicy::StopAtRatio { .. }
            | shio_core::SeedPolicy::StopAtTime { .. }
            | shio_core::SeedPolicy::RatioOrTime { .. } => Self::StopAutomatically,
        }
    }
}

impl std::fmt::Display for SeedPolicyChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::StopAutomatically => "Stop automatically",
            Self::SeedForever => "Seed forever",
            Self::NeverSeed => "Never seed",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tab {
    All,
    Active,
    Completed,
    Queued,
    Errors,
}

impl Tab {
    pub(crate) const ALL: &[Self] = &[
        Self::All,
        Self::Active,
        Self::Completed,
        Self::Queued,
        Self::Errors,
    ];

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Active => "active",
            Self::Completed => "completed",
            Self::Queued => "queued",
            Self::Errors => "errors",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SortCol {
    Name,
    Size,
    Progress,
    Speed,
    Eta,
    DateAdded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SortDirection {
    Ascending,
    Descending,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DownloadColumn {
    Name,
    Size,
    Progress,
    Speed,
    Eta,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct DownloadColumnWidths {
    pub(crate) name: f32,
    pub(crate) size: f32,
    pub(crate) progress: f32,
    pub(crate) speed: f32,
    pub(crate) eta: f32,
}

impl Default for DownloadColumnWidths {
    fn default() -> Self {
        Self {
            name: 280.0,
            size: 132.0,
            progress: 150.0,
            speed: 128.0,
            eta: 128.0,
        }
    }
}

impl DownloadColumnWidths {
    pub(crate) const fn get(self, column: DownloadColumn) -> f32 {
        match column {
            DownloadColumn::Name => self.name,
            DownloadColumn::Size => self.size,
            DownloadColumn::Progress => self.progress,
            DownloadColumn::Speed => self.speed,
            DownloadColumn::Eta => self.eta,
        }
    }

    pub(crate) const fn set(&mut self, column: DownloadColumn, width: f32) {
        match column {
            DownloadColumn::Name => self.name = width,
            DownloadColumn::Size => self.size = width,
            DownloadColumn::Progress => self.progress = width,
            DownloadColumn::Speed => self.speed = width,
            DownloadColumn::Eta => self.eta = width,
        }
    }
}

impl SortDirection {
    pub(crate) const fn toggle(self) -> Self {
        match self {
            Self::Ascending => Self::Descending,
            Self::Descending => Self::Ascending,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingsCategory {
    General,
    Network,
    Notifications,
    Appearance,
    About,
}

impl SettingsCategory {
    pub(crate) const ALL: &[Self] = &[
        Self::General,
        Self::Network,
        Self::Notifications,
        Self::Appearance,
        Self::About,
    ];

    pub(crate) const fn display_label(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Network => "Network",
            Self::Notifications => "Notifications",
            Self::Appearance => "Appearance",
            Self::About => "About",
        }
    }

    pub(crate) const fn description(self) -> &'static str {
        match self {
            Self::General => {
                "Download defaults, file routing, archive handling, and tray behavior."
            },
            Self::Network => "Bandwidth and torrents.",
            Self::Notifications => "System notification behavior.",
            Self::Appearance => "Theme and visual preferences.",
            Self::About => "Version, source, and diagnostics.",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::FirstRunStep;

    #[test]
    fn first_run_step_moves_through_setup_sequence() {
        assert_eq!(
            (
                [
                    FirstRunStep::Look.next(),
                    FirstRunStep::Folder.next(),
                    FirstRunStep::Behavior.next(),
                ],
                [
                    FirstRunStep::Look.previous(),
                    FirstRunStep::Folder.previous(),
                    FirstRunStep::Behavior.previous(),
                ],
            ),
            (
                [
                    Some(FirstRunStep::Folder),
                    Some(FirstRunStep::Behavior),
                    None,
                ],
                [None, Some(FirstRunStep::Look), Some(FirstRunStep::Folder),],
            )
        );
    }
}
