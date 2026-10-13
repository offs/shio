use shio_core::{
    AppConfig, DownloadEngine, EngineCommand, HttpPreview, HttpPreviewResult, HttpPreviewState,
};
use std::time::Duration;
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};

const FILE_RESPONSE: &str = "HTTP/1.1 206 Partial Content\r\nContent-Length: 2\r\nContent-Range: bytes 0-1/1234\r\nContent-Type: application/octet-stream\r\n";

async fn read_request(stream: &mut TcpStream) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream);
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).await?;
        if line.trim_end().is_empty() {
            return Ok(());
        }
    }
}

async fn answer(listener: &TcpListener, head: &str) -> std::io::Result<()> {
    let (mut stream, _) = listener.accept().await?;
    read_request(&mut stream).await?;
    stream.write_all(format!("{head}\r\nab").as_bytes()).await?;
    stream.shutdown().await
}

async fn listen() -> (TcpListener, String) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    (listener, format!("http://{address}"))
}

struct Engine {
    _dir: TempDir,
    commands: mpsc::Sender<EngineCommand>,
    handle: tokio::task::JoinHandle<()>,
}

impl Engine {
    fn start() -> Self {
        let dir = TempDir::new().unwrap();
        let (engine, commands, _progress) =
            DownloadEngine::new(AppConfig::default(), dir.path()).unwrap();
        Self {
            _dir: dir,
            commands,
            handle: tokio::spawn(engine.run()),
        }
    }

    async fn preview(&self, request_id: u64, url: String) -> mpsc::Receiver<HttpPreviewResult> {
        let (reply, results) = mpsc::channel(1);
        self.commands
            .send(EngineCommand::ResolveHttpPreview {
                request_id,
                url,
                reply,
            })
            .await
            .unwrap();
        results
    }

    async fn cancel(&self, request_id: u64) {
        self.commands
            .send(EngineCommand::CancelPreview { request_id })
            .await
            .unwrap();
    }

    async fn shutdown(self) {
        let (reply, ack) = oneshot::channel();
        self.commands
            .send(EngineCommand::Shutdown { reply })
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), ack)
            .await
            .unwrap()
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), self.handle)
            .await
            .unwrap()
            .unwrap();
    }
}

async fn resolve(head: String, path: &str) -> HttpPreviewState {
    let (listener, base) = listen().await;
    let server = tokio::spawn(async move { answer(&listener, &head).await });
    let engine = Engine::start();
    let mut results = engine.preview(7, format!("{base}{path}")).await;

    let result = tokio::time::timeout(Duration::from_secs(2), results.recv())
        .await
        .unwrap()
        .unwrap();
    engine.shutdown().await;
    server.await.unwrap().unwrap();
    assert_eq!(result.request_id, 7);
    result.state
}

fn ready(filename: &str) -> HttpPreviewState {
    HttpPreviewState::Ready(HttpPreview {
        filename: filename.into(),
        total_size: Some(1234),
        content_type: Some("application/octet-stream".into()),
        accept_ranges: true,
    })
}

#[tokio::test]
async fn preview_names_files_from_fragments_then_content_disposition() {
    let with_disposition =
        format!("{FILE_RESPONSE}Content-Disposition: attachment; filename=\"server.bin\"\r\n");

    assert_eq!(
        resolve(FILE_RESPONSE.into(), "/opaque-id#release.part01.rar").await,
        ready("release.part01.rar")
    );
    assert_eq!(
        resolve(with_disposition, "/downloads/ignored.dat").await,
        ready("server.bin")
    );
}

#[tokio::test]
async fn preview_blocks_html_landing_pages() {
    let state = resolve(
        "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nContent-Type: text/html\r\n".into(),
        "/landing",
    )
    .await;

    assert!(
        matches!(state, HttpPreviewState::Blocked { .. }),
        "{state:?}"
    );
}

#[tokio::test]
async fn preview_results_never_expose_redirect_tokens() {
    let (listener, base) = listen().await;
    let redirect = format!(
        "HTTP/1.1 302 Found\r\nLocation: {base}/cdn/file.bin?token=secret-preview-token\r\nContent-Length: 2\r\n"
    );
    let server = tokio::spawn(async move {
        answer(&listener, &redirect).await?;
        answer(&listener, FILE_RESPONSE).await
    });
    let engine = Engine::start();
    let mut results = engine.preview(43, format!("{base}/download")).await;

    let result = tokio::time::timeout(Duration::from_secs(2), results.recv())
        .await
        .unwrap()
        .unwrap();
    engine.shutdown().await;
    server.await.unwrap().unwrap();

    assert!(!format!("{result:?}").contains("secret-preview-token"));
}

#[tokio::test]
async fn cancel_and_shutdown_stop_in_flight_previews_without_results() {
    for shutdown in [false, true] {
        let (listener, base) = listen().await;
        let (accepted_tx, accepted) = oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            read_request(&mut stream).await.unwrap();
            accepted_tx.send(()).unwrap();
            std::future::pending::<()>().await;
        });
        let engine = Engine::start();
        let mut results = engine.preview(45, format!("{base}/slow.bin")).await;
        tokio::time::timeout(Duration::from_secs(2), accepted)
            .await
            .unwrap()
            .unwrap();

        let engine = if shutdown {
            engine.shutdown().await;
            None
        } else {
            engine.cancel(45).await;
            Some(engine)
        };

        let closed = tokio::time::timeout(Duration::from_secs(2), results.recv())
            .await
            .unwrap();
        assert_eq!(closed, None, "shutdown: {shutdown}");
        server.abort();
        if let Some(engine) = engine {
            engine.shutdown().await;
        }
    }
}

#[tokio::test]
async fn cancel_before_resolve_prevents_the_request() {
    let (listener, base) = listen().await;
    let (accepted_tx, accepted) = oneshot::channel();
    let server = tokio::spawn(async move {
        let _ = listener.accept().await;
        let _ = accepted_tx.send(());
    });
    let engine = Engine::start();

    engine.cancel(46).await;
    let mut results = engine.preview(46, format!("{base}/cancelled.bin")).await;

    let closed = tokio::time::timeout(Duration::from_secs(2), results.recv())
        .await
        .unwrap();
    assert_eq!(closed, None);
    assert!(
        tokio::time::timeout(Duration::from_millis(300), accepted)
            .await
            .is_err(),
        "cancelled preview still reached the server"
    );
    engine.shutdown().await;
    server.abort();
}
