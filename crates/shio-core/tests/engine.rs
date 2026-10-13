use std::time::Duration;

use shio_core::{
    AppConfig, Download, DownloadEngine, DownloadId, DownloadStatus, EngineCommand,
    HttpArchivePartRequest, HttpArchiveSetRequest, HttpDownloadRequest, ProgressEvent,
    ProgressStream,
};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::sync::{mpsc, oneshot};

const TIMEOUT: Duration = Duration::from_secs(20);
const PAYLOAD_LEN: usize = 256 * 1024;

struct Running {
    tx: mpsc::Sender<EngineCommand>,
    events: ProgressStream,
    task: tokio::task::JoinHandle<()>,
}

fn start(dir: &TempDir) -> Running {
    let config = AppConfig {
        download_dir: dir.path().to_path_buf(),
        max_concurrent: 2,
        max_retries: 0,
        ..AppConfig::default()
    };
    let (engine, tx, events) = DownloadEngine::new(config, dir.path()).unwrap();
    Running {
        tx,
        events,
        task: tokio::spawn(engine.run()),
    }
}

impl Running {
    async fn send(
        &self,
        command: impl FnOnce(oneshot::Sender<shio_core::Result<()>>) -> EngineCommand,
    ) -> shio_core::Result<()> {
        let (reply, result) = oneshot::channel();
        self.tx.send(command(reply)).await.unwrap();
        result.await.unwrap()
    }

    async fn recv(&mut self) -> ProgressEvent {
        tokio::time::timeout(TIMEOUT, self.events.recv())
            .await
            .expect("event before timeout")
            .expect("engine running")
    }

    async fn resync(&mut self) -> Vec<Download> {
        match self.recv().await {
            ProgressEvent::Resync { downloads, .. } => downloads,
            other => panic!("a stream starts with a resync, got {other:?}"),
        }
    }

    async fn next_status(&mut self, id: DownloadId) -> DownloadStatus {
        loop {
            if let ProgressEvent::Changed { downloads, .. } = self.recv().await
                && let Some(row) = downloads.iter().find(|row| row.id == id)
            {
                return row.status;
            }
        }
    }

    async fn shutdown(self) {
        let (reply, done) = oneshot::channel();
        self.tx
            .send(EngineCommand::Shutdown { reply })
            .await
            .unwrap();
        tokio::time::timeout(TIMEOUT, done).await.unwrap().unwrap();
        self.task.await.unwrap();
    }
}

fn payload() -> Vec<u8> {
    (0..PAYLOAD_LEN).map(|i| (i % 251) as u8).collect()
}

// Throttled to about 400 KiB/s so a transfer stays active long enough to pause.
async fn slow_server() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(serve(stream));
        }
    });
    format!("http://{address}/test.bin")
}

async fn serve(mut stream: tokio::net::TcpStream) {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 1024];
    while !request.windows(4).any(|window| window == b"\r\n\r\n") {
        match stream.read(&mut buffer).await {
            Ok(0) | Err(_) => return,
            Ok(read) => request.extend_from_slice(&buffer[..read]),
        }
    }
    let request = String::from_utf8_lossy(&request).to_ascii_lowercase();
    let range = request.lines().find_map(|line| {
        let (start, end) = line.strip_prefix("range: bytes=")?.trim().split_once('-')?;
        let start: usize = start.parse().ok()?;
        let end = end
            .parse::<usize>()
            .map_or(PAYLOAD_LEN - 1, |end| end.min(PAYLOAD_LEN - 1));
        Some((start, end))
    });
    let body = payload();
    let (status, body) = match range {
        Some((start, end)) => (
            format!("206 Partial Content\r\nContent-Range: bytes {start}-{end}/{PAYLOAD_LEN}"),
            &body[start..=end],
        ),
        None => ("200 OK".to_string(), &body[..]),
    };
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\nETag: \"v1\"\r\n\
         Content-Type: application/octet-stream\r\nConnection: close\r\n\r\n",
        body.len()
    );
    if stream.write_all(head.as_bytes()).await.is_err() || request.starts_with("head ") {
        return;
    }
    for chunk in body.chunks(4096) {
        if stream.write_all(chunk).await.is_err() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn add_active_download(engine: &mut Running, dir: &TempDir) -> DownloadId {
    let request = HttpDownloadRequest::new(slow_server().await, dir.path().to_path_buf())
        .with_filename("test.bin".into())
        .with_segments(1);
    let id = request.id();
    engine
        .send(|reply| EngineCommand::AddHttp { request, reply })
        .await
        .unwrap();
    loop {
        if let ProgressEvent::Update(progress) = engine.recv().await
            && progress.id == id
            && progress.downloaded > 0
        {
            return id;
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn pausing_an_active_download_keeps_it_paused_and_resumable() {
    let dir = TempDir::new().unwrap();
    let mut engine = start(&dir);
    let id = add_active_download(&mut engine, &dir).await;

    engine
        .send(|reply| EngineCommand::Pause { id, reply })
        .await
        .unwrap();
    // The command commits Paused; the stopped worker's exit then commits again.
    assert_eq!(engine.next_status(id).await, DownloadStatus::Paused);
    assert_eq!(engine.next_status(id).await, DownloadStatus::Paused);

    engine
        .send(|reply| EngineCommand::Resume { id, reply })
        .await
        .unwrap();
    while engine.next_status(id).await != DownloadStatus::Completed {}
    engine.shutdown().await;

    assert_eq!(
        std::fs::read(dir.path().join("test.bin")).unwrap(),
        payload()
    );
    let mut restarted = start(&dir);
    assert_eq!(
        restarted.resync().await[0].status,
        DownloadStatus::Completed
    );
    restarted.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn resuming_while_the_paused_worker_drains_runs_one_worker() {
    let dir = TempDir::new().unwrap();
    let mut engine = start(&dir);
    let id = add_active_download(&mut engine, &dir).await;

    engine
        .send(|reply| EngineCommand::Pause { id, reply })
        .await
        .unwrap();
    engine
        .send(|reply| EngineCommand::Resume { id, reply })
        .await
        .unwrap();

    loop {
        match engine.next_status(id).await {
            DownloadStatus::Completed => break,
            DownloadStatus::Cancelled | DownloadStatus::Error => panic!("resume was lost"),
            _ => {},
        }
    }
    engine.shutdown().await;
    assert_eq!(
        std::fs::read(dir.path().join("test.bin")).unwrap(),
        payload()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn package_commands_apply_to_the_parts_they_fit() {
    let dir = TempDir::new().unwrap();
    let mut engine = start(&dir);
    let part = |url: String, number| {
        HttpArchivePartRequest::new(url, format!("set.part0{number}.rar"), number, 1).unwrap()
    };
    let request = HttpArchiveSetRequest::new(
        "set".into(),
        dir.path().to_path_buf(),
        vec![
            part(slow_server().await, 1),
            part("http://127.0.0.1:1/missing.rar".into(), 2),
        ],
        false,
    )
    .unwrap();
    let package = request.id();
    let row = package.row_id();
    engine
        .send(|reply| EngineCommand::AddHttpArchiveSet { request, reply })
        .await
        .unwrap();
    let (downloads, packages) = loop {
        if let ProgressEvent::Changed {
            downloads,
            packages,
        } = engine.recv().await
        {
            break (downloads, packages);
        }
    };
    let parts: Vec<DownloadId> = packages[0].child_ids().collect();
    assert_eq!(parts, downloads.iter().map(|d| d.id).collect::<Vec<_>>());
    let (mut completed, mut failed) = (false, false);
    while !(completed && failed) {
        if let ProgressEvent::Changed { downloads, .. } = engine.recv().await {
            for row in downloads {
                completed |= row.id == parts[0] && row.status == DownloadStatus::Completed;
                failed |= row.id == parts[1] && row.status == DownloadStatus::Error;
            }
        }
    }

    engine
        .send(|reply| EngineCommand::Cancel { id: row, reply })
        .await
        .unwrap();
    assert_eq!(
        engine.next_status(parts[1]).await,
        DownloadStatus::Cancelled
    );
    assert!(
        engine
            .send(|reply| EngineCommand::Pause { id: row, reply })
            .await
            .is_err()
    );

    engine
        .send(|reply| EngineCommand::Remove {
            id: row,
            delete_files: true,
            reply,
        })
        .await
        .unwrap();
    let removed = loop {
        if let ProgressEvent::Removed {
            downloads,
            packages,
        } = engine.recv().await
        {
            break (downloads, packages);
        }
    };
    assert_eq!(removed, (parts, vec![package]));
    assert!(!dir.path().join("set/set.part01.rar").exists());
    engine.shutdown().await;
}

#[tokio::test]
async fn invalid_adds_are_rejected_before_anything_is_stored() {
    let dir = TempDir::new().unwrap();
    let mut engine = start(&dir);
    assert_eq!(engine.resync().await, Vec::<Download>::new());
    std::fs::write(dir.path().join("taken.bin"), b"user file").unwrap();

    for url in [
        "ftp://example.com/a.bin",
        "not a url",
        "https://example.com/taken.bin",
    ] {
        let request = HttpDownloadRequest::new(url.into(), dir.path().to_path_buf());
        let added = engine
            .send(|reply| EngineCommand::AddHttp { request, reply })
            .await;
        assert!(added.is_err(), "{url}");
    }
    engine.shutdown().await;

    let mut restarted = start(&dir);
    assert_eq!(restarted.resync().await, Vec::<Download>::new());
    assert_eq!(
        std::fs::read(dir.path().join("taken.bin")).unwrap(),
        b"user file"
    );
    restarted.shutdown().await;
}

#[tokio::test]
async fn dropping_every_command_sender_stops_the_engine() {
    let dir = TempDir::new().unwrap();
    let Running {
        tx,
        mut events,
        task,
    } = start(&dir);
    drop(tx);

    tokio::time::timeout(TIMEOUT, task).await.unwrap().unwrap();
    assert!(matches!(
        events.recv().await,
        Some(ProgressEvent::Resync { .. })
    ));
    assert!(events.recv().await.is_none());
}
