use crate::error::{Result, ShioError};
use crate::probe::{
    apply_custom_headers, http_error, is_transient, reject_encoded_body, retry_delay,
};
use crate::types::{ChunkInfo, ChunkStatus};
use crate::worker::blocking;
use futures::StreamExt as _;
use parking_lot::Mutex;
use reqwest::StatusCode;
use reqwest::header::{self, HeaderMap, HeaderValue};
use std::collections::HashMap;
use std::fs::File;
use std::num::NonZeroU64;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore, mpsc};
use tokio_util::sync::CancellationToken;

pub const MAX_SEGMENTS: u8 = 16;
const HOST_CONNECTIONS: usize = 16;
const MIN_PIECE_BYTES: u64 = 4 * 1024 * 1024;
const MAX_PIECES: u64 = 1024;
const WRITE_BUFFER_BYTES: usize = 1024 * 1024;
const MAX_BUFFER_AGE: Duration = Duration::from_millis(250);
const LIMITER_BURST: Duration = Duration::from_millis(100);

/// An unknown size is one open-ended stream (`end` unused); an empty body needs no piece.
pub(crate) fn plan_pieces(total: Option<u64>, ranged: bool) -> Vec<ChunkInfo> {
    let Some(total) = total else {
        return vec![pending(0, 0, 0)];
    };
    let size = if ranged {
        MIN_PIECE_BYTES.max(total.div_ceil(MAX_PIECES))
    } else {
        total.max(1)
    };
    (0..total.div_ceil(size))
        .zip(0_u32..)
        .map(|(i, index)| {
            let start = i * size;
            pending(index, start, (start + size).min(total) - 1)
        })
        .collect()
}

const fn pending(index: u32, start: u64, end: u64) -> ChunkInfo {
    ChunkInfo {
        index,
        start,
        end,
        downloaded: 0,
        status: ChunkStatus::Pending,
    }
}

/// Per host across all downloads, so the parts of a package stay within a per-client limit.
#[derive(Default)]
pub(crate) struct HostLimits(Mutex<HashMap<String, Arc<Semaphore>>>);

impl HostLimits {
    pub(crate) fn get(&self, url: &str) -> Arc<Semaphore> {
        let host = url::Url::parse(url)
            .ok()
            .and_then(|url| url.host_str().map(str::to_owned))
            .unwrap_or_default();
        let mut hosts = self.0.lock();
        hosts.retain(|_, semaphore| Arc::strong_count(semaphore) > 1);
        hosts
            .entry(host)
            .or_insert_with(|| Arc::new(Semaphore::new(HOST_CONNECTIONS)))
            .clone()
    }
}

#[derive(Clone)]
pub(crate) struct BandwidthLimiter {
    state: Arc<Mutex<BandwidthState>>,
    changed: Arc<Notify>,
}

struct BandwidthState {
    bytes_per_second: Option<NonZeroU64>,
    next_available: Instant,
}

impl BandwidthLimiter {
    pub(crate) fn new(bytes_per_second: Option<u64>) -> Self {
        Self {
            state: Arc::new(Mutex::new(BandwidthState {
                bytes_per_second: bytes_per_second.and_then(NonZeroU64::new),
                next_available: Instant::now(),
            })),
            changed: Arc::new(Notify::new()),
        }
    }

    pub(crate) fn set_limit(&self, bytes_per_second: Option<u64>) {
        let mut state = self.state.lock();
        state.bytes_per_second = bytes_per_second.and_then(NonZeroU64::new);
        state.next_available = Instant::now();
        drop(state);
        self.changed.notify_waiters();
    }

    async fn acquire(&self, bytes: u64, cancel: &CancellationToken) -> Result<()> {
        loop {
            let changed = self.changed.notified();
            let mut changed = std::pin::pin!(changed);
            changed.as_mut().enable();
            let wait = {
                let mut state = self.state.lock();
                let Some(limit) = state.bytes_per_second else {
                    return Ok(());
                };
                let now = Instant::now();
                if state.next_available <= now {
                    let nanos = u128::from(bytes) * 1_000_000_000 / u128::from(limit.get());
                    let cost = Duration::from_nanos(u64::try_from(nanos).unwrap_or(u64::MAX));
                    // Schedule from the previous deadline, not from `now`, so timer and wake-up
                    // latency do not accumulate into a rate below the limit.
                    let floor = now.checked_sub(LIMITER_BURST).unwrap_or(now);
                    state.next_available = state.next_available.max(floor) + cost;
                    return Ok(());
                }
                state.next_available - now
            };
            tokio::select! {
                () = cancel.cancelled() => return Err(ShioError::Cancelled),
                () = tokio::time::sleep(wait) => {},
                () = changed => {},
            }
        }
    }
}

struct Piece {
    start: u64,
    end: u64,
    /// `None` until an open-ended stream of unknown size ends.
    len: Option<u64>,
    downloaded: u64,
    persisted: u64,
    busy: bool,
}

impl Piece {
    fn is_complete(&self) -> bool {
        self.len == Some(self.downloaded)
    }

    fn is_free(&self) -> bool {
        !self.busy && !self.is_complete()
    }

    fn remaining(&self) -> u64 {
        self.len.map_or(u64::MAX, |len| len - self.downloaded)
    }
}

/// The pieces of one download. A piece has at most one writer (`busy`), and a writer only
/// grows its own piece, so no two connections ever write the same byte.
pub(crate) struct Pieces(Mutex<Vec<Piece>>);

impl Pieces {
    pub(crate) fn new(chunks: Vec<ChunkInfo>, total: Option<u64>) -> Self {
        Self(Mutex::new(
            chunks
                .into_iter()
                .map(|chunk| Piece {
                    start: chunk.start,
                    end: chunk.end,
                    len: total.map(|_| chunk.end - chunk.start + 1),
                    downloaded: chunk.downloaded,
                    persisted: chunk.downloaded,
                    busy: false,
                })
                .collect(),
        ))
    }

    pub(crate) fn downloaded(&self) -> u64 {
        self.0.lock().iter().map(|piece| piece.downloaded).sum()
    }

    pub(crate) fn is_complete(&self) -> bool {
        self.0.lock().iter().all(Piece::is_complete)
    }

    /// The start of the largest free run, or its middle when the writer before the run is
    /// already streaming into it.
    pub(crate) fn claim(&self) -> Option<usize> {
        let mut pieces = self.0.lock();
        let mut largest: Option<(u64, usize, usize)> = None;
        let mut index = 0;
        while index < pieces.len() {
            let first = index;
            let mut remaining = 0_u64;
            while index < pieces.len() && pieces[index].is_free() {
                remaining = remaining.saturating_add(pieces[index].remaining());
                index += 1;
            }
            if index > first && largest.is_none_or(|(most, ..)| remaining > most) {
                largest = Some((remaining, first, index));
            }
            index += 1;
        }
        let (_, first, end) = largest?;
        let piece = if first > 0 && pieces[first - 1].busy {
            first + (end - first) / 2
        } else {
            first
        };
        pieces[piece].busy = true;
        Some(piece)
    }

    fn advance(&self, piece: usize) -> Option<usize> {
        let mut pieces = self.0.lock();
        let next = pieces
            .get_mut(piece + 1)
            .filter(|next| next.is_free() && next.downloaded == 0)?;
        next.busy = true;
        pieces[piece].busy = false;
        Some(piece + 1)
    }

    fn release(&self, piece: usize) {
        self.0.lock()[piece].busy = false;
    }

    fn record(&self, piece: usize, downloaded: u64) {
        self.0.lock()[piece].downloaded = downloaded;
    }

    fn finish_stream(&self, piece: usize) {
        let mut pieces = self.0.lock();
        pieces[piece].len = Some(pieces[piece].downloaded);
    }

    fn has_other_writers(&self) -> bool {
        self.0.lock().iter().filter(|piece| piece.busy).count() > 1
    }

    fn is_last(&self, piece: usize) -> bool {
        piece + 1 == self.0.lock().len()
    }

    pub(crate) fn unsaved(&self) -> Vec<ChunkInfo> {
        self.0
            .lock()
            .iter()
            .zip(0_u32..)
            .filter(|(piece, _)| piece.downloaded != piece.persisted)
            .map(|(piece, index)| ChunkInfo {
                index,
                start: piece.start,
                end: piece.end,
                downloaded: piece.downloaded,
                status: if piece.is_complete() {
                    ChunkStatus::Completed
                } else if piece.downloaded > 0 {
                    ChunkStatus::Downloading
                } else {
                    ChunkStatus::Pending
                },
            })
            .collect()
    }

    pub(crate) fn saved(&self, rows: &[ChunkInfo]) {
        let mut pieces = self.0.lock();
        for row in rows {
            pieces[row.index as usize].persisted = row.downloaded;
        }
    }
}

pub(crate) struct Transfer {
    pub(crate) client: reqwest::Client,
    pub(crate) url: String,
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) file: Arc<File>,
    pub(crate) if_range: Option<HeaderValue>,
    pub(crate) total: Option<u64>,
    pub(crate) require_range: bool,
    pub(crate) pieces: Pieces,
    pub(crate) limiter: BandwidthLimiter,
    pub(crate) cancel: CancellationToken,
    pub(crate) max_retries: u32,
    /// Signalled on every 206, so connections ramp up one at a time.
    pub(crate) ranged: mpsc::UnboundedSender<()>,
}

pub(crate) enum Exit {
    Done,
    Refused,
}

/// A connection that fails before receiving anything while others are writing was refused.
///
/// It gives its piece back instead of retrying, so a per-client limit costs speed, not the
/// download.
pub(crate) async fn connect(
    transfer: Arc<Transfer>,
    piece: usize,
    _permit: OwnedSemaphorePermit,
) -> Result<Exit> {
    let mut connection = Connection::new(&transfer, piece);
    let mut retries = 0_u32;
    let exit = loop {
        connection.progressed = false;
        match connection.fetch().await {
            Ok(()) => {
                retries = 0;
                transfer.pieces.release(connection.piece);
                match transfer.pieces.claim() {
                    Some(next) => connection.load(next),
                    None => return Ok(Exit::Done),
                }
            },
            Err(error) => {
                retries = if connection.progressed {
                    1
                } else {
                    retries + 1
                };
                if !connection.progressed
                    && is_transient(&error)
                    && transfer.pieces.has_other_writers()
                {
                    tracing::info!("extra connection refused: {}", error.safe_diagnostic());
                    break Ok(Exit::Refused);
                }
                let Some(delay) = retry_delay(&error, retries, transfer.max_retries) else {
                    break Err(error);
                };
                tracing::warn!(
                    "piece {} retry {retries}/{} in {delay:?}: {}",
                    connection.piece,
                    transfer.max_retries,
                    error.safe_diagnostic()
                );
                tokio::select! {
                    () = transfer.cancel.cancelled() => break Err(ShioError::Cancelled),
                    () = tokio::time::sleep(delay) => {},
                }
            },
        }
    };
    transfer.pieces.release(connection.piece);
    exit
}

struct Connection<'a> {
    transfer: &'a Transfer,
    piece: usize,
    start: u64,
    len: Option<u64>,
    downloaded: u64,
    progressed: bool,
}

impl<'a> Connection<'a> {
    fn new(transfer: &'a Transfer, piece: usize) -> Self {
        let mut connection = Self {
            transfer,
            piece,
            start: 0,
            len: None,
            downloaded: 0,
            progressed: false,
        };
        connection.load(piece);
        connection
    }

    fn load(&mut self, piece: usize) {
        let pieces = self.transfer.pieces.0.lock();
        let Piece {
            start,
            len,
            downloaded,
            ..
        } = pieces[piece];
        (self.piece, self.start, self.len, self.downloaded) = (piece, start, len, downloaded);
    }

    fn room(&self, buffered: usize) -> u64 {
        self.len
            .map_or(u64::MAX, |len| len - self.downloaded - buffered as u64)
    }

    async fn fetch(&mut self) -> Result<()> {
        let transfer = self.transfer;
        if self.downloaded > 0 && transfer.if_range.is_none() {
            self.restart().await?;
        }
        let resume = transfer.require_range || self.downloaded > 0;
        let offset = self.start + self.downloaded;
        let mut request = transfer.client.get(&transfer.url);
        if resume {
            request = request.header(header::RANGE, format!("bytes={offset}-"));
            if let Some(validator) = &transfer.if_range {
                request = request.header(header::IF_RANGE, validator.clone());
            }
        }
        let request = apply_custom_headers(request, &transfer.headers)?;
        let response = tokio::select! {
            () = transfer.cancel.cancelled() => return Err(ShioError::Cancelled),
            response = request.send() => response?,
        };
        if !response.status().is_success() {
            return Err(http_error(&response));
        }
        reject_encoded_body(response.headers())?;
        if response.status() == StatusCode::PARTIAL_CONTENT {
            if !resume {
                return Err(ShioError::Other("unrequested partial content".into()));
            }
            validate_content_range(response.headers(), offset, transfer.total)?;
            let _ = transfer.ranged.send(());
        } else {
            if let (Some(total), Some(length)) = (transfer.total, response.content_length())
                && total != length
            {
                return Err(ShioError::ResourceChanged);
            }
            if transfer.require_range {
                return Err(ShioError::ResourceChanged);
            }
            if resume {
                self.restart().await?;
            }
        }

        let mut stream = response.bytes_stream();
        let mut buffer = Vec::with_capacity(WRITE_BUFFER_BYTES);
        let mut buffered_since = Instant::now();
        let received = 'stream: loop {
            let next = tokio::select! {
                () = transfer.cancel.cancelled() => break Err(ShioError::Cancelled),
                next = stream.next() => next,
            };
            let bytes = match next {
                None => break Ok(()),
                Some(Ok(bytes)) => bytes,
                Some(Err(error)) => break Err(error.into()),
            };
            if let Err(error) = transfer
                .limiter
                .acquire(bytes.len() as u64, &transfer.cancel)
                .await
            {
                break Err(error);
            }
            let mut rest = &bytes[..];
            while !rest.is_empty() {
                let room = self.room(buffer.len());
                if room == 0 {
                    break 'stream Err(ShioError::SizeMismatch {
                        expected: self.downloaded,
                        actual: self.downloaded + rest.len() as u64,
                    });
                }
                let (now, later) = rest.split_at(
                    usize::try_from(room).map_or(rest.len(), |room| room.min(rest.len())),
                );
                if buffer.is_empty() {
                    buffered_since = Instant::now();
                }
                buffer.extend_from_slice(now);
                rest = later;
                if self.room(buffer.len()) == 0 {
                    buffer = self.write(buffer).await?;
                    if !transfer.pieces.is_last(self.piece) {
                        match transfer.pieces.advance(self.piece) {
                            Some(next) => self.load(next),
                            None => break 'stream Ok(()),
                        }
                    }
                }
            }
            if buffer.len() >= WRITE_BUFFER_BYTES || buffered_since.elapsed() >= MAX_BUFFER_AGE {
                buffer = self.write(buffer).await?;
            }
        };
        // Bytes that arrived before a cancel or stream error are valid; keep them so a resume
        // continues exactly where the file ends.
        let written = self.write(buffer).await;
        received?;
        written?;
        match self.len {
            None => {
                transfer.pieces.finish_stream(self.piece);
                Ok(())
            },
            Some(len) if self.downloaded != len => Err(ShioError::SizeMismatch {
                expected: len,
                actual: self.downloaded,
            }),
            Some(_) => Ok(()),
        }
    }

    /// Counts bytes only once they are written, so a failed write never leaves a counted hole.
    async fn write(&mut self, buffer: Vec<u8>) -> Result<Vec<u8>> {
        if buffer.is_empty() {
            return Ok(buffer);
        }
        let file = self.transfer.file.clone();
        let offset = self.start + self.downloaded;
        let mut buffer =
            blocking(move || Ok(write_all_at(&file, &buffer, offset).map(|()| buffer)?)).await?;
        self.downloaded += buffer.len() as u64;
        self.progressed = true;
        self.transfer.pieces.record(self.piece, self.downloaded);
        buffer.clear();
        Ok(buffer)
    }

    async fn restart(&mut self) -> Result<()> {
        if self.transfer.total.is_none() {
            let file = self.transfer.file.clone();
            blocking(move || Ok(file.set_len(0)?)).await?;
        }
        self.downloaded = 0;
        self.transfer.pieces.record(self.piece, 0);
        Ok(())
    }
}

fn write_all_at(file: &File, buffer: &[u8], offset: u64) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::FileExt::write_all_at(file, buffer, offset)
    }
    #[cfg(windows)]
    {
        let (mut buffer, mut offset) = (buffer, offset);
        while !buffer.is_empty() {
            match std::os::windows::fs::FileExt::seek_write(file, buffer, offset)? {
                0 => return Err(std::io::ErrorKind::WriteZero.into()),
                written => {
                    buffer = &buffer[written..];
                    offset += written as u64;
                },
            }
        }
        Ok(())
    }
}

fn validate_content_range(headers: &HeaderMap, start: u64, total: Option<u64>) -> Result<()> {
    let invalid = || ShioError::Other("invalid content-range".into());
    let value = headers
        .get(header::CONTENT_RANGE)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(invalid)?;
    let (range, size) = value
        .strip_prefix("bytes ")
        .and_then(|value| value.split_once('/'))
        .ok_or_else(invalid)?;
    if total.is_some_and(|total| size.parse() != Ok(total)) {
        return Err(ShioError::ResourceChanged);
    }
    let (first, last) = range
        .split_once('-')
        .and_then(|(first, last)| Some((first.parse::<u64>().ok()?, last.parse::<u64>().ok()?)))
        .ok_or_else(invalid)?;
    if last < first
        || first != start
        || total.is_some_and(|total| total.checked_sub(1) != Some(last))
    {
        return Err(ShioError::Other("unexpected content-range".into()));
    }
    let range_len = (last - first).saturating_add(1);
    match headers
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok()?.parse::<u64>().ok())
    {
        Some(length) if length != range_len => Err(ShioError::SizeMismatch {
            expected: range_len,
            actual: length,
        }),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::probe::tests::one_shot_server;

    fn transfer(url: String, file: &std::path::Path, chunk: (u64, u64, u64)) -> Transfer {
        let (start, end, downloaded) = chunk;
        Transfer {
            client: reqwest::Client::new(),
            url,
            headers: Vec::new(),
            file: Arc::new(
                std::fs::OpenOptions::new()
                    .create(true)
                    .truncate(false)
                    .write(true)
                    .open(file)
                    .expect("open payload"),
            ),
            if_range: Some(HeaderValue::from_static("\"version-1\"")),
            total: None,
            require_range: false,
            pieces: Pieces::new(
                vec![ChunkInfo {
                    downloaded,
                    ..pending(0, start, end)
                }],
                None,
            ),
            limiter: BandwidthLimiter::new(None),
            cancel: CancellationToken::new(),
            max_retries: 3,
            ranged: mpsc::unbounded_channel().0,
        }
    }

    fn permit() -> OwnedSemaphorePermit {
        Arc::new(Semaphore::new(1))
            .try_acquire_owned()
            .expect("free permit")
    }

    #[test]
    fn pieces_tile_the_body_exactly() {
        const MIB: u64 = 1024 * 1024;
        let ranges = |total, ranged| {
            plan_pieces(total, ranged)
                .iter()
                .map(|piece| (piece.start, piece.end))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            ranges(Some(10 * MIB), true),
            [
                (0, 4 * MIB - 1),
                (4 * MIB, 8 * MIB - 1),
                (8 * MIB, 10 * MIB - 1)
            ]
        );
        assert_eq!(ranges(Some(10 * MIB), false), [(0, 10 * MIB - 1)]);
        assert_eq!(ranges(Some(1), true), [(0, 0)]);
        assert_eq!(ranges(Some(0), true), []);
        assert_eq!(ranges(None, true), [(0, 0)]);

        let huge = plan_pieces(Some(8 * 1024 * MIB + 1), true);
        assert_eq!(huge.len(), 1024);
        assert!(huge.windows(2).all(|pair| pair[0].end + 1 == pair[1].start));
        assert_eq!(huge.last().map(|piece| piece.end), Some(8 * 1024 * MIB));
    }

    #[test]
    fn connections_split_the_largest_free_run_and_never_share_a_piece() {
        let pieces = Pieces::new(
            (0..8)
                .map(|index| pending(index, u64::from(index) * 10, u64::from(index) * 10 + 9))
                .collect(),
            Some(80),
        );
        let claims: Vec<_> = std::iter::from_fn(|| pieces.claim()).collect();
        assert_eq!(claims, [0, 4, 2, 6, 1, 3, 5, 7]);

        pieces.release(3);
        pieces.record(3, 1);
        assert_eq!(
            pieces.advance(2),
            None,
            "a started piece is resumed by a new request"
        );
        pieces.record(1, 10);
        assert_eq!(pieces.advance(1), None, "a busy piece is never taken over");
        assert_eq!(pieces.claim(), Some(3));
    }

    #[test]
    fn content_range_must_match_the_request_and_resource() {
        for (range, length, total, valid) in [
            ("bytes 0-9/10", "10", Some(10), true),
            ("bytes 0-9/*", "10", None, true),
            ("bytes 5-9/10", "5", Some(10), false),
            ("bytes 0-8/10", "9", Some(10), false),
            ("bytes 0-9/10", "4", Some(10), false),
            ("bytes 0-18446744073709551615/*", "1", None, false),
            ("bytes 0-9", "10", None, false),
        ] {
            let mut headers = HeaderMap::new();
            headers.insert(header::CONTENT_RANGE, HeaderValue::from_static(range));
            headers.insert(header::CONTENT_LENGTH, HeaderValue::from_static(length));
            assert_eq!(
                validate_content_range(&headers, 0, total).is_ok(),
                valid,
                "{range}"
            );
        }
        let mut changed = HeaderMap::new();
        changed.insert(
            header::CONTENT_RANGE,
            HeaderValue::from_static("bytes 0-9/20"),
        );
        assert!(matches!(
            validate_content_range(&changed, 0, Some(10)),
            Err(ShioError::ResourceChanged)
        ));
    }

    #[tokio::test]
    async fn ignored_resume_range_restarts_an_open_ended_stream() {
        let response = b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\nnew";
        let (url, request) = one_shot_server(response.to_vec(), false).await;
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("payload.part");
        std::fs::write(&path, b"old stale tail").expect("seed partial");
        let transfer = transfer(url, &path, (0, 0, 3));
        let mut connection = Connection::new(&transfer, 0);

        connection.fetch().await.expect("download succeeds");

        let request = request
            .await
            .expect("captured request")
            .to_ascii_lowercase();
        assert!(request.contains("range: bytes=3-\r\n"));
        assert!(request.contains("if-range: \"version-1\""));
        assert_eq!(std::fs::read(path).expect("payload"), b"new");
    }

    #[tokio::test]
    async fn cancel_keeps_received_bytes_written_and_counted() {
        let response = b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\nhello";
        let (url, _request) = one_shot_server(response.to_vec(), true).await;
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("payload.part");
        let transfer = Arc::new(transfer(url, &path, (0, 0, 0)));
        let task = tokio::spawn(connect(transfer.clone(), 0, permit()));
        tokio::time::sleep(Duration::from_millis(100)).await;
        transfer.cancel.cancel();

        let result = tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .expect("cancellation is prompt")
            .expect("task joins");
        assert!(matches!(result, Err(ShioError::Cancelled)));
        assert_eq!(transfer.pieces.downloaded(), 5);
        assert_eq!(std::fs::read(path).expect("payload"), b"hello");
    }

    #[tokio::test]
    async fn permanent_http_errors_are_not_retried() {
        let response = b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n";
        let (url, _request) = one_shot_server(response.to_vec(), false).await;
        let temp = tempfile::tempdir().expect("tempdir");
        let transfer = transfer(url, &temp.path().join("payload.part"), (0, 0, 0));
        let started = Instant::now();

        let result = connect(Arc::new(transfer), 0, permit()).await;

        assert!(matches!(result, Err(ShioError::Http { code: 404, .. })));
        assert!(started.elapsed() < Duration::from_millis(500));
    }

    #[tokio::test]
    async fn limiter_paces_reservations_and_ignores_cancelled_waiters() {
        let limiter = BandwidthLimiter::new(Some(1_000));
        let (active, cancelled) = (CancellationToken::new(), CancellationToken::new());
        cancelled.cancel();
        limiter.acquire(100, &active).await.expect("first permit");
        assert!(matches!(
            limiter.acquire(100, &cancelled).await,
            Err(ShioError::Cancelled)
        ));
        let started = Instant::now();

        limiter.acquire(100, &active).await.expect("second permit");

        let waited = started.elapsed();
        assert!(
            (Duration::from_millis(80)..Duration::from_millis(180)).contains(&waited),
            "{waited:?}"
        );
    }

    #[tokio::test]
    async fn raising_the_limit_wakes_waiters() {
        let limiter = BandwidthLimiter::new(Some(1));
        let cancel = CancellationToken::new();
        limiter.acquire(1, &cancel).await.expect("first permit");
        let waiter = {
            let limiter = limiter.clone();
            let cancel = cancel.clone();
            tokio::spawn(async move { limiter.acquire(1, &cancel).await })
        };
        tokio::task::yield_now().await;

        limiter.set_limit(None);

        tokio::time::timeout(Duration::from_millis(500), waiter)
            .await
            .expect("waiter woke")
            .expect("task joins")
            .expect("permit");
    }
}
