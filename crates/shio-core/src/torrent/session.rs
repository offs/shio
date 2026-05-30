use std::path::Path;
use std::sync::Arc;

use librqbit::{Session, SessionOptions, SessionPersistenceConfig};

use crate::error::{Result, ShioError};

const TORRENT_LISTEN_PORT_SPAN: u16 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TorrentSessionStop {
    KeepFiles,
    DeleteFiles,
}

impl TorrentSessionStop {
    const fn delete_files(self) -> bool {
        matches!(self, Self::DeleteFiles)
    }
}

pub(crate) async fn open_session(
    data_dir: &Path,
    listen_port: u16,
    dht_enabled: bool,
    upnp_enabled: bool,
) -> Result<Arc<Session>> {
    let output_folder = data_dir.join("torrents");
    tokio::fs::create_dir_all(&output_folder)
        .await
        .map_err(ShioError::Io)?;

    let persistence_folder = data_dir.join("rqbit-session");
    tokio::fs::create_dir_all(&persistence_folder)
        .await
        .map_err(ShioError::Io)?;

    let opts = SessionOptions {
        disable_dht: !dht_enabled,
        fastresume: true,
        persistence: Some(SessionPersistenceConfig::Json {
            folder: Some(persistence_folder),
        }),
        listen_port_range: Some(listen_port_range(listen_port)),
        enable_upnp_port_forwarding: upnp_enabled,
        ..Default::default()
    };

    Session::new_with_opts(output_folder, opts)
        .await
        .map_err(|e| ShioError::Other(format!("librqbit session: {e}")))
}

pub(crate) async fn stop_torrent_in_session(
    session: &Session,
    info_hash: [u8; 20],
    mode: TorrentSessionStop,
) -> Result<()> {
    let torrent_id = librqbit::api::TorrentIdOrHash::Hash(librqbit::dht::Id20::new(info_hash));
    if session.get(torrent_id).is_none() {
        return Ok(());
    }
    session
        .delete(torrent_id, mode.delete_files())
        .await
        .map_err(|e| ShioError::Other(format!("delete torrent session: {e}")))
}

pub(super) fn listen_port_range(listen_port: u16) -> std::ops::Range<u16> {
    let start = listen_port.min(u16::MAX - TORRENT_LISTEN_PORT_SPAN);
    start..(start + TORRENT_LISTEN_PORT_SPAN)
}
