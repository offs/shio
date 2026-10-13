use shio_core::{
    AppConfig, DownloadEngine, DownloadStatus, EngineCommand, ProgressEvent,
    TorrentDownloadRequest, TorrentSource,
};
use std::time::Duration;

#[tokio::test(flavor = "multi_thread")]
#[ignore = "network-dependent, slow; run with --ignored"]
async fn downloads_small_torrent_end_to_end() {
    let tmp = tempfile::TempDir::new().unwrap();
    let config = AppConfig {
        download_dir: tmp.path().join("downloads"),
        ..AppConfig::default()
    };
    let (engine, tx, mut progress) = DownloadEngine::new(config, &tmp.path().join("data")).unwrap();
    let engine_handle = tokio::spawn(engine.run());

    let magnet = "magnet:?xt=urn:btih:cab507494d02ebb1178b38f2e9d7be299c86b862&dn=ubuntu";
    let (reply, ack) = tokio::sync::oneshot::channel();
    tx.send(EngineCommand::AddTorrentPrepared {
        request: TorrentDownloadRequest::new(
            TorrentSource::Magnet(magnet.into()),
            tmp.path().join("downloads"),
        ),
        reply,
    })
    .await
    .unwrap();
    ack.await.unwrap().unwrap();

    let deadline = tokio::time::Instant::now() + Duration::from_mins(10);
    let mut saw_metadata = false;
    loop {
        tokio::select! {
            () = tokio::time::sleep_until(deadline) => {
                panic!("timeout without first bytes (metadata={saw_metadata})");
            }
            event = progress.recv() => {
                let Some(event) = event else { panic!("engine stopped") };
                let ProgressEvent::Update(p) = event else { continue; };
                saw_metadata |= p.status == DownloadStatus::FetchingMetadata;
                if p.status == DownloadStatus::Downloading && p.downloaded > 0 {
                    break;
                }
            }
        }
    }

    let (reply, done) = tokio::sync::oneshot::channel();
    tx.send(EngineCommand::Shutdown { reply }).await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), done)
        .await
        .unwrap()
        .unwrap();
    engine_handle.await.unwrap();
}
