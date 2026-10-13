use std::collections::{BTreeMap, HashMap};
use std::net::Ipv6Addr;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Once};
use std::time::Duration;

use librqbit::api::TorrentIdOrHash;
use librqbit::dht::{DhtPersistenceConfig, Id20};
use librqbit::{
    AddTorrent, AddTorrentOptions, AddTorrentResponse, DhtSessionConfig, ListenerOptions,
    ManagedTorrentState, Session, SessionOptions, SessionPersistenceConfig,
};
use serde::{Deserialize, Serialize};

use super::metainfo::{hex, parse_magnet_info_hash, parse_torrent_manifest};
use super::staging::payload_dir;
use crate::config::TorrentConfig;
use crate::error::{Result, ShioError};
use crate::types::{Download, MagnetPreviewManifest};

const SESSION_OP_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) fn librqbit_rate_limit(limit: Option<u64>) -> Option<NonZeroU32> {
    limit.and_then(|limit| NonZeroU32::new(limit.min(u64::from(u32::MAX)) as u32))
}

pub(crate) async fn open_session(
    data_dir: &Path,
    config: &TorrentConfig,
    speed_limit: Option<u64>,
    owned: &[Download],
) -> Result<Arc<Session>> {
    static NOFILE: Once = Once::new();
    NOFILE.call_once(|| match librqbit::try_increase_nofile_limit() {
        Ok(limit) => tracing::info!(limit, "open file limit"),
        Err(error) => tracing::warn!("failed to raise the open file limit: {error:#}"),
    });

    let persistence = data_dir.join("rqbit-session");
    let expected = owned
        .iter()
        .filter_map(|download| {
            let torrent = download.torrent()?;
            Some((hex(&torrent.info_hash), payload_dir(download)))
        })
        .collect::<HashMap<_, _>>();
    let folder = persistence.clone();
    tokio::task::spawn_blocking(move || {
        crate::engine::secure_private_directory(&folder)?;
        filter_persisted_session(&folder, &expected)
    })
    .await
    .map_err(|error| ShioError::Other(format!("torrent session setup failed: {error}")))??;

    let opts = SessionOptions {
        dht: config.dht.then(|| DhtSessionConfig {
            persistence: Some(DhtPersistenceConfig {
                config_filename: Some(data_dir.join("dht.json")),
                ..Default::default()
            }),
            ..Default::default()
        }),
        disable_local_service_discovery: !config.dht,
        fastresume: true,
        persistence: Some(SessionPersistenceConfig::Json {
            folder: Some(persistence),
        }),
        listen: Some(ListenerOptions {
            listen_addr: (Ipv6Addr::UNSPECIFIED, listen_port(config.listen_port)).into(),
            enable_upnp_port_forwarding: config.upnp,
            ..Default::default()
        }),
        ratelimits: librqbit::limits::LimitsConfig {
            download_bps: librqbit_rate_limit(speed_limit),
            upload_bps: None,
        },
        ..Default::default()
    };
    Session::new_with_opts(data_dir.to_path_buf(), opts)
        .await
        .map_err(|e| ShioError::Other(format!("librqbit session: {e:#}")))
}

fn listen_port(port: u16) -> u16 {
    match std::net::TcpListener::bind((Ipv6Addr::UNSPECIFIED, port)) {
        Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
            tracing::warn!(
                port,
                "torrent listen port is in use; listening on a random port"
            );
            0
        },
        _ => port,
    }
}

/// Mirrors librqbit's `SerializedSessionDatabase` at the pinned rev; re-check on upgrade.
#[derive(Serialize, Deserialize)]
struct PersistedSession {
    torrents: BTreeMap<usize, PersistedTorrent>,
}

#[derive(Serialize, Deserialize)]
struct PersistedTorrent {
    info_hash: String,
    trackers: Vec<String>,
    output_folder: PathBuf,
    only_files: Option<Vec<usize>>,
    is_paused: bool,
}

// librqbit restores every persisted torrent and creates its files on open, even paused ones.
// Dropping entries Shio does not own at that folder keeps them out of the user's directory;
// restoring the rest paused keeps them under the engine's queue.
fn filter_persisted_session(folder: &Path, expected: &HashMap<String, PathBuf>) -> Result<()> {
    let path = folder.join("session.json");
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    let Ok(mut session) = serde_json::from_slice::<PersistedSession>(&bytes) else {
        tracing::warn!(path = %path.display(), "torrent session file is corrupt; moving it aside");
        std::fs::rename(&path, folder.join("session.json.corrupt"))?;
        return Ok(());
    };

    let mut changed = false;
    session.torrents.retain(|_, torrent| {
        let hash = &torrent.info_hash;
        // Without its saved metadata librqbit would resolve the magnet and write unvalidated files.
        let keep = expected.get(hash) == Some(&torrent.output_folder)
            && folder.join(format!("{hash}.torrent")).is_file();
        if !keep && hash.len() == 40 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            for extension in ["torrent", "bitv"] {
                let file = folder.join(format!("{hash}.{extension}"));
                if let Err(error) = std::fs::remove_file(&file)
                    && error.kind() != std::io::ErrorKind::NotFound
                {
                    tracing::warn!(path = %file.display(), "failed to remove stale torrent session file: {error}");
                }
            }
        }
        changed |= !keep || !torrent.is_paused;
        torrent.is_paused = true;
        keep
    });
    if !changed {
        return Ok(());
    }

    let json = serde_json::to_vec(&session)
        .map_err(|error| ShioError::Other(format!("torrent session encode: {error}")))?;
    let tmp = folder.join("session.json.shio-tmp");
    {
        use std::io::Write as _;
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(&json)?;
        file.sync_all()?;
    }
    std::fs::rename(&tmp, &path)?;
    #[cfg(unix)]
    std::fs::File::open(folder)?.sync_all()?;
    Ok(())
}

pub(super) fn torrent_id(info_hash: [u8; 20]) -> TorrentIdOrHash {
    TorrentIdOrHash::Hash(Id20::new(info_hash))
}

async fn with_timeout<E: std::fmt::Display>(
    operation: &str,
    future: impl Future<Output = std::result::Result<(), E>>,
) -> Result<()> {
    tokio::time::timeout(SESSION_OP_TIMEOUT, future)
        .await
        .map_err(|_| ShioError::Other(format!("{operation} timed out")))?
        .map_err(|e| ShioError::Other(format!("{operation}: {e:#}")))
}

pub(crate) async fn stop_torrent_in_session(session: &Session, info_hash: [u8; 20]) -> Result<()> {
    if session.get(torrent_id(info_hash)).is_none() {
        return Ok(());
    }
    with_timeout(
        "delete torrent session",
        session.delete(torrent_id(info_hash), false),
    )
    .await
}

pub(super) async fn pause_torrent_in_session(session: &Session, info_hash: [u8; 20]) -> Result<()> {
    let Some(handle) = session.get(torrent_id(info_hash)) else {
        return Ok(());
    };
    if handle.is_paused()
        || handle.with_state(|state| matches!(state, ManagedTorrentState::Error(_)))
    {
        return Ok(());
    }
    with_timeout("pause torrent session", session.pause(&handle)).await
}

/// Resolves magnet metadata without adding the torrent; callers bound it with cancellation.
pub(super) async fn resolve_magnet(session: &Arc<Session>, magnet: &str) -> Result<Vec<u8>> {
    let info_hash = parse_magnet_info_hash(magnet)?;
    if let Some(bytes) = session
        .get(torrent_id(info_hash))
        .and_then(|handle| handle.with_metadata(|m| m.torrent_bytes.to_vec()).ok())
    {
        return Ok(bytes);
    }
    let options = AddTorrentOptions {
        list_only: true,
        ..Default::default()
    };
    match session
        .add_torrent(AddTorrent::from_url(magnet), Some(options))
        .await
        .map_err(|e| ShioError::Other(format!("magnet metadata: {e:#}")))?
    {
        AddTorrentResponse::ListOnly(response) => Ok(response.torrent_bytes.to_vec()),
        AddTorrentResponse::Added(..) | AddTorrentResponse::AlreadyManaged(..) => Err(
            ShioError::Other("magnet metadata: librqbit added the torrent".into()),
        ),
    }
}

pub(crate) async fn resolve_magnet_preview(
    session: &Arc<Session>,
    magnet: &str,
) -> Result<MagnetPreviewManifest> {
    let metadata_bytes = resolve_magnet(session, magnet).await?;
    let manifest = parse_torrent_manifest(&metadata_bytes)?;
    Ok(MagnetPreviewManifest {
        name: manifest.name,
        total_size: manifest.total_size,
        is_private: manifest.is_private,
        files: manifest.files,
        trackers: manifest.trackers,
        metadata_bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persisted_session_keeps_only_owned_entries_and_pauses_them() {
        let temp = tempfile::tempdir().expect("tempdir");
        let owned = "aa".repeat(20);
        let removed = "bb".repeat(20);
        let moved = "cc".repeat(20);
        let unresolved = "dd".repeat(20);
        let entry = |hash: &str, folder: &str| {
            serde_json::json!({
                "info_hash": hash, "trackers": [], "output_folder": folder,
                "only_files": null, "is_paused": false,
            })
        };
        let session = serde_json::json!({ "torrents": {
            "0": entry(&owned, "/d/.shio-1.torrent/payload"),
            "1": entry(&removed, "/d/.shio-2.torrent/payload"),
            "2": entry(&moved, "/d"),
            "3": entry(&unresolved, "/d/.shio-4.torrent/payload"),
        }});
        std::fs::write(temp.path().join("session.json"), session.to_string()).expect("session");
        std::fs::write(temp.path().join(format!("{owned}.torrent")), b"meta").expect("torrent");
        std::fs::write(temp.path().join(format!("{removed}.bitv")), b"bits").expect("bitv");
        let expected = HashMap::from([
            (owned.clone(), PathBuf::from("/d/.shio-1.torrent/payload")),
            (moved, PathBuf::from("/d/.shio-3.torrent/payload")),
            (unresolved, PathBuf::from("/d/.shio-4.torrent/payload")),
        ]);

        filter_persisted_session(temp.path(), &expected).expect("filter");

        let filtered: PersistedSession =
            serde_json::from_slice(&std::fs::read(temp.path().join("session.json")).expect("read"))
                .expect("json");
        let kept = filtered.torrents.values().collect::<Vec<_>>();
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].info_hash, owned);
        assert!(kept[0].is_paused);
        assert!(!temp.path().join(format!("{removed}.bitv")).exists());
    }

    #[test]
    fn corrupt_session_file_is_moved_aside() {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::write(temp.path().join("session.json"), b"{\"torr").expect("session");

        filter_persisted_session(temp.path(), &HashMap::new()).expect("filter");

        assert!(!temp.path().join("session.json").exists());
        assert!(temp.path().join("session.json.corrupt").exists());
    }
}
