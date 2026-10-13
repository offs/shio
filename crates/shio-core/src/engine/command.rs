use std::fmt;

use tokio::sync::{mpsc, oneshot};

use super::{HttpArchiveSetRequest, HttpDownloadRequest, TorrentDownloadRequest};
use crate::config::AppConfig;
use crate::types::{DownloadId, HttpPreviewResult, MagnetPreviewResult};

/// Replies only report errors; state arrives as `ProgressEvent`s once durable.
/// Row commands also accept a package's row id and apply to every part that fits.
pub enum EngineCommand {
    AddHttp {
        request: HttpDownloadRequest,
        reply: oneshot::Sender<crate::Result<()>>,
    },
    AddHttpArchiveSet {
        request: HttpArchiveSetRequest,
        reply: oneshot::Sender<crate::Result<()>>,
    },
    AddTorrentPrepared {
        request: TorrentDownloadRequest,
        reply: oneshot::Sender<crate::Result<()>>,
    },
    Pause {
        id: DownloadId,
        reply: oneshot::Sender<crate::Result<()>>,
    },
    Resume {
        id: DownloadId,
        reply: oneshot::Sender<crate::Result<()>>,
    },
    Cancel {
        id: DownloadId,
        reply: oneshot::Sender<crate::Result<()>>,
    },
    Remove {
        id: DownloadId,
        delete_files: bool,
        reply: oneshot::Sender<crate::Result<()>>,
    },
    Retry {
        id: DownloadId,
        reply: oneshot::Sender<crate::Result<()>>,
    },
    RetryExtract {
        id: DownloadId,
        password: Option<String>,
        reply: oneshot::Sender<crate::Result<()>>,
    },
    ForceRecheck {
        id: DownloadId,
        reply: oneshot::Sender<crate::Result<()>>,
    },
    StopSeeding {
        id: DownloadId,
        reply: oneshot::Sender<crate::Result<()>>,
    },
    SetPin {
        id: DownloadId,
        pinned: bool,
        reply: oneshot::Sender<crate::Result<()>>,
    },
    PauseAll {
        reply: oneshot::Sender<crate::Result<()>>,
    },
    ResumeAll {
        reply: oneshot::Sender<crate::Result<()>>,
    },
    /// Torrent port, DHT and `UPnP` apply when the torrent session next opens; the rest at once.
    SetConfig {
        config: AppConfig,
        reply: oneshot::Sender<crate::Result<()>>,
    },
    ResolveMagnetPreview {
        request_id: u64,
        magnet: String,
        reply: mpsc::Sender<MagnetPreviewResult>,
    },
    ResolveHttpPreview {
        request_id: u64,
        url: String,
        reply: mpsc::Sender<HttpPreviewResult>,
    },
    /// Cancels either kind of preview; it may arrive before the preview it cancels.
    CancelPreview { request_id: u64 },
    /// Replies once every worker's final state is durable.
    Shutdown { reply: oneshot::Sender<()> },
}

impl fmt::Debug for EngineCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AddHttp { request, .. } => f.debug_tuple("AddHttp").field(request).finish(),
            Self::AddHttpArchiveSet { request, .. } => {
                f.debug_tuple("AddHttpArchiveSet").field(request).finish()
            },
            Self::AddTorrentPrepared { request, .. } => {
                f.debug_tuple("AddTorrentPrepared").field(request).finish()
            },
            Self::Pause { id, .. } => f.debug_tuple("Pause").field(id).finish(),
            Self::Resume { id, .. } => f.debug_tuple("Resume").field(id).finish(),
            Self::Cancel { id, .. } => f.debug_tuple("Cancel").field(id).finish(),
            Self::Remove {
                id, delete_files, ..
            } => f
                .debug_tuple("Remove")
                .field(id)
                .field(delete_files)
                .finish(),
            Self::Retry { id, .. } => f.debug_tuple("Retry").field(id).finish(),
            Self::RetryExtract { id, password, .. } => f
                .debug_tuple("RetryExtract")
                .field(id)
                .field(&password.as_ref().map(|_| "<redacted>"))
                .finish(),
            Self::ForceRecheck { id, .. } => f.debug_tuple("ForceRecheck").field(id).finish(),
            Self::StopSeeding { id, .. } => f.debug_tuple("StopSeeding").field(id).finish(),
            Self::SetPin { id, pinned, .. } => {
                f.debug_tuple("SetPin").field(id).field(pinned).finish()
            },
            Self::PauseAll { .. } => f.write_str("PauseAll"),
            Self::ResumeAll { .. } => f.write_str("ResumeAll"),
            Self::SetConfig { config, .. } => f.debug_tuple("SetConfig").field(config).finish(),
            Self::ResolveMagnetPreview { request_id, .. } => f
                .debug_tuple("ResolveMagnetPreview")
                .field(request_id)
                .finish(),
            Self::ResolveHttpPreview { request_id, .. } => f
                .debug_tuple("ResolveHttpPreview")
                .field(request_id)
                .finish(),
            Self::CancelPreview { request_id } => {
                f.debug_tuple("CancelPreview").field(request_id).finish()
            },
            Self::Shutdown { .. } => f.write_str("Shutdown"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{
        Download, DownloadKind, HttpState, ProxyConfig, ProxyProtocol, TorrentSource,
    };

    #[test]
    fn debug_output_never_contains_secrets() {
        const SECRET: &str = "s3cret-token";
        let url = format!("https://example.com/file.bin?token={SECRET}");
        let magnet =
            format!("magnet:?xt=urn:btih:cab507494d02ebb1178b38f2e9d7be299c86b862&tr={SECRET}");
        let part =
            crate::HttpArchivePartRequest::new(url.clone(), "a.part01.rar".into(), 1, 1).unwrap();
        let part2 =
            crate::HttpArchivePartRequest::new(format!("{url}&2"), "a.part02.rar".into(), 2, 1)
                .unwrap();
        let config = AppConfig {
            proxy: Some(ProxyConfig {
                protocol: ProxyProtocol::Http,
                host: "proxy".into(),
                port: 8080,
                username: Some(SECRET.into()),
                password: Some(SECRET.into()),
            }),
            ..AppConfig::default()
        };
        let mut http = HttpState::new(url.clone());
        http.headers.push(("Authorization".into(), SECRET.into()));
        let download =
            Download::with_kind("f".into(), std::env::temp_dir(), DownloadKind::Http(http));
        let commands = [
            EngineCommand::AddHttp {
                request: HttpDownloadRequest::new(url.clone(), std::env::temp_dir()),
                reply: oneshot::channel().0,
            },
            EngineCommand::AddHttpArchiveSet {
                request: HttpArchiveSetRequest::new(
                    "a".into(),
                    std::env::temp_dir(),
                    vec![part, part2],
                    false,
                )
                .unwrap(),
                reply: oneshot::channel().0,
            },
            EngineCommand::AddTorrentPrepared {
                request: TorrentDownloadRequest::new(
                    TorrentSource::Magnet(magnet.clone()),
                    std::env::temp_dir(),
                ),
                reply: oneshot::channel().0,
            },
            EngineCommand::RetryExtract {
                id: DownloadId::new(),
                password: Some(SECRET.into()),
                reply: oneshot::channel().0,
            },
            EngineCommand::SetConfig {
                config,
                reply: oneshot::channel().0,
            },
            EngineCommand::ResolveMagnetPreview {
                request_id: 1,
                magnet,
                reply: mpsc::channel(1).0,
            },
            EngineCommand::ResolveHttpPreview {
                request_id: 2,
                url,
                reply: mpsc::channel(1).0,
            },
        ];

        let debug = commands.iter().map(|command| format!("{command:?}"));
        for debug in debug.chain([format!("{download:?}")]) {
            assert!(!debug.contains(SECRET), "{debug}");
        }
    }
}
