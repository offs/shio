use shio_core::{
    AppConfig, Download, DownloadEngine, DownloadStatus, EngineCommand, HttpDownloadRequest,
};
use std::io::Write as _;
use std::net::SocketAddr;
use std::time::Duration;
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

const BODY_LEN: usize = 5 * 1024 * 1024;

fn deterministic_body() -> Vec<u8> {
    (0..BODY_LEN).map(|i| (i % 251) as u8).collect()
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

async fn read_request(stream: &mut TcpStream) -> (String, Vec<(String, String)>) {
    let (reader, _) = stream.split();
    let mut reader = BufReader::new(reader);

    let mut request_line = String::new();
    reader.read_line(&mut request_line).await.unwrap();

    let mut headers = Vec::new();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some((k, v)) = trimmed.split_once(':') {
            headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
        }
    }

    (request_line, headers)
}

async fn serve_full_body(addr: SocketAddr, body: Vec<u8>) {
    let listener = TcpListener::bind(addr).await.unwrap();
    loop {
        let Ok((mut stream, _)) = listener.accept().await else {
            return;
        };
        let body = body.clone();
        tokio::spawn(async move {
            let (_, headers) = read_request(&mut stream).await;
            let has_range = headers.iter().any(|(k, _)| k == "range");
            let mut response = Vec::new();
            if has_range {
                let _ = write!(
                    response,
                    "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\nContent-Range: bytes 0-{}/{}\r\n\r\n",
                    body.len(),
                    body.len() - 1,
                    body.len()
                );
            } else {
                let _ = write!(
                    response,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\n\r\n",
                    body.len()
                );
            }
            response.extend_from_slice(&body);
            let _ = stream.write_all(&response).await;
            let _ = stream.shutdown().await;
        });
    }
}

fn range_header(headers: &[(String, String)]) -> Option<(usize, usize)> {
    let value = headers
        .iter()
        .find(|(k, _)| k == "range")
        .map(|(_, v)| v.as_str())?;
    let range = value.strip_prefix("bytes=")?;
    let (start, end) = range.split_once('-')?;
    Some((start.parse().ok()?, end.parse().ok()?))
}

async fn serve_ranges(addr: SocketAddr, body: Vec<u8>) {
    let listener = TcpListener::bind(addr).await.unwrap();
    loop {
        let Ok((mut stream, _)) = listener.accept().await else {
            return;
        };
        let body = body.clone();
        tokio::spawn(async move {
            let (_, headers) = read_request(&mut stream).await;
            let (start, end) = range_header(&headers).unwrap_or((0, body.len() - 1));
            let part = &body[start..=end];
            let mut response = Vec::new();
            let _ = write!(
                response,
                "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\nContent-Range: bytes {}-{}/{}\r\n\r\n",
                part.len(),
                start,
                end,
                body.len()
            );
            response.extend_from_slice(part);
            let _ = stream.write_all(&response).await;
            let _ = stream.shutdown().await;
        });
    }
}

async fn serve_ignored_ranges(addr: SocketAddr, body: Vec<u8>) {
    let listener = TcpListener::bind(addr).await.unwrap();
    loop {
        let Ok((mut stream, _)) = listener.accept().await else {
            return;
        };
        let body = body.clone();
        tokio::spawn(async move {
            let _ = read_request(&mut stream).await;
            let mut response = Vec::new();
            let _ = write!(
                response,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\n\r\n",
                body.len()
            );
            response.extend_from_slice(&body);
            let _ = stream.write_all(&response).await;
            let _ = stream.shutdown().await;
        });
    }
}

async fn serve_wrong_content_range(addr: SocketAddr, body: Vec<u8>) {
    let listener = TcpListener::bind(addr).await.unwrap();
    loop {
        let Ok((mut stream, _)) = listener.accept().await else {
            return;
        };
        let body = body.clone();
        tokio::spawn(async move {
            let (_, headers) = read_request(&mut stream).await;
            let (start, end) = range_header(&headers).unwrap_or((0, body.len() - 1));
            let part = &body[start..=end];
            let mut response = Vec::new();
            let _ = write!(
                response,
                "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\nContent-Range: bytes 0-{}/{}\r\n\r\n",
                part.len(),
                part.len() - 1,
                body.len()
            );
            response.extend_from_slice(part);
            let _ = stream.write_all(&response).await;
            let _ = stream.shutdown().await;
        });
    }
}

async fn bind_ephemeral() -> (SocketAddr, TcpListener) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    (addr, listener)
}

fn test_config(dir: &TempDir) -> AppConfig {
    AppConfig {
        download_dir: dir.path().to_path_buf(),
        max_concurrent: 1,
        default_segments: 1,
        ..AppConfig::default()
    }
}

async fn run_download(
    dir: &TempDir,
    url: String,
    segments: u8,
) -> (DownloadStatus, std::path::PathBuf) {
    let db_path = dir.path().join("shio.db");
    let mut config = test_config(dir);
    config.default_segments = segments;
    let mut download = Download::new(url, dir.path().to_path_buf());
    download.filename = "file.bin".into();
    if let Some(http) = download.http_mut() {
        http.segments = segments;
    }
    let id = download.id;
    let expected_path = download.file_path();
    let (engine, mut progress) = DownloadEngine::new(config, &db_path).unwrap();
    let tx = engine.command_sender();
    let handle = tokio::spawn(engine.run());
    tx.send(add_http(&download)).await.unwrap();

    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    let mut status = DownloadStatus::Pending;
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(500), progress.recv()).await {
            Ok(Some(p)) if p.id == id && (p.status.is_terminal() || p.status.is_failed()) => {
                status = p.status;
                break;
            },
            Ok(None) => break,
            _ => {},
        }
    }

    let (cmd, ack) = EngineCommand::shutdown();
    let notified = ack.notified();
    tx.send(cmd).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), notified)
        .await
        .unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;
    (status, expected_path)
}

#[tokio::test]
async fn downloads_full_body_and_reaches_completed() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("shio.db");

    let (addr, listener) = bind_ephemeral().await;
    drop(listener);
    let body = deterministic_body();
    let server_body = body.clone();
    tokio::spawn(async move { serve_full_body(addr, server_body).await });

    let url = format!("http://{addr}/file.bin");
    let mut download = Download::new(url, dir.path().to_path_buf());
    download.filename = "file.bin".into();
    if let Some(http) = download.http_mut() {
        http.segments = 1;
    }
    let id = download.id;
    let expected_path = download.file_path();

    let (engine, mut progress) = DownloadEngine::new(test_config(&dir), &db_path).unwrap();
    let tx = engine.command_sender();
    let handle = tokio::spawn(engine.run());

    tx.send(add_http(&download)).await.unwrap();

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let mut completed = false;
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(500), progress.recv()).await {
            Ok(Some(p)) if p.id == id && matches!(p.status, DownloadStatus::Completed) => {
                completed = true;
                break;
            },
            Ok(None) => break,
            _ => {},
        }
    }

    let (cmd, ack) = EngineCommand::shutdown();
    let notified = ack.notified();
    tx.send(cmd).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), notified)
        .await
        .unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;

    assert!(completed, "download did not complete within 10s");
    let on_disk = std::fs::read(&expected_path).expect("file written");
    assert_eq!(on_disk.len(), body.len());
    assert_eq!(on_disk, body);
}

#[tokio::test]
async fn downloads_multi_segment_ranges_to_exact_file() {
    let dir = TempDir::new().unwrap();
    let (addr, listener) = bind_ephemeral().await;
    drop(listener);
    let body = deterministic_body();
    tokio::spawn(serve_ranges(addr, body.clone()));

    let (status, expected_path) = run_download(&dir, format!("http://{addr}/file.bin"), 4).await;

    assert_eq!(status, DownloadStatus::Completed);
    assert_eq!(std::fs::read(expected_path).unwrap(), body);
}

#[tokio::test]
async fn ignored_range_request_fails_instead_of_completing_corrupt_file() {
    let dir = TempDir::new().unwrap();
    let (addr, listener) = bind_ephemeral().await;
    drop(listener);
    let body = deterministic_body();
    tokio::spawn(serve_ignored_ranges(addr, body));

    let (status, _path) = run_download(&dir, format!("http://{addr}/file.bin"), 4).await;

    assert_eq!(status, DownloadStatus::Error);
}

#[tokio::test]
async fn wrong_content_range_fails_instead_of_completing_corrupt_file() {
    let dir = TempDir::new().unwrap();
    let (addr, listener) = bind_ephemeral().await;
    drop(listener);
    let body = deterministic_body();
    tokio::spawn(serve_wrong_content_range(addr, body));

    let (status, _path) = run_download(&dir, format!("http://{addr}/file.bin"), 4).await;

    assert_eq!(status, DownloadStatus::Error);
}
