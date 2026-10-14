use parking_lot::Mutex;
use shio_core::{
    AppConfig, Download, DownloadEngine, DownloadId, DownloadStatus, EngineCommand,
    HttpArchivePartRequest, HttpArchiveSetRequest, HttpDownloadRequest, PackageExtractState,
    ProgressEvent, ProgressStream, ShioError,
};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};

const PIECE: usize = 4 * 1024 * 1024;
const BODY_LEN: usize = PIECE + PIECE / 4;
const ETAG: &str = "\"test-version\"";

fn payload(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i % 251) as u8).collect()
}

struct Request {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: String,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    fn range(&self) -> Option<(usize, Option<usize>)> {
        let (start, end) = self
            .header("range")?
            .strip_prefix("bytes=")?
            .split_once('-')?;
        let end = if end.is_empty() {
            None
        } else {
            Some(end.parse().ok()?)
        };
        Some((start.parse().ok()?, end))
    }
}

async fn read_request(stream: &mut TcpStream) -> Option<Request> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).await.ok()?;
    let mut words = line.split_whitespace();
    let method = words.next()?.to_owned();
    let path = words.next()?.to_owned();
    let mut headers = Vec::new();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).await.ok()?;
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((key, value)) = line.split_once(':') {
            headers.push((key.trim().to_ascii_lowercase(), value.trim().to_owned()));
        }
    }
    let length = headers
        .iter()
        .find(|(key, _)| key == "content-length")
        .and_then(|(_, value)| value.parse().ok())
        .unwrap_or(0);
    let mut body = vec![0; length];
    reader.read_exact(&mut body).await.ok()?;
    Some(Request {
        method,
        path,
        headers,
        body: String::from_utf8_lossy(&body).into_owned(),
    })
}

async fn spawn_handler<F, Fut>(handle: F) -> String
where
    F: Fn(Request, TcpStream) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let handle = Arc::new(handle);
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let handle = handle.clone();
            tokio::spawn(async move {
                if let Some(request) = read_request(&mut stream).await {
                    handle(request, stream).await;
                }
            });
        }
    });
    format!("http://{address}")
}

async fn spawn_server(respond: impl Fn(&Request) -> Vec<u8> + Send + Sync + 'static) -> String {
    spawn_handler(move |request, mut stream| {
        let reply = respond(&request);
        async move {
            let _ = stream.write_all(&reply).await;
            let _ = stream.shutdown().await;
        }
    })
    .await
}

fn response(status: &str, headers: &str, body: &[u8]) -> Vec<u8> {
    let mut bytes = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n",
        body.len()
    )
    .into_bytes();
    bytes.extend_from_slice(body);
    bytes
}

fn html(body: &str) -> Vec<u8> {
    response(
        "200 OK",
        "Content-Type: text/html; charset=utf-8\r\n",
        body.as_bytes(),
    )
}

fn not_found() -> Vec<u8> {
    response("404 Not Found", "", b"")
}

fn head_len(response: &[u8]) -> usize {
    response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .expect("complete response head")
        + 4
}

fn ranged(body: &[u8], request: &Request) -> Vec<u8> {
    let range = request
        .range()
        .map(|(start, end)| (start, end.unwrap_or_else(|| body.len().saturating_sub(1))));
    match range {
        Some((start, end)) if start > end || end >= body.len() => {
            response("416 Range Not Satisfiable", "", b"")
        },
        Some((start, end)) if request.header("if-range").is_none_or(|tag| tag == ETAG) => response(
            "206 Partial Content",
            &format!(
                "ETag: {ETAG}\r\nContent-Range: bytes {start}-{end}/{}\r\n",
                body.len()
            ),
            &body[start..=end],
        ),
        _ => response("200 OK", &format!("ETag: {ETAG}\r\n"), body),
    }
}

struct Engine {
    commands: mpsc::Sender<EngineCommand>,
    progress: ProgressStream,
    handle: tokio::task::JoinHandle<()>,
}

impl Engine {
    fn start(dir: &Path) -> Self {
        let config = AppConfig {
            download_dir: dir.to_owned(),
            max_concurrent: 1,
            max_retries: 0,
            ..AppConfig::default()
        };
        let (engine, commands, progress) = DownloadEngine::new(config, dir).unwrap();
        Self {
            commands,
            progress,
            handle: tokio::spawn(engine.run()),
        }
    }

    async fn command(
        &self,
        command: impl FnOnce(oneshot::Sender<shio_core::Result<()>>) -> EngineCommand,
    ) {
        let (reply, ack) = oneshot::channel();
        self.commands.send(command(reply)).await.unwrap();
        ack.await.unwrap().unwrap();
    }

    async fn add(&self, url: String, dir: &Path, segments: u8) -> DownloadId {
        let request = HttpDownloadRequest::new(url, dir.to_owned()).with_segments(segments);
        let id = request.id();
        self.command(|reply| EngineCommand::AddHttp { request, reply })
            .await;
        id
    }

    async fn wait(
        &mut self,
        id: DownloadId,
        done: impl Fn(DownloadStatus, u64) -> bool + Sync,
    ) -> Download {
        let mut row: Option<Download> = None;
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                match self.progress.recv().await.expect("engine running") {
                    ProgressEvent::Update(update) if update.id == id => {
                        if let Some(row) = row.as_mut() {
                            row.status = update.status;
                            row.downloaded = update.downloaded;
                        }
                    },
                    ProgressEvent::Changed { downloads, .. }
                    | ProgressEvent::Resync { downloads, .. } => {
                        if let Some(update) =
                            downloads.into_iter().find(|download| download.id == id)
                        {
                            row = Some(update);
                        }
                    },
                    _ => {},
                }
                if let Some(row) = row.as_ref()
                    && done(row.status, row.downloaded)
                {
                    return row.clone();
                }
            }
        })
        .await
        .expect("download reached the expected state")
    }

    async fn finish(mut self, id: DownloadId) -> Download {
        let row = self
            .wait(id, |status, _| status.is_terminal() || status.is_failed())
            .await;
        let (reply, ack) = oneshot::channel();
        self.commands
            .send(EngineCommand::Shutdown { reply })
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), ack)
            .await
            .unwrap()
            .unwrap();
        let _ = tokio::time::timeout(Duration::from_secs(5), self.handle).await;
        row
    }
}

async fn download(url: String, segments: u8) -> (Download, TempDir) {
    let dir = TempDir::new().unwrap();
    let engine = Engine::start(dir.path());
    let id = engine.add(url, dir.path(), segments).await;
    (engine.finish(id).await, dir)
}

#[tokio::test]
async fn downloads_complete_exactly() {
    let body = payload(BODY_LEN);
    let single = spawn_server({
        let body = body.clone();
        move |_| response("200 OK", "", &body)
    })
    .await;
    let ranged_server = spawn_server({
        let body = body.clone();
        move |request| ranged(&body, request)
    })
    .await;
    let ignores_ranges = spawn_server({
        let body = body.clone();
        move |_| {
            let headers = format!("Accept-Ranges: bytes\r\nETag: {ETAG}\r\n");
            response("200 OK", &headers, &body)
        }
    })
    .await;
    let empty = spawn_server(|request| {
        if request.range() == Some((0, Some(1))) {
            response(
                "416 Range Not Satisfiable",
                "Content-Range: bytes */0\r\n",
                b"",
            )
        } else {
            response("200 OK", "", b"")
        }
    })
    .await;

    for (server, segments, expected) in [
        (single, 1, &body[..]),
        (ranged_server, 4, &body[..]),
        (ignores_ranges, 4, &body[..]),
        (empty, 4, &[][..]),
    ] {
        let (row, dir) = download(format!("{server}/file.bin"), segments).await;
        assert_eq!(row.status, DownloadStatus::Completed, "{server}");
        assert_eq!(
            std::fs::read(row.file_path()).unwrap(),
            expected,
            "{server}"
        );
        assert_eq!(row.downloaded, expected.len() as u64, "{server}");
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .filter(|name| name.to_string_lossy().starts_with(".shio-"))
            .collect();
        assert!(leftovers.is_empty(), "{server}: {leftovers:?}");
    }
}

#[tokio::test]
async fn resume_after_a_finished_piece_completes_exactly() {
    let body = payload(BODY_LEN);
    let released = Arc::new(AtomicBool::new(false));
    let second = Arc::new(tokio::sync::Semaphore::new(0));
    let ranges = Arc::new(Mutex::new(Vec::new()));
    let server = spawn_handler({
        let (body, released, second, ranges) = (
            body.clone(),
            released.clone(),
            second.clone(),
            ranges.clone(),
        );
        move |request, mut stream| {
            let (body, released, second, ranges) = (
                body.clone(),
                released.clone(),
                second.clone(),
                ranges.clone(),
            );
            async move {
                let range = request.range();
                ranges.lock().extend(range);
                let reply = ranged(&body, &request);
                let (head, released) = (head_len(&reply), released.load(Ordering::SeqCst));
                match range {
                    Some((0, None)) if !released => {
                        let _ = stream.write_all(&reply[..head]).await;
                        let _ = second.acquire().await;
                        let _ = stream.write_all(&reply[head..]).await;
                    },
                    Some((start, None)) if start > 0 && !released => {
                        let _ = stream.write_all(&reply[..head]).await;
                        second.add_permits(1);
                        std::future::pending::<()>().await;
                    },
                    _ => {
                        let _ = stream.write_all(&reply).await;
                    },
                }
                let _ = stream.shutdown().await;
            }
        }
    })
    .await;
    let dir = TempDir::new().unwrap();
    let mut engine = Engine::start(dir.path());
    let id = engine
        .add(format!("{server}/file.bin"), dir.path(), 4)
        .await;

    engine
        .wait(id, |_, downloaded| downloaded >= PIECE as u64)
        .await;
    engine
        .command(|reply| EngineCommand::Pause { id, reply })
        .await;
    engine
        .wait(id, |status, _| status == DownloadStatus::Paused)
        .await;
    released.store(true, Ordering::SeqCst);
    engine
        .command(|reply| EngineCommand::Resume { id, reply })
        .await;
    let row = engine.finish(id).await;

    assert_eq!(row.status, DownloadStatus::Completed);
    assert_eq!(std::fs::read(row.file_path()).unwrap(), body);
    let ranges = ranges.lock();
    let first_piece = ranges.iter().filter(|range| **range == (0, None)).count();
    assert_eq!(
        first_piece, 1,
        "finished piece was fetched again: {ranges:?}"
    );
}

#[tokio::test]
async fn corrupt_responses_fail_instead_of_publishing() {
    let body = payload(BODY_LEN);
    let wrong_range = spawn_server({
        let body = body.clone();
        move |request| {
            let (start, _) = request.range().unwrap_or((0, None));
            let end = body.len() - 1;
            response(
                "206 Partial Content",
                &format!(
                    "ETag: {ETAG}\r\nContent-Range: bytes 0-{}/{}\r\n",
                    end - start,
                    body.len()
                ),
                &body[start..=end],
            )
        }
    })
    .await;
    let short_body = spawn_server(|_| {
        let mut bytes = response("200 OK", "", b"0123456789");
        bytes.truncate(bytes.len() - 4);
        bytes
    })
    .await;
    let encoded =
        spawn_server(|_| response("200 OK", "Content-Encoding: gzip\r\n", b"0123456789")).await;

    for (url, segments) in [(wrong_range, 4), (short_body, 1), (encoded, 1)] {
        let (row, dir) = download(format!("{url}/file.bin"), segments).await;
        assert_eq!(row.status, DownloadStatus::Error, "{url}");
        assert!(!dir.path().join("file.bin").exists());
    }
}

/// Paced writes keep several connections open at once.
async fn trickle(stream: &mut TcpStream, bytes: &[u8]) {
    for part in bytes.chunks(256 * 1024) {
        if stream.write_all(part).await.is_err() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let _ = stream.shutdown().await;
}

#[tokio::test]
async fn a_connection_limited_server_still_completes() {
    let body = payload(4 * PIECE);
    let (open, refused) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let server = spawn_handler({
        let (body, open, refused) = (body.clone(), open.clone(), refused.clone());
        move |request, mut stream| {
            let (body, open, refused) = (body.clone(), open.clone(), refused.clone());
            async move {
                if open.fetch_add(1, Ordering::SeqCst) >= 2 {
                    refused.fetch_add(1, Ordering::SeqCst);
                    let busy = response("503 Service Unavailable", "", b"");
                    let _ = stream.write_all(&busy).await;
                } else {
                    trickle(&mut stream, &ranged(&body, &request)).await;
                }
                open.fetch_sub(1, Ordering::SeqCst);
            }
        }
    })
    .await;

    let (row, _dir) = download(format!("{server}/file.bin"), 8).await;

    assert_eq!(row.status, DownloadStatus::Completed);
    assert_eq!(std::fs::read(row.file_path()).unwrap(), body);
    assert!(
        refused.load(Ordering::SeqCst) > 0,
        "extra connections were tried"
    );
}

#[tokio::test]
async fn a_stalled_connection_does_not_hold_back_the_rest() {
    let body = payload(4 * PIECE);
    let others = Arc::new(tokio::sync::Semaphore::new(0));
    let server = spawn_handler({
        let (body, others) = (body.clone(), others.clone());
        move |request, mut stream| {
            let (body, others) = (body.clone(), others.clone());
            async move {
                let reply = ranged(&body, &request);
                if request.range() == Some((0, None)) {
                    let head = head_len(&reply);
                    let _ = stream.write_all(&reply[..head]).await;
                    let _ = others.acquire_many(2).await;
                    let _ = stream.write_all(&reply[head..head + PIECE]).await;
                    std::future::pending::<()>().await;
                }
                if request.range().is_some_and(|(start, _)| start > 0) {
                    others.add_permits(1);
                }
                let _ = stream.write_all(&reply).await;
                let _ = stream.shutdown().await;
            }
        }
    })
    .await;

    let (row, _dir) = download(format!("{server}/file.bin"), 4).await;

    assert_eq!(row.status, DownloadStatus::Completed);
    assert_eq!(std::fs::read(row.file_path()).unwrap(), body);
}

#[tokio::test]
async fn content_disposition_replaces_a_url_derived_name() {
    let server = spawn_server(|_| {
        response(
            "200 OK",
            "Content-Disposition: attachment; filename=\"real.bin\"\r\n",
            b"payload",
        )
    })
    .await;

    let (row, dir) = download(format!("{server}/get.php?id=7"), 1).await;

    assert_eq!(row.status, DownloadStatus::Completed);
    assert_eq!(row.filename, "real.bin");
    assert_eq!(
        std::fs::read(dir.path().join("real.bin")).unwrap(),
        b"payload"
    );
    assert!(!dir.path().join("get.php").exists());
}

#[tokio::test]
async fn redirects_keep_tokens_private_and_resolve_per_request() {
    let body = payload(BODY_LEN);
    let cdn_requests = Arc::new(Mutex::new(Vec::new()));
    let cdn = spawn_server({
        let (body, cdn_requests) = (body.clone(), cdn_requests.clone());
        move |request| {
            cdn_requests
                .lock()
                .push(request.header("referer").map(str::to_owned));
            ranged(&body, request)
        }
    })
    .await;
    let origin_requests = Arc::new(Mutex::new(0));
    let origin = spawn_server({
        let origin_requests = origin_requests.clone();
        move |request| {
            *origin_requests.lock() += 1;
            response(
                "302 Found",
                &format!("Location: {cdn}/signed{}\r\n", request.path),
                b"",
            )
        }
    })
    .await;

    let (row, _dir) = download(format!("{origin}/file.bin?token=secret"), 4).await;

    assert_eq!(row.status, DownloadStatus::Completed);
    assert_eq!(std::fs::read(row.file_path()).unwrap(), body);
    let cdn_requests = cdn_requests.lock();
    assert_eq!(
        *origin_requests.lock(),
        cdn_requests.len(),
        "every request starts at the origin"
    );
    assert!(cdn_requests.iter().all(Option::is_none));
}

fn htmx_page(request: &Request, file: &str) -> Option<Vec<u8>> {
    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/page") if request.header("hx-request").is_none() => Some(response(
            "403 Forbidden",
            "cf-mitigated: challenge\r\nContent-Type: text/html; charset=utf-8\r\n",
            b"<html>just a moment</html>",
        )),
        ("GET", "/page") => Some(html(r#"<a class="btn" hx-post="/go">DOWNLOAD</a>"#)),
        ("POST", "/go") if request.header("hx-request") == Some("true") => {
            Some(response("200 OK", &format!("HX-Redirect: {file}\r\n"), b""))
        },
        _ => None,
    }
}

#[tokio::test]
async fn download_pages_resolve_to_their_file() {
    let body = payload(BODY_LEN);
    let htmx = spawn_server({
        let body = body.clone();
        move |request| {
            htmx_page(request, "/file.bin").unwrap_or_else(|| match request.path.as_str() {
                "/file.bin" => ranged(&body, request),
                _ => not_found(),
            })
        }
    })
    .await;
    let xfs = spawn_server({
        let body = body.clone();
        move |request| match (request.method.as_str(), request.path.as_str()) {
            ("GET", "/page") => response(
                "200 OK",
                "Set-Cookie: xfss=1\r\nContent-Type: text/html; charset=utf-8\r\n",
                br#"<form name="F1" method="POST" action="">
                    <input type="hidden" name="op" value="download2">
                    <input type="hidden" name="method_free" value="">
                    </form><span class="seconds">1</span>"#,
            ),
            ("POST", "/page")
                if request.body.contains("op=download2")
                    && request.body.contains("method_free=Free+Download")
                    && request
                        .header("cookie")
                        .is_some_and(|c| c.contains("xfss=1")) =>
            {
                html(r#"<a href="/cdn/file.bin">file.bin</a>"#)
            },
            ("POST", "/page") => response("403 Forbidden", "", b""),
            (_, "/cdn/file.bin") => ranged(&body, request),
            _ => not_found(),
        }
    })
    .await;
    let confirm = spawn_server({
        let body = body.clone();
        move |request| match request.path.as_str() {
            "/page" => html(
                r#"<form id="download-form" method="get" action="/dl">
                <input type="hidden" name="confirm" value="t">
                <input type="submit" value="Download anyway"></form>"#,
            ),
            "/dl?confirm=t" => {
                let mut reply = ranged(&body, request);
                let head = head_len(&reply) - 2;
                let disposition = b"Content-Disposition: attachment; filename=\"file.bin\"\r\n";
                reply.splice(head..head, disposition.iter().copied());
                reply
            },
            _ => not_found(),
        }
    })
    .await;

    for server in [htmx, xfs, confirm] {
        let (row, _dir) = download(format!("{server}/page#file.bin"), 4).await;
        assert_eq!(row.status, DownloadStatus::Completed, "{server}");
        assert_eq!(row.filename, "file.bin", "{server}");
        assert_eq!(std::fs::read(row.file_path()).unwrap(), body, "{server}");
    }
}

#[tokio::test]
async fn decoy_files_are_never_downloaded() {
    let ads = spawn_server(|request| match request.path.as_str() {
        "/landing" => html(
            r#"<meta http-equiv="refresh" content="0;url=/x.rar">
            <div class="cf-turnstile" data-sitekey="k"></div>"#,
        ),
        "/x.rar" => response(
            "200 OK",
            "Content-Disposition: attachment; filename=\"x.rar\"\r\n",
            b"Rar! decoy",
        ),
        _ => not_found(),
    })
    .await;
    // `localhost` is another site than `127.0.0.1`, as an ad network would be.
    let ads = ads.replace("127.0.0.1", "localhost");
    let server = spawn_server(move |request| match request.path.as_str() {
        "/page" => html(
            r#"<p>Your file</p><a href="/setup.exe">Download</a>
            <a href="/out?u=1">Download now</a>"#,
        ),
        "/setup.exe" => response(
            "200 OK",
            "Content-Disposition: attachment; filename=\"setup.exe\"\r\n",
            b"MZ decoy",
        ),
        "/out?u=1" => response("302 Found", &format!("Location: {ads}/landing\r\n"), b""),
        _ => not_found(),
    })
    .await;

    let (row, dir) = download(format!("{server}/page#file.rar"), 1).await;

    assert_eq!(row.status, DownloadStatus::Error);
    assert_eq!(
        row.error_message,
        Some(ShioError::NoFileOnPage.short_label())
    );
    assert!(!dir.path().join("file.rar").exists());
    assert!(!dir.path().join("setup.exe").exists());
    assert!(!dir.path().join("x.rar").exists());
}

#[tokio::test]
async fn captcha_and_password_pages_fail_with_a_clear_reason() {
    let captcha = spawn_server(|_| {
        html(
            r#"<form method="post"><div class="cf-turnstile" data-sitekey="k"></div>
            <button>Download</button></form>"#,
        )
    })
    .await;
    let password = spawn_server(|_| {
        html(
            r#"<form name="F1" method="POST"><input type="hidden" name="op" value="download2">
            <input type="password" name="password"></form>
            <form action="/login"><input type="password" name="pass"></form>"#,
        )
    })
    .await;

    for (server, error) in [
        (captcha, ShioError::PageNeedsCaptcha),
        (password, ShioError::PagePasswordProtected),
    ] {
        let (row, _dir) = download(format!("{server}/page#file.bin"), 1).await;
        assert_eq!(row.status, DownloadStatus::Error, "{server}");
        assert_eq!(row.error_message, Some(error.short_label()), "{server}");
    }
}

#[tokio::test]
async fn mixed_host_part_sets_download_and_extract() {
    let mut builder = rars::Builder::new(rars::ArchiveVersion::Rar50).store(true);
    for (name, content) in [("notes.txt", &b"notes"[..]), ("payload.bin", &[7; 900][..])] {
        builder
            .add_bytes(name.as_bytes().to_vec(), content.to_vec(), None, None)
            .unwrap();
    }
    let volumes = Arc::new(builder.volume_size(Some(600)).build_volumes(None).unwrap());
    assert!((2..=9).contains(&volumes.len()), "{}", volumes.len());

    let page_host = spawn_server({
        let volumes = volumes.clone();
        move |request| {
            htmx_page(request, "/v1.rar").unwrap_or_else(|| match request.path.as_str() {
                "/v1.rar" => ranged(&volumes[0], request),
                _ => not_found(),
            })
        }
    })
    .await;
    let direct_host = spawn_server({
        let volumes = volumes.clone();
        move |request| {
            let part = request
                .path
                .strip_prefix("/p")
                .and_then(|path| path.strip_suffix(".rar"))
                .and_then(|number| number.parse::<usize>().ok());
            match part.and_then(|number| volumes.get(number - 1)) {
                Some(volume) => ranged(volume, request),
                None => not_found(),
            }
        }
    })
    .await;

    let dir = TempDir::new().unwrap();
    let mut engine = Engine::start(dir.path());
    let parts = (1..=volumes.len() as u32)
        .map(|number| {
            let name = format!("Some_Game_--_uploader.example_--_.part{number}.rar");
            let url = if number == 1 {
                format!("{page_host}/page#{name}")
            } else {
                format!("{direct_host}/p{number}.rar")
            };
            HttpArchivePartRequest::new(url, name, number, 1).unwrap()
        })
        .collect();
    let request =
        HttpArchiveSetRequest::new("Some Game".into(), dir.path().to_owned(), parts, true).unwrap();
    let package = request.id();
    engine
        .command(|reply| EngineCommand::AddHttpArchiveSet { request, reply })
        .await;

    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let (ProgressEvent::Changed { packages, .. } | ProgressEvent::Resync { packages, .. }) =
                engine.progress.recv().await.expect("engine running")
            else {
                continue;
            };
            if let Some(state) = packages
                .iter()
                .find(|candidate| candidate.id == package)
                .map(|package| package.extract_state)
            {
                assert_ne!(state, PackageExtractState::Error);
                if state == PackageExtractState::Completed {
                    return;
                }
            }
        }
    })
    .await
    .expect("the set downloaded and extracted");

    assert_eq!(
        std::fs::read(dir.path().join("Some Game/Some Game/notes.txt")).unwrap(),
        b"notes"
    );
}
