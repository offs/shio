use shio_core::{
    AppConfig, DownloadEngine, EngineCommand, HttpPreview, HttpPreviewResult, HttpPreviewState,
};
use std::io::Write as _;
use std::net::SocketAddr;
use std::time::Duration;
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

const PREVIEW_TOKEN: &str = "secret-preview-token";

async fn read_request(stream: &mut TcpStream) -> std::io::Result<()> {
    let (reader, _) = stream.split();
    let mut reader = BufReader::new(reader);

    let mut request_line = String::new();
    reader.read_line(&mut request_line).await?;

    loop {
        let mut line = String::new();
        reader.read_line(&mut line).await?;
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
    }

    Ok(())
}

async fn bind_ephemeral() -> (SocketAddr, TcpListener) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    (addr, listener)
}

async fn write_response(stream: &mut TcpStream, response: &[u8]) -> std::io::Result<()> {
    stream.write_all(response).await?;
    stream.shutdown().await
}

async fn serve_probe_file(listener: TcpListener) -> std::io::Result<()> {
    let (mut stream, _) = listener.accept().await?;
    read_request(&mut stream).await?;
    let mut response = Vec::new();
    write!(
        response,
        "HTTP/1.1 206 Partial Content\r\nContent-Length: 2\r\nAccept-Ranges: bytes\r\nContent-Range: bytes 0-1/1234\r\nContent-Type: application/octet-stream\r\nContent-Disposition: attachment; filename=\"preview-file.bin\"\r\n\r\nab"
    )?;
    write_response(&mut stream, &response).await
}

async fn serve_probe_file_without_disposition(listener: TcpListener) -> std::io::Result<()> {
    let (mut stream, _) = listener.accept().await?;
    read_request(&mut stream).await?;
    let mut response = Vec::new();
    write!(
        response,
        "HTTP/1.1 206 Partial Content\r\nContent-Length: 2\r\nAccept-Ranges: bytes\r\nContent-Range: bytes 0-1/1234\r\nContent-Type: application/octet-stream\r\n\r\nab"
    )?;
    write_response(&mut stream, &response).await
}

async fn serve_html_landing_page(listener: TcpListener) -> std::io::Result<()> {
    let (mut stream, _) = listener.accept().await?;
    read_request(&mut stream).await?;
    let body = b"<html><a href=\"/real-file.bin\">download</a></html>";
    let mut response = Vec::new();
    write!(
        response,
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: text/html\r\n\r\n",
        body.len()
    )?;
    response.extend_from_slice(body);
    write_response(&mut stream, &response).await
}

async fn serve_redirect_to_tokenized_file(
    listener: TcpListener,
    addr: SocketAddr,
    token: &'static str,
) -> std::io::Result<()> {
    let (mut stream, _) = listener.accept().await?;
    read_request(&mut stream).await?;
    let mut response = Vec::new();
    write!(
        response,
        "HTTP/1.1 302 Found\r\nLocation: http://{addr}/cdn/file.bin?token={token}\r\nContent-Length: 0\r\n\r\n"
    )?;
    write_response(&mut stream, &response).await?;

    let (mut stream, _) = listener.accept().await?;
    read_request(&mut stream).await?;
    let mut response = Vec::new();
    write!(
        response,
        "HTTP/1.1 206 Partial Content\r\nContent-Length: 2\r\nAccept-Ranges: bytes\r\nContent-Range: bytes 0-1/1234\r\nContent-Type: application/octet-stream\r\nContent-Disposition: attachment; filename=\"preview-file.bin\"\r\n\r\nab"
    )?;
    write_response(&mut stream, &response).await
}

async fn serve_slow_probe_file(
    listener: TcpListener,
    accepted_tx: tokio::sync::oneshot::Sender<()>,
    release_rx: tokio::sync::oneshot::Receiver<()>,
) -> std::io::Result<()> {
    let (mut stream, _) = listener.accept().await?;
    read_request(&mut stream).await?;
    accepted_tx.send(()).expect("signal accepted slow request");
    release_rx.await.map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "slow server release dropped",
        )
    })?;

    let mut response = Vec::new();
    write!(
        response,
        "HTTP/1.1 206 Partial Content\r\nContent-Length: 2\r\nAccept-Ranges: bytes\r\nContent-Range: bytes 0-1/1234\r\nContent-Type: application/octet-stream\r\nContent-Disposition: attachment; filename=\"late-file.bin\"\r\n\r\nab"
    )?;
    write_response(&mut stream, &response).await
}

async fn signal_if_request_arrives(
    listener: TcpListener,
    accepted_tx: tokio::sync::oneshot::Sender<()>,
) -> std::io::Result<()> {
    let (_stream, _) = listener.accept().await?;
    accepted_tx.send(()).expect("signal unexpected request");
    Ok(())
}

async fn await_server(handle: tokio::task::JoinHandle<std::io::Result<()>>) -> std::io::Result<()> {
    tokio::time::timeout(Duration::from_secs(2), handle)
        .await
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "server timed out"))?
        .map_err(|error| std::io::Error::other(format!("server task panicked: {error}")))?
}

async fn shutdown_engine(
    tx: &tokio::sync::mpsc::Sender<EngineCommand>,
    handle: tokio::task::JoinHandle<()>,
) {
    let (cmd, ack) = EngineCommand::shutdown();
    let notified = ack.notified();
    tx.send(cmd).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), notified)
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), handle)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn http_preview_uses_fragment_filename_for_direct_multifile_url() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("shio.db");

    let (addr, listener) = bind_ephemeral().await;
    let server = tokio::spawn(serve_probe_file_without_disposition(listener));

    let url = format!("http://{addr}/opaque-id#release.part01.rar");
    let (engine, _progress) = DownloadEngine::new(AppConfig::default(), &db_path).unwrap();
    let tx = engine.command_sender();
    let handle = tokio::spawn(engine.run());
    let (reply, mut preview_rx) = tokio::sync::mpsc::channel::<HttpPreviewResult>(1);

    tx.send(EngineCommand::ResolveHttpPreview {
        request_id: 41,
        url,
        reply,
    })
    .await
    .unwrap();

    let result = tokio::time::timeout(Duration::from_secs(2), preview_rx.recv())
        .await
        .unwrap()
        .unwrap();
    shutdown_engine(&tx, handle).await;
    await_server(server).await.unwrap();

    assert_eq!(
        result.state,
        HttpPreviewState::Ready(HttpPreview {
            filename: "release.part01.rar".to_string(),
            total_size: Some(1234),
            content_type: Some("application/octet-stream".to_string()),
            accept_ranges: true,
        })
    );
}

#[tokio::test]
async fn http_preview_probes_direct_file_and_preserves_request_context() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("shio.db");

    let (addr, listener) = bind_ephemeral().await;
    let server = tokio::spawn(serve_probe_file(listener));

    let url = format!("http://{addr}/downloads/ignored-name.dat");
    let (engine, _progress) = DownloadEngine::new(AppConfig::default(), &db_path).unwrap();
    let tx = engine.command_sender();
    let handle = tokio::spawn(engine.run());
    let (reply, mut preview_rx) = tokio::sync::mpsc::channel::<HttpPreviewResult>(1);

    tx.send(EngineCommand::ResolveHttpPreview {
        request_id: 42,
        url: url.clone(),
        reply,
    })
    .await
    .unwrap();

    let result = tokio::time::timeout(Duration::from_secs(2), preview_rx.recv())
        .await
        .unwrap()
        .unwrap();
    shutdown_engine(&tx, handle).await;
    await_server(server).await.unwrap();

    assert_eq!(result.request_id, 42);
    assert_eq!(
        result.state,
        HttpPreviewState::Ready(HttpPreview {
            filename: "preview-file.bin".to_string(),
            total_size: Some(1234),
            content_type: Some("application/octet-stream".to_string()),
            accept_ranges: true,
        })
    );
}

#[tokio::test]
async fn http_preview_blocks_html_landing_pages_without_resolving() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("shio.db");

    let (addr, listener) = bind_ephemeral().await;
    let server = tokio::spawn(serve_html_landing_page(listener));

    let url = format!("http://{addr}/landing");
    let (engine, _progress) = DownloadEngine::new(AppConfig::default(), &db_path).unwrap();
    let tx = engine.command_sender();
    let handle = tokio::spawn(engine.run());
    let (reply, mut preview_rx) = tokio::sync::mpsc::channel::<HttpPreviewResult>(1);

    tx.send(EngineCommand::ResolveHttpPreview {
        request_id: 47,
        url,
        reply,
    })
    .await
    .unwrap();

    let result = tokio::time::timeout(Duration::from_secs(2), preview_rx.recv())
        .await
        .unwrap()
        .unwrap();
    shutdown_engine(&tx, handle).await;
    await_server(server).await.unwrap();

    assert_eq!(
        result,
        HttpPreviewResult {
            request_id: 47,
            state: HttpPreviewState::Blocked {
                reason: "not a direct file link".to_string(),
            },
        }
    );
}

#[tokio::test]
async fn http_preview_debug_output_omits_tokenized_urls() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("shio.db");

    let (addr, listener) = bind_ephemeral().await;
    let server = tokio::spawn(serve_redirect_to_tokenized_file(
        listener,
        addr,
        PREVIEW_TOKEN,
    ));

    let url = format!("http://{addr}/download");
    let (engine, _progress) = DownloadEngine::new(AppConfig::default(), &db_path).unwrap();
    let tx = engine.command_sender();
    let handle = tokio::spawn(engine.run());
    let (reply, mut preview_rx) = tokio::sync::mpsc::channel::<HttpPreviewResult>(1);

    tx.send(EngineCommand::ResolveHttpPreview {
        request_id: 43,
        url,
        reply,
    })
    .await
    .unwrap();

    let result = tokio::time::timeout(Duration::from_secs(2), preview_rx.recv())
        .await
        .unwrap()
        .unwrap();
    shutdown_engine(&tx, handle).await;
    await_server(server).await.unwrap();

    assert_eq!(result.request_id, 43);
    assert!(!format!("{result:?}").contains(PREVIEW_TOKEN));
}

#[tokio::test]
async fn shutdown_cancels_in_flight_http_preview_without_stale_result() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("shio.db");

    let (addr, listener) = bind_ephemeral().await;
    let (accepted_tx, accepted_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(serve_slow_probe_file(listener, accepted_tx, release_rx));

    let url = format!("http://{addr}/slow-file.bin");
    let (engine, _progress) = DownloadEngine::new(AppConfig::default(), &db_path).unwrap();
    let tx = engine.command_sender();
    let handle = tokio::spawn(engine.run());
    let (reply, mut preview_rx) = tokio::sync::mpsc::channel::<HttpPreviewResult>(1);

    tx.send(EngineCommand::ResolveHttpPreview {
        request_id: 44,
        url,
        reply,
    })
    .await
    .unwrap();

    tokio::time::timeout(Duration::from_secs(2), accepted_rx)
        .await
        .unwrap()
        .unwrap();
    shutdown_engine(&tx, handle).await;

    let stale = tokio::time::timeout(Duration::from_millis(500), preview_rx.recv()).await;
    if let Ok(Some(result)) = stale {
        panic!("preview result arrived after shutdown: {result:?}");
    }
    release_tx.send(()).expect("release slow server");
    match await_server(server).await {
        Ok(()) => {},
        Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => {},
        Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => {},
        Err(error) => panic!("slow server failed: {error}"),
    }
}

#[tokio::test]
async fn cancel_http_preview_stops_in_flight_request_without_stale_result() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("shio.db");

    let (addr, listener) = bind_ephemeral().await;
    let (accepted_tx, accepted_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(serve_slow_probe_file(listener, accepted_tx, release_rx));

    let url = format!("http://{addr}/slow-file.bin");
    let (engine, _progress) = DownloadEngine::new(AppConfig::default(), &db_path).unwrap();
    let tx = engine.command_sender();
    let handle = tokio::spawn(engine.run());
    let (reply, mut preview_rx) = tokio::sync::mpsc::channel::<HttpPreviewResult>(1);

    tx.send(EngineCommand::ResolveHttpPreview {
        request_id: 45,
        url,
        reply,
    })
    .await
    .unwrap();

    tokio::time::timeout(Duration::from_secs(2), accepted_rx)
        .await
        .unwrap()
        .unwrap();
    tx.send(EngineCommand::CancelHttpPreview { request_id: 45 })
        .await
        .unwrap();

    let closed = tokio::time::timeout(Duration::from_secs(2), preview_rx.recv())
        .await
        .unwrap();
    assert_eq!(closed, None);

    release_tx.send(()).expect("release slow server");
    match await_server(server).await {
        Ok(()) => {},
        Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => {},
        Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => {},
        Err(error) => panic!("slow server failed: {error}"),
    }
    shutdown_engine(&tx, handle).await;
}

#[tokio::test]
async fn cancel_before_http_preview_resolve_prevents_network_request() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("shio.db");

    let (addr, listener) = bind_ephemeral().await;
    let (accepted_tx, accepted_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(signal_if_request_arrives(listener, accepted_tx));

    let url = format!("http://{addr}/cancelled-before-start.bin");
    let (engine, _progress) = DownloadEngine::new(AppConfig::default(), &db_path).unwrap();
    let tx = engine.command_sender();
    let handle = tokio::spawn(engine.run());
    let (reply, mut preview_rx) = tokio::sync::mpsc::channel::<HttpPreviewResult>(1);

    tx.send(EngineCommand::CancelHttpPreview { request_id: 46 })
        .await
        .unwrap();
    tx.send(EngineCommand::ResolveHttpPreview {
        request_id: 46,
        url,
        reply,
    })
    .await
    .unwrap();

    let closed = tokio::time::timeout(Duration::from_secs(2), preview_rx.recv())
        .await
        .unwrap();
    assert_eq!(closed, None);

    let accepted = tokio::time::timeout(Duration::from_millis(300), accepted_rx).await;
    assert!(accepted.is_err(), "cancelled preview still reached server");

    shutdown_engine(&tx, handle).await;
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
}
