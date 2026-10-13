use std::time::Duration;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::{DownloadEngine, Internal};
use crate::error::ShioError;
use crate::types::{HttpPreview, HttpPreviewResult, HttpPreviewState, MagnetPreviewResult};

const HTTP_PREVIEW_TIMEOUT: Duration = Duration::from_secs(30);
const MAGNET_PREVIEW_TIMEOUT: Duration = Duration::from_mins(2);
// The app cancels finished previews too, so remembered early cancels are bounded.
const EARLY_CANCEL_MEMORY: usize = 32;

impl DownloadEngine {
    pub(super) fn resolve_http_preview(
        &mut self,
        request_id: u64,
        url: String,
        reply: mpsc::Sender<HttpPreviewResult>,
    ) {
        let Some(cancel) = self.start_preview(request_id) else {
            return;
        };
        let client = self.http.clone();
        let loop_tx = self.ctx.loop_tx.clone();
        self.tasks.spawn(async move {
            let probe = tokio::time::timeout(
                HTTP_PREVIEW_TIMEOUT,
                crate::probe::probe(&client, &url, &[]),
            );
            let state = tokio::select! {
                () = cancel.cancelled() => None,
                probe = probe => Some(match probe {
                    Ok(Ok(probe)) => HttpPreviewState::Ready(HttpPreview {
                        filename: fragment_filename(&url)
                            .or_else(|| probe.filename().map(str::to_owned))
                            .unwrap_or_else(|| crate::filename::extract_filename(&url)),
                        total_size: probe.content_length,
                        content_type: probe.content_type,
                        accept_ranges: probe.accept_ranges,
                    }),
                    Ok(Err(ShioError::NotADirectFile { .. })) => HttpPreviewState::Blocked {
                        reason: "not a direct file link".into(),
                    },
                    Ok(Err(error)) => HttpPreviewState::Error {
                        message: error.short_label(),
                    },
                    Err(_) => HttpPreviewState::Error {
                        message: "preview timed out".into(),
                    },
                }),
            };
            if let Some(state) = state {
                let _ = reply.send(HttpPreviewResult { request_id, state }).await;
            }
            let _ = loop_tx.send(Internal::PreviewDone(request_id));
        });
    }

    pub(super) fn resolve_magnet_preview(
        &mut self,
        request_id: u64,
        magnet: String,
        reply: mpsc::Sender<MagnetPreviewResult>,
    ) {
        let Some(cancel) = self.start_preview(request_id) else {
            return;
        };
        let shared = self.ctx.shared.clone();
        let loop_tx = self.ctx.loop_tx.clone();
        self.tasks.spawn(async move {
            let resolve = tokio::time::timeout(MAGNET_PREVIEW_TIMEOUT, async {
                let session = shared.session().await?;
                crate::torrent::resolve_magnet_preview(&session, &magnet).await
            });
            let result = tokio::select! {
                () = cancel.cancelled() => None,
                result = resolve => Some(match result {
                    Ok(result) => result.map_err(|error| error.short_label()),
                    Err(_) => Err("metadata lookup timed out".to_string()),
                }),
            };
            if let Some(result) = result {
                let _ = reply
                    .send(MagnetPreviewResult {
                        request_id,
                        magnet,
                        result,
                    })
                    .await;
            }
            let _ = loop_tx.send(Internal::PreviewDone(request_id));
        });
    }

    fn start_preview(&mut self, request_id: u64) -> Option<CancellationToken> {
        if let Some(index) = self.early_cancels.iter().position(|id| *id == request_id) {
            self.early_cancels.remove(index);
            return None;
        }
        let cancel = self.shutdown.child_token();
        self.previews.insert(request_id, cancel.clone());
        Some(cancel)
    }

    pub(super) fn cancel_preview(&mut self, request_id: u64) {
        if let Some(cancel) = self.previews.remove(&request_id) {
            cancel.cancel();
            return;
        }
        if self.early_cancels.len() == EARLY_CANCEL_MEMORY {
            self.early_cancels.pop_front();
        }
        self.early_cancels.push_back(request_id);
    }
}

fn fragment_filename(url: &str) -> Option<String> {
    let url = url::Url::parse(url).ok()?;
    let fragment = url.fragment()?.trim();
    (!fragment.is_empty()).then(|| crate::filename::sanitize_filename(fragment))
}
