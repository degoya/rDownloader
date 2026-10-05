//! One connection fetching one chunk: request, resume position, then the body.

use std::sync::Arc;

use rd_files::PartFile;
use rd_limits::ScopedLimiter;
use reqwest::Client;
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::{ChunkSpec, hostlimit::HostLimits, transform::StreamTransform};

use super::{
    CheckpointSink, DownloadOutcome, HttpDownloadError, ReplayPayload, failure::network_failure,
};

pub(super) struct Worker {
    pub(super) client: Client,
    pub(super) limiter: ScopedLimiter,
    /// Shared across every transfer, so one host sees one budget.
    pub(super) hosts: HostLimits,
    /// Whether this worker's chunk is the whole file.
    pub(super) covers_whole_file: bool,
    pub(super) part: PartFile,
    pub(super) checkpoints: Arc<dyn CheckpointSink>,
    pub(super) cancellation: CancellationToken,
    pub(super) url: Url,
    pub(super) validator: Option<String>,
    pub(super) require_range: bool,
    pub(super) headers: Arc<Vec<(String, String)>>,
    pub(super) method: rd_core::ReplayMethod,
    pub(super) body: Option<ReplayPayload>,
    pub(super) approved_origins: Arc<Vec<String>>,
    pub(super) captured_user_agent: Option<String>,
    /// How this stream's bytes become the file. `None` for every ordinary download.
    pub(super) transform: Option<Arc<StreamTransform>>,
    /// Finished provider-chunk MACs, shared with every other worker of this file.
    pub(super) macs: Arc<std::sync::Mutex<std::collections::BTreeMap<usize, [u8; 16]>>>,
}

impl Worker {
    pub(super) async fn run(self, chunk: ChunkSpec) -> Result<DownloadOutcome, HttpDownloadError> {
        if self.cancellation.is_cancelled() {
            return Ok(DownloadOutcome::Paused);
        }
        let builder = self.build_request(&chunk);
        // One slot per host, held until this body is drained. Four chunks of one file and
        // several files of one hoster would otherwise open as many connections at once as
        // the queue happens to have work, which is what makes a hoster start answering
        // with throttle pages instead of bytes.
        let _slot = tokio::select! {
            () = self.cancellation.cancelled() => return Ok(DownloadOutcome::Paused),
            slot = self.hosts.acquire(&self.url) => slot,
        };
        let response = builder.send().await.map_err(network_failure)?;
        self.check_response(&response)?;
        let position = self.start_position(&chunk, &response)?;
        self.write_body(&chunk, response, position).await
    }
}
