use shio_core::{
    AppConfig, Download, DownloadEngine, DownloadStatus, EngineCommand, HttpDownloadRequest,
};
use std::time::Duration;
use tempfile::TempDir;

fn test_config(dir: &TempDir) -> AppConfig {
    AppConfig {
        download_dir: dir.path().to_path_buf(),
        max_concurrent: 2,
        ..AppConfig::default()
    }
}

fn make_download(dir: &TempDir, url: &str) -> Download {
    let mut d = Download::new(url.to_string(), dir.path().to_path_buf());
    d.filename = "test.bin".into();
    d
}

fn add_http(download: &Download) -> EngineCommand {
    let http = download.http().unwrap();
    let (reply, _ack) = tokio::sync::oneshot::channel();
    EngineCommand::AddHttp {
        request: HttpDownloadRequest::new(http.url.clone(), download.save_path.clone())
            .with_id(download.id)
            .with_filename(download.filename.clone())
            .with_segments(http.segments)
            .with_subfolder(http.subfolder.clone()),
        reply,
    }
}

async fn shutdown_engine(tx: &tokio::sync::mpsc::Sender<EngineCommand>) {
    let (cmd, ack) = EngineCommand::shutdown();
    let notified = ack.notified();
    tx.send(cmd).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), notified)
        .await
        .unwrap();
}

#[tokio::test]
async fn engine_opens_and_shuts_down_cleanly() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("shio.db");
    let (engine, _rx) = DownloadEngine::new(test_config(&dir), &db_path).unwrap();
    let tx = engine.command_sender();
    let handle = tokio::spawn(engine.run());

    shutdown_engine(&tx).await;
    tokio::time::timeout(Duration::from_secs(2), handle)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn add_and_remove_clears_state() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("shio.db");
    let (engine, _rx) = DownloadEngine::new(test_config(&dir), &db_path).unwrap();
    let tx = engine.command_sender();

    let download = make_download(&dir, "http://127.0.0.1:1/never-resolves.bin");
    let id = download.id;
    tx.send(add_http(&download)).await.unwrap();

    let handle = tokio::spawn(engine.run());
    tokio::time::sleep(Duration::from_millis(100)).await;

    let (reply, _ack) = tokio::sync::oneshot::channel();
    tx.send(EngineCommand::Remove {
        id,
        delete_files: false,
        reply,
    })
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    shutdown_engine(&tx).await;
    tokio::time::timeout(Duration::from_secs(2), handle)
        .await
        .unwrap()
        .unwrap();

    let (engine, _rx) = DownloadEngine::new(test_config(&dir), &db_path).unwrap();
    assert!(engine.downloads().iter().all(|d| d.id != id));
}

#[tokio::test(flavor = "multi_thread")]
async fn add_torrent_persists_to_db() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db_path = tmp.path().join("shio.db");
    let cfg = shio_core::AppConfig::default();
    let (engine, progress) = shio_core::DownloadEngine::new(cfg, &db_path).unwrap();
    let tx = engine.command_sender();
    let handle = tokio::spawn(engine.run());

    let (reply, _ack) = tokio::sync::oneshot::channel();
    tx.send(shio_core::EngineCommand::AddTorrent {
        source: shio_core::TorrentSource::Magnet(
            "magnet:?xt=urn:btih:cab507494d02ebb1178b38f2e9d7be299c86b862&dn=ubuntu".into(),
        ),
        save_path: tmp.path().to_path_buf(),
        start_paused: true,
        auto_extract: true,
        reply,
    })
    .await
    .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    shutdown_engine(&tx).await;
    handle.await.unwrap();

    let (engine, _progress) =
        shio_core::DownloadEngine::new(shio_core::AppConfig::default(), &db_path).unwrap();
    let all = engine.downloads();
    assert_eq!(all.len(), 1);
    assert!(all[0].kind.is_torrent());
    assert!(all[0].torrent().is_some_and(|torrent| torrent.auto_extract));

    drop(progress);
}

#[tokio::test]
async fn opens_existing_db_twice_without_corruption() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("shio.db");

    let (engine, _rx) = DownloadEngine::new(test_config(&dir), &db_path).unwrap();
    let tx = engine.command_sender();
    let handle = tokio::spawn(engine.run());
    shutdown_engine(&tx).await;
    handle.await.unwrap();

    let (engine, _rx) = DownloadEngine::new(test_config(&dir), &db_path).unwrap();
    assert!(engine.downloads().is_empty());
}

#[tokio::test]
async fn update_metadata_persists_across_restart() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("shio.db");
    let (engine, _rx) = DownloadEngine::new(test_config(&dir), &db_path).unwrap();
    let tx = engine.command_sender();

    let download = make_download(&dir, "http://127.0.0.1:1/a.bin");
    let id = download.id;
    tx.send(add_http(&download)).await.unwrap();

    let handle = tokio::spawn(engine.run());
    tokio::time::sleep(Duration::from_millis(100)).await;

    let (reply, _ack) = tokio::sync::oneshot::channel();
    tx.send(EngineCommand::UpdateMetadata {
        id,
        filename: "renamed.bin".into(),
        save_path: dir.path().to_path_buf(),
        reply,
    })
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    shutdown_engine(&tx).await;
    tokio::time::timeout(Duration::from_secs(2), handle)
        .await
        .unwrap()
        .unwrap();

    let (engine, _rx) = DownloadEngine::new(test_config(&dir), &db_path).unwrap();
    let found = engine.downloads().into_iter().find(|d| d.id == id).unwrap();
    assert_eq!(found.filename, "renamed.bin");
}

#[tokio::test]
async fn shutdown_ack_waits_for_worker_final_status_write() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("shio.db");
    let (engine, _rx) = DownloadEngine::new(test_config(&dir), &db_path).unwrap();
    let tx = engine.command_sender();

    let download = make_download(&dir, "http://127.0.0.1:1/fails.bin");
    let id = download.id;
    tx.send(add_http(&download)).await.unwrap();

    let handle = tokio::spawn(engine.run());
    tokio::time::sleep(Duration::from_millis(300)).await;
    shutdown_engine(&tx).await;
    tokio::time::timeout(Duration::from_secs(2), handle)
        .await
        .unwrap()
        .unwrap();

    let (engine, _rx) = DownloadEngine::new(test_config(&dir), &db_path).unwrap();
    let status = engine
        .downloads()
        .into_iter()
        .find(|d| d.id == id)
        .map(|d| d.status);

    assert!(matches!(
        status,
        Some(DownloadStatus::Error | DownloadStatus::Cancelled)
    ));
}
