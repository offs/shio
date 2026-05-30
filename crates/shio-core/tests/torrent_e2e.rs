use shio_core::{
    AppConfig, DownloadEngine, DownloadStatus, EngineCommand, TorrentDownloadRequest, TorrentSource,
};
use std::time::Duration;

#[tokio::test(flavor = "multi_thread")]
#[ignore = "network-dependent, slow; run with --ignored"]
async fn downloads_small_torrent_end_to_end() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db_path = tmp.path().join("shio.db");
    let cfg = AppConfig::default();
    let (engine, mut progress) = DownloadEngine::new(cfg, &db_path).unwrap();
    let tx = engine.command_sender();
    let engine_handle = tokio::spawn(engine.run());

    let magnet =
        "magnet:?xt=urn:btih:cab507494d02ebb1178b38f2e9d7be299c86b862&dn=ubuntu".to_string();
    let (reply, _ack) = tokio::sync::oneshot::channel();
    tx.send(EngineCommand::AddTorrentPrepared {
        request: TorrentDownloadRequest::new(
            TorrentSource::Magnet(magnet),
            tmp.path().to_path_buf(),
        ),
        reply,
    })
    .await
    .unwrap();

    let deadline = tokio::time::Instant::now() + Duration::from_mins(10);
    let mut saw_fetching = false;
    let mut saw_downloading = false;
    loop {
        tokio::select! {
            () = tokio::time::sleep_until(deadline) => {
                panic!("timeout without first bytes (fetching={saw_fetching}, downloading={saw_downloading})");
            }
            maybe = progress.recv() => {
                let Some(p) = maybe else { panic!("progress channel closed"); };
                match p.status {
                    DownloadStatus::FetchingMetadata => saw_fetching = true,
                    DownloadStatus::Downloading => saw_downloading = true,
                    _ => {}
                }
                if p.downloaded > 0 && saw_downloading {
                    break;
                }
            }
        }
    }

    let (cmd, ack) = EngineCommand::shutdown();
    let notified = ack.notified();
    let _ = tx.send(cmd).await;
    let _ = tokio::time::timeout(Duration::from_secs(5), notified).await;
    let _ = engine_handle.await;
}
