use super::{HttpArchiveSetRequest, HttpDownloadRequest, TorrentDownloadRequest};
use crate::config::TorrentConfig;
use crate::types::{DownloadId, HttpPreviewResult, MagnetPreviewResult, TorrentSource};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{Notify, mpsc, oneshot};

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
    PauseAll {
        reply: oneshot::Sender<crate::Result<()>>,
    },
    ResumeAll {
        reply: oneshot::Sender<crate::Result<()>>,
    },
    SetSpeedLimit(Option<u64>),
    SetMaxConcurrent(u8),
    SetTorrentConfig(TorrentConfig),
    SetPin {
        id: DownloadId,
        pinned: bool,
        reply: oneshot::Sender<crate::Result<()>>,
    },
    UpdateMetadata {
        id: DownloadId,
        filename: String,
        save_path: PathBuf,
        reply: oneshot::Sender<crate::Result<()>>,
    },
    AddTorrent {
        source: TorrentSource,
        save_path: PathBuf,
        start_paused: bool,
        auto_extract: bool,
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
    CancelHttpPreview {
        request_id: u64,
    },
    ForceRecheck {
        id: DownloadId,
        reply: oneshot::Sender<crate::Result<()>>,
    },
    StopSeeding {
        id: DownloadId,
        reply: oneshot::Sender<crate::Result<()>>,
    },
    Shutdown {
        ack: Arc<Notify>,
    },
}

impl std::fmt::Debug for EngineCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AddHttp { request, .. } => f
                .debug_struct("AddHttp")
                .field("id", &request.id)
                .field("filename", &request.filename)
                .finish(),
            Self::AddHttpArchiveSet { request, .. } => f
                .debug_struct("AddHttpArchiveSet")
                .field("id", &request.package.id)
                .field("name", &request.package.name)
                .field("parts", &request.parts.len())
                .finish(),
            Self::AddTorrentPrepared { request, .. } => f
                .debug_struct("AddTorrentPrepared")
                .field("filename", &request.filename)
                .finish(),
            Self::Pause { id, .. } => f.debug_tuple("Pause").field(id).finish(),
            Self::Resume { id, .. } => f.debug_tuple("Resume").field(id).finish(),
            Self::Cancel { id, .. } => f.debug_tuple("Cancel").field(id).finish(),
            Self::Remove {
                id, delete_files, ..
            } => f
                .debug_struct("Remove")
                .field("id", id)
                .field("delete_files", delete_files)
                .finish(),
            Self::Retry { id, .. } => f.debug_tuple("Retry").field(id).finish(),
            Self::RetryExtract { id, password, .. } => f
                .debug_struct("RetryExtract")
                .field("id", id)
                .field("password", &password.as_ref().map(|_| "<redacted>"))
                .finish(),
            Self::PauseAll { .. } => f.write_str("PauseAll"),
            Self::ResumeAll { .. } => f.write_str("ResumeAll"),
            Self::SetSpeedLimit(limit) => f.debug_tuple("SetSpeedLimit").field(limit).finish(),
            Self::SetMaxConcurrent(max) => f.debug_tuple("SetMaxConcurrent").field(max).finish(),
            Self::SetTorrentConfig(config) => {
                f.debug_tuple("SetTorrentConfig").field(config).finish()
            },
            Self::SetPin { id, pinned, .. } => {
                f.debug_tuple("SetPin").field(id).field(pinned).finish()
            },
            Self::UpdateMetadata {
                id,
                filename,
                save_path,
                ..
            } => f
                .debug_struct("UpdateMetadata")
                .field("id", id)
                .field("filename", filename)
                .field("save_path", save_path)
                .finish(),
            Self::AddTorrent {
                source,
                save_path,
                start_paused,
                auto_extract,
                ..
            } => f
                .debug_struct("AddTorrent")
                .field("source", &torrent_source_debug(source))
                .field("save_path", save_path)
                .field("start_paused", start_paused)
                .field("auto_extract", auto_extract)
                .finish(),
            Self::ResolveMagnetPreview { request_id, .. } => f
                .debug_struct("ResolveMagnetPreview")
                .field("request_id", request_id)
                .field("magnet", &"<redacted>")
                .field("reply", &"<sender>")
                .finish(),
            Self::ResolveHttpPreview { request_id, .. } => f
                .debug_struct("ResolveHttpPreview")
                .field("request_id", request_id)
                .field("url", &"<redacted>")
                .field("reply", &"<sender>")
                .finish(),
            Self::CancelHttpPreview { request_id } => f
                .debug_struct("CancelHttpPreview")
                .field("request_id", request_id)
                .finish(),
            Self::ForceRecheck { id, .. } => f.debug_tuple("ForceRecheck").field(id).finish(),
            Self::StopSeeding { id, .. } => f.debug_tuple("StopSeeding").field(id).finish(),
            Self::Shutdown { .. } => f
                .debug_struct("Shutdown")
                .field("ack", &"<notify>")
                .finish(),
        }
    }
}

impl EngineCommand {
    pub fn shutdown() -> (Self, Arc<Notify>) {
        let ack = Arc::new(Notify::new());
        (Self::Shutdown { ack: ack.clone() }, ack)
    }
}

pub(super) fn ack(reply: oneshot::Sender<crate::Result<()>>, result: crate::Result<()>) {
    if reply.send(result).is_err() {
        tracing::debug!("engine command acknowledgement dropped");
    }
}

fn torrent_source_debug(source: &TorrentSource) -> String {
    match source {
        TorrentSource::Magnet(_) => "magnet:<redacted>".to_string(),
        TorrentSource::File(bytes) => format!("torrent-file:{} bytes", bytes.len()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_preview_command_debug_redacts_url() {
        const TOKEN: &str = "secret-command-token";
        let (reply, _rx) = mpsc::channel(1);
        let cmd = EngineCommand::ResolveHttpPreview {
            request_id: 55,
            url: format!("https://example.com/file.bin?token={TOKEN}"),
            reply,
        };

        let debug = format!("{cmd:?}");

        assert!(debug.contains("ResolveHttpPreview"));
        assert!(!debug.contains(TOKEN));
    }

    #[test]
    fn add_command_debug_redacts_http_url() {
        const TOKEN: &str = "secret-add-token";
        let download = HttpDownloadRequest::new(
            format!("https://example.com/file.bin?token={TOKEN}"),
            PathBuf::from("downloads"),
        );
        let (reply, _ack) = oneshot::channel();
        let cmd = EngineCommand::AddHttp {
            request: download,
            reply,
        };

        let debug = format!("{cmd:?}");

        assert!(debug.contains("AddHttp"));
        assert!(!debug.contains(TOKEN));
        assert!(!debug.contains("https://example.com"));
    }

    #[test]
    fn torrent_command_debug_redacts_source_data() {
        const TOKEN: &str = "secret-magnet-token";
        let (reply, _ack) = oneshot::channel();
        let cmd = EngineCommand::AddTorrent {
            source: TorrentSource::Magnet(format!(
                "magnet:?xt=urn:btih:cab507494d02ebb1178b38f2e9d7be299c86b862&tr={TOKEN}"
            )),
            save_path: PathBuf::from("downloads"),
            start_paused: true,
            auto_extract: false,
            reply,
        };

        let debug = format!("{cmd:?}");

        assert!(debug.contains("AddTorrent"));
        assert!(!debug.contains(TOKEN));
        assert!(!debug.contains("magnet:?"));
    }

    #[test]
    fn magnet_preview_command_debug_redacts_magnet() {
        const TOKEN: &str = "secret-preview-token";
        let (reply, _rx) = mpsc::channel(1);
        let cmd = EngineCommand::ResolveMagnetPreview {
            request_id: 7,
            magnet: format!(
                "magnet:?xt=urn:btih:cab507494d02ebb1178b38f2e9d7be299c86b862&tr={TOKEN}"
            ),
            reply,
        };

        let debug = format!("{cmd:?}");

        assert!(debug.contains("ResolveMagnetPreview"));
        assert!(!debug.contains(TOKEN));
        assert!(!debug.contains("magnet:?"));
    }
}
