use crate::app::TorrentPick;
use crate::instance::Activation;
use crate::theme::{ThemeCatalog, ThemeId};
use iced::keyboard::{Key, Modifiers};
use iced::widget::text_editor;
use iced::{Point, Size, window};
use shio_core::{DownloadId, HttpPreviewResult, MagnetPreviewResult, ProgressEvent, SeedPolicy};
use std::path::PathBuf;

#[derive(Clone)]
pub(crate) enum Message {
    Engine(Vec<ProgressEvent>),
    EngineReply(Result<(), String>),
    EngineStopped(Result<(), String>),

    SearchChanged(String),
    SearchFocused(bool),
    HighlightSuggestion {
        down: bool,
    },
    AcceptSuggestion,
    ApplySuggestion(&'static str),
    TabSelected(Tab),
    ToggleSortMenu,
    SortBy(SortKey),
    RowPressed(DownloadId),
    RowDoubleClicked(DownloadId),
    OpenRowMenu(DownloadId, Point),
    CloseMenu,

    Pause(DownloadId),
    Resume(DownloadId),
    Retry(DownloadId),
    RetryExtract(DownloadId),
    RequestPassword(DownloadId),
    ForceRecheck(DownloadId),
    StopSeeding(DownloadId),
    TogglePin(DownloadId),
    OpenFile(DownloadId),
    OpenFolder(DownloadId),
    CopyUrl(DownloadId),
    RequestRemove(DownloadId),
    RequestCancel(DownloadId),
    PauseAll,
    ResumeAll,

    CloseOverlay,
    ConfirmRemove {
        delete_files: bool,
    },
    ConfirmCancel,
    PasswordChanged(String),
    SubmitPassword,

    OpenAdd,
    AddUrlsAction(text_editor::Action),
    AddSubmit,
    AddSubmitted(Vec<Result<(), String>>),
    PickTorrentFiles,
    TorrentFilesPicked(Vec<PathBuf>),
    TorrentFilesRead(Vec<Result<TorrentPick, String>>),
    AddPickFolder,
    AddFolderPicked(PathBuf),
    AddSavePathChanged(String),
    AddFilenameChanged(String),
    AddSubfolderToggled(bool),
    AddSubfolderNameChanged(String),
    AddAutoExtractToggled(bool),
    AddTorrentSelected(u64),
    AddTorrentFileToggled {
        torrent: u64,
        file: usize,
        selected: bool,
    },
    AddTorrentFilesToggled {
        torrent: u64,
        selected: bool,
    },
    AddTorrentSearchChanged(String),
    MagnetPreviewed(MagnetPreviewResult),
    HttpPreviewed(HttpPreviewResult),
    ClipboardBaseline(Option<String>),
    ClipboardRead(Option<String>),
    AddFromClipboard,
    DismissClipboardOffer,
    Pasted(Option<String>),
    FileDropped(PathBuf),
    ShortcutRead(Option<String>),

    OpenSettings,
    SettingsSection(SettingsSection),
    ToggleNotifications,
    ToggleCloseToTray,
    ToggleClipboard,
    DefaultCreateSubfolderToggled,
    DefaultAutoExtractToggled,
    DeleteArchiveAfterExtractToggled,
    PickDownloadFolder,
    DownloadFolderPicked(PathBuf),
    MaxConcurrentChanged(u8),
    SegmentsChanged(u8),
    SpeedLimitToggled(bool),
    SpeedLimitInput(String),
    SpeedLimitUnit(SpeedUnit),
    TorrentPortInput(String),
    TorrentDhtToggled(bool),
    TorrentUpnpToggled(bool),
    SeedPolicyChanged(SeedPolicyChoice),
    SeedRatioInput(String),
    SeedDaysInput(String),
    SettingsCommit,
    ThemeChanged(ThemeId),
    BlurToggled(bool),
    BlurActive(bool),
    SoftwareRendering,
    FollowNoctalia,
    #[cfg(target_os = "linux")]
    NoctaliaFollowed(Result<(ThemeCatalog, ThemeId), String>),
    RegisterAssociations,
    UnregisterAssociations,
    AssociationsChanged(Result<&'static str, String>),
    OpenLogsFolder,
    OpenUrl(&'static str),
    OpenStartupLocation,
    Opened(Result<(), String>),
    ConfigSaved {
        id: u64,
        result: Result<(), String>,
    },
    DismissNotice(NoticeId),
    ToastTick,
    ThemesChanged(ThemeCatalog),
    FirstRunBack,
    FirstRunNext,

    KeyPressed {
        key: Key,
        modifiers: Modifiers,
        captured: bool,
    },
    ModifiersChanged(Modifiers),

    WindowOpened,
    WindowFocused(bool),
    WindowResized(Size),
    Maximized(bool),
    WindowDrag,
    WindowResize(window::Direction),
    WindowMinimize,
    WindowMaximizeToggle,
    WindowClose,
    WindowClosed(window::Id),
    TrayShow,
    InstanceActivated(Activation),
    Quit,
    ShutdownComplete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NoticeId {
    ConfigSave,
    EngineStopped,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tab {
    All,
    Active,
    Seeding,
    Done,
    Failed,
}

impl Tab {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Active => "active",
            Self::Seeding => "seeding",
            Self::Done => "done",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SortKey {
    Name,
    Size,
    Progress,
    Added,
}

impl SortKey {
    pub(crate) const ALL: [Self; 4] = [Self::Name, Self::Size, Self::Progress, Self::Added];

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Size => "size",
            Self::Progress => "progress",
            Self::Added => "added",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingsSection {
    General,
    Downloads,
    Torrents,
    Appearance,
    About,
}

impl SettingsSection {
    pub(crate) const ALL: [Self; 5] = [
        Self::General,
        Self::Downloads,
        Self::Torrents,
        Self::Appearance,
        Self::About,
    ];

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::General => "general",
            Self::Downloads => "downloads",
            Self::Torrents => "torrents",
            Self::Appearance => "appearance",
            Self::About => "about",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SpeedUnit {
    KiB,
    MiB,
}

impl SpeedUnit {
    pub(crate) const ALL: [Self; 2] = [Self::KiB, Self::MiB];

    pub(crate) const fn bytes(self) -> u64 {
        match self {
            Self::KiB => 1024,
            Self::MiB => 1024 * 1024,
        }
    }
}

impl std::fmt::Display for SpeedUnit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::KiB => "KiB/s",
            Self::MiB => "MiB/s",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SeedPolicyChoice {
    StopAutomatically,
    SeedForever,
    NeverSeed,
}

impl SeedPolicyChoice {
    pub(crate) const ALL: [Self; 3] = [Self::StopAutomatically, Self::SeedForever, Self::NeverSeed];

    pub(crate) const fn of(policy: SeedPolicy) -> Self {
        match policy {
            SeedPolicy::SeedForever => Self::SeedForever,
            SeedPolicy::NeverSeed => Self::NeverSeed,
            SeedPolicy::StopAtRatio { .. }
            | SeedPolicy::StopAtTime { .. }
            | SeedPolicy::RatioOrTime { .. } => Self::StopAutomatically,
        }
    }
}

impl std::fmt::Display for SeedPolicyChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::StopAutomatically => "stop automatically",
            Self::SeedForever => "seed forever",
            Self::NeverSeed => "never seed",
        })
    }
}
