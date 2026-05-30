use crate::error::{Result, ShioError};
use crate::types::{ChunkInfo, ChunkStatus};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio::io::{AsyncSeekExt, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone)]
pub(crate) struct ChunkProgress {
    pub(crate) chunk_index: u32,
    pub(crate) downloaded: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct ChunkPlan {
    pub(crate) chunks: Vec<ChunkInfo>,
}

impl ChunkPlan {
    pub(crate) fn plan_chunks(total_size: u64, num_segments: u8) -> Self {
        let num = u64::from(num_segments.max(1));
        let chunk_size = total_size / num;
        let mut chunks = Vec::with_capacity(num as usize);

        for i in 0..num {
            let start = i * chunk_size;
            let end = if i == num - 1 {
                total_size - 1
            } else {
                (i + 1) * chunk_size - 1
            };
            chunks.push(ChunkInfo {
                index: i as u32,
                start,
                end,
                downloaded: 0,
                status: ChunkStatus::Pending,
            });
        }

        Self { chunks }
    }

    pub(crate) fn single_stream() -> Self {
        Self {
            chunks: vec![ChunkInfo {
                index: 0,
                start: 0,
                end: 0,
                downloaded: 0,
                status: ChunkStatus::Pending,
            }],
        }
    }
}

#[derive(Debug)]
pub(crate) struct ChunkDownloader {
    pub(crate) client: reqwest::Client,
    pub(crate) url: String,
    pub(crate) chunk: ChunkInfo,
    pub(crate) file_path: PathBuf,
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) cancel: CancellationToken,
    pub(crate) speed_limit: Option<u64>,
    pub(crate) require_range: bool,
}

impl ChunkDownloader {
    #[allow(clippy::cognitive_complexity)]
    pub(crate) async fn download(
        &mut self,
        progress_tx: mpsc::Sender<ChunkProgress>,
    ) -> Result<()> {
        use futures::StreamExt;

        let mut request = self.client.get(&self.url);

        let should_request_range =
            self.chunk.end > 0 && (self.require_range || self.chunk.downloaded > 0);
        if should_request_range {
            let range_start = self.chunk.start + self.chunk.downloaded;
            let range_end = self.chunk.end;
            request = request.header(
                reqwest::header::RANGE,
                format!("bytes={range_start}-{range_end}"),
            );
        } else if self.chunk.downloaded > 0 {
            request = request.header(
                reqwest::header::RANGE,
                format!("bytes={}-", self.chunk.downloaded),
            );
        }

        request = crate::probe::apply_custom_headers(request, &self.headers)?;

        let requested_resume = self.chunk.downloaded > 0 || should_request_range;
        let response = request.send().await?;
        let status = response.status();

        if !status.is_success() {
            return Err(ShioError::Http {
                code: status.as_u16(),
                message: status.canonical_reason().unwrap_or("unknown").to_string(),
            });
        }

        let server_honored_range = status == reqwest::StatusCode::PARTIAL_CONTENT;
        if should_request_range && self.require_range {
            if !server_honored_range {
                return Err(ShioError::NotResumable);
            }
            validate_content_range(response.headers(), self.chunk.start, self.chunk.end)?;
        }
        if requested_resume && !server_honored_range && !self.require_range {
            self.chunk.downloaded = 0;
        }

        let mut stream = response.bytes_stream();

        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&self.file_path)
            .await?;

        let write_offset = self.chunk.start + self.chunk.downloaded;
        file.seek(std::io::SeekFrom::Start(write_offset)).await?;

        let mut last_report = Instant::now();

        while let Some(chunk_result) = stream.next().await {
            if self.cancel.is_cancelled() {
                tracing::debug!("Chunk {} cancelled", self.chunk.index);
                return Err(ShioError::Cancelled);
            }

            let bytes = chunk_result?;
            file.write_all(&bytes).await?;

            let len = bytes.len() as u64;
            self.chunk.downloaded += len;

            if let Some(expected_duration_ms) = self
                .speed_limit
                .and_then(|limit| len.saturating_mul(1000).checked_div(limit))
                .map(|duration| duration.max(1))
            {
                let actual_ms = last_report.elapsed().as_millis() as u64;
                if actual_ms < expected_duration_ms {
                    tokio::time::sleep(Duration::from_millis(expected_duration_ms - actual_ms))
                        .await;
                }
            }

            if last_report.elapsed().as_millis() >= 100 {
                let _ = progress_tx
                    .send(ChunkProgress {
                        chunk_index: self.chunk.index,
                        downloaded: self.chunk.downloaded,
                    })
                    .await;
                last_report = Instant::now();
            }
        }

        let _ = progress_tx
            .send(ChunkProgress {
                chunk_index: self.chunk.index,
                downloaded: self.chunk.downloaded,
            })
            .await;

        file.flush().await?;

        tracing::debug!(
            "Chunk {} completed: {} bytes",
            self.chunk.index,
            self.chunk.downloaded
        );
        Ok(())
    }
}

fn validate_content_range(
    headers: &reqwest::header::HeaderMap,
    expected_start: u64,
    expected_end: u64,
) -> Result<()> {
    let value = headers
        .get(reqwest::header::CONTENT_RANGE)
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| ShioError::Other("missing content-range".into()))?;
    let range = value
        .strip_prefix("bytes ")
        .and_then(|v| v.split_once('/').map(|(range, _)| range))
        .and_then(|range| range.split_once('-'))
        .and_then(|(start, end)| Some((start.parse::<u64>().ok()?, end.parse::<u64>().ok()?)))
        .ok_or_else(|| ShioError::Other("invalid content-range".into()))?;
    if range != (expected_start, expected_end) {
        return Err(ShioError::SizeMismatch {
            expected: expected_end - expected_start + 1,
            actual: range.1.saturating_sub(range.0).saturating_add(1),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_covers_requested_range() {
        let plan = ChunkPlan::plan_chunks(1000, 3);

        let ranges: Vec<_> = plan
            .chunks
            .iter()
            .map(|chunk| {
                (
                    chunk.index,
                    chunk.start,
                    chunk.end,
                    chunk.downloaded,
                    chunk.status,
                )
            })
            .collect();
        assert_eq!(
            ranges,
            vec![
                (0, 0, 332, 0, ChunkStatus::Pending),
                (1, 333, 665, 0, ChunkStatus::Pending),
                (2, 666, 999, 0, ChunkStatus::Pending),
            ]
        );
    }

    #[test]
    fn single_stream_is_one_unbounded_chunk() {
        let plan = ChunkPlan::single_stream();
        assert_eq!(plan.chunks.len(), 1);
        assert_eq!(plan.chunks[0].start, 0);
        assert_eq!(plan.chunks[0].end, 0);
        assert_eq!(plan.chunks[0].status, ChunkStatus::Pending);
    }

    #[tokio::test]
    async fn invalid_custom_header_returns_input_error_before_request() {
        let mut downloader = ChunkDownloader {
            client: reqwest::Client::new(),
            url: "http://127.0.0.1:1/file.bin".to_string(),
            chunk: ChunkInfo {
                index: 0,
                start: 0,
                end: 0,
                downloaded: 0,
                status: ChunkStatus::Pending,
            },
            file_path: PathBuf::from("file.bin"),
            headers: vec![("bad header".to_string(), "value".to_string())],
            cancel: CancellationToken::new(),
            speed_limit: None,
            require_range: false,
        };
        let (tx, _rx) = mpsc::channel(1);

        let result = downloader.download(tx).await;

        assert!(
            matches!(result, Err(ShioError::Config(message)) if message.starts_with("invalid header "))
        );
    }
}
