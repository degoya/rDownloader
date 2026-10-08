//! Queue runner: downloads one whole gallery with `gallery-dl` into a subfolder of the
//! package destination. Progress is bytes/files seen so far; the total is unknown upfront.

use std::{path::PathBuf, sync::Arc};

use anyhow::Result;
use async_trait::async_trait;
use rd_core::{DownloadFile, DownloadKind, DownloadPackage, Failure, FailureKind};
use rd_db::Database;
use rd_scheduler::{ExternalRunner, RunOutcome};
use rd_tools::{
    LiveSlots, ProgressThrottle, ToolLine, ToolProcess,
    process::{Stdout, prepare},
};
use tokio_util::sync::CancellationToken;

use crate::SharedGallerySettings;

/// Downloads `DownloadKind::Gallery` files.
pub struct GalleryRunner {
    database: Database,
    settings: SharedGallerySettings,
    /// Read on every dispatch pass, so a changed setting needs no restart (audit 1.9.1, TR-07).
    slots: LiveSlots<rd_core::GallerySettings>,
}

impl GalleryRunner {
    #[must_use]
    pub fn new(database: Database, settings: SharedGallerySettings) -> Self {
        let slots = LiveSlots::new(Arc::clone(&settings), |settings| {
            settings.gallery_max_parallel
        });
        Self {
            database,
            settings,
            slots,
        }
    }
}

/// Folder name of the gallery below the package destination.
pub(crate) fn gallery_folder(file_name: &str) -> String {
    let stem = std::path::Path::new(file_name)
        .file_stem()
        .and_then(|value| value.to_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("gallery");
    rd_files::sanitize_file_name(stem)
}

/// Maps a gallery-dl failure (exit status + stderr tail) onto the retry policy.
pub(crate) fn map_gallery_error(stderr: &str) -> Failure {
    let lower = stderr.to_ascii_lowercase();
    if lower.contains("unsupported url") {
        return Failure::coded(
            FailureKind::Unsupported,
            "gallery.unsupported_url",
            "gallery-dl does not support this URL",
        );
    }
    if lower.contains("authentication") || lower.contains("login required") {
        return Failure::coded(
            FailureKind::Permanent,
            "gallery.auth_required",
            "This gallery requires authentication (configure it in gallery-dl.conf)",
        );
    }
    // Re-runs are cheap: gallery-dl skips files that already exist.
    rd_tools::tool_failed("gallery.tool_failed", stderr, "gallery-dl failed")
}

/// A run its deadline ended: the tool failed and is retried, where a stop read as the
/// person's own pause and was never tried again (re-audit 1.9.1, RA-TR-03).
fn timed_out(stderr: &str) -> Failure {
    rd_tools::tool_failed(
        "gallery.tool_failed",
        stderr,
        "gallery-dl went silent past its time limit",
    )
}

#[async_trait]
impl ExternalRunner for GalleryRunner {
    fn kind(&self) -> DownloadKind {
        DownloadKind::Gallery
    }

    /// gallery-dl skips files that already exist, so a re-run adopts what an earlier one stored;
    /// nothing is resumed mid-file and nothing is verified against a digest.
    fn reuse(&self) -> rd_core::ReuseCapability {
        rd_core::ReuseCapability {
            resume_partial: false,
            recheck_partial: false,
            adopt_completed: true,
            verify_completed: false,
            applies_collision_policy: false,
        }
    }

    fn slot_capacity(&self) -> usize {
        self.slots.get()
    }

    async fn run(
        &self,
        file: &DownloadFile,
        package: &DownloadPackage,
        cancellation: CancellationToken,
        limits: rd_scheduler::RunLimits,
    ) -> Result<RunOutcome> {
        let settings = self.settings.read().await.clone();
        // Leased before the version is assessed, and only gallery downloads stop when
        // gallery-dl is too old or listed as broken; both rules live in `prepare`.
        let tool = match prepare(
            "gallery-dl",
            rd_core::locate_tool_leased(
                settings.gallery_executable.as_deref(),
                settings.vendor_directory.as_deref(),
                "gallery-dl",
            )
            .map(|(tool, lease)| (tool.path, lease)),
            rd_tools::Capability::GalleryDownload,
        )
        .await
        {
            Ok(tool) => tool,
            Err(failure) => return Ok(RunOutcome::Failed(failure)),
        };
        let folder = gallery_folder(&file.file_name);
        let target = PathBuf::from(&package.destination).join(&folder);
        tokio::fs::create_dir_all(&target).await?;
        let mut command = tokio::process::Command::new(tool.path());
        // gallery-dl fetches over its own sockets, so the limit goes to the process. It
        // takes one rate for the whole job; see the bandwidth capability matrix.
        if let Some(binding) = limits.bandwidth.binding_limit() {
            command
                .arg("--limit-rate")
                .arg(binding.bytes_per_second.to_string());
        }
        command
            .arg("-D")
            .arg(&target)
            .arg("--")
            .arg(file.source.as_str());
        let mut process = ToolProcess::spawn(&mut command, "gallery-dl", Stdout::Read)?
            .with_silence_limit(rd_tools::SILENCE_LIMIT);
        // gallery-dl prints one path per stored file ("# path" for skipped ones). Sizes are
        // summed from disk; totals stay unknown, so the UI shows plain byte progress.
        let mut committed: u64 = 0;
        let mut throttle = ProgressThrottle::default();
        loop {
            let line = match process.next_line(cancellation.cancelled()).await? {
                ToolLine::Line(line) => line,
                ToolLine::End => break,
                ToolLine::Stopped => return Ok(RunOutcome::Stopped),
                // No deadline: a gallery runs as long as its files take. Only the silence
                // limit ends it, when gallery-dl has stored nothing for that long.
                ToolLine::TimedOut => {
                    return Ok(RunOutcome::Failed(timed_out(&process.stderr().await)));
                }
            };
            let path = line.trim().trim_start_matches("# ").trim();
            if path.is_empty() {
                continue;
            }
            if let Ok(meta) = tokio::fs::metadata(path).await {
                committed += meta.len();
            }
            if throttle.due() {
                let _ = self
                    .database
                    .set_download_progress(file.id, committed, None)
                    .await;
                throttle.mark();
            }
        }
        let status = process.wait().await?;
        let stderr_text = process.stderr().await;
        if !status.success() {
            return Ok(RunOutcome::Failed(map_gallery_error(&stderr_text)));
        }
        let _ = self
            .database
            .set_download_progress(file.id, committed, Some(committed))
            .await;
        Ok(RunOutcome::Completed { final_name: folder })
    }
}

#[cfg(test)]
mod tests {
    use super::{gallery_folder, map_gallery_error, timed_out};
    use rd_core::FailureKind;

    /// RA-TR-03: a deadline is a failure with a retry, not a stop.
    #[test]
    fn a_run_past_its_deadline_is_a_retryable_failure() {
        let failure = timed_out("");
        assert_eq!(failure.code.as_deref(), Some("gallery.tool_failed"));
        assert!(failure.category.is_retryable());
        assert!(
            failure.message.contains("time limit"),
            "{}",
            failure.message
        );
    }

    #[test]
    fn folder_name_is_sanitized_and_never_empty() {
        assert_eq!(gallery_folder("artworks"), "artworks");
        assert_eq!(gallery_folder("set: one?"), "set_ one_");
        assert_eq!(gallery_folder(""), "gallery");
    }

    #[test]
    fn errors_map_to_retry_policy() {
        assert!(matches!(
            map_gallery_error("error: Unsupported URL 'https://x'").category,
            FailureKind::Unsupported
        ));
        assert!(matches!(
            map_gallery_error("error: Login required").category,
            FailureKind::Permanent
        ));
        assert!(matches!(
            map_gallery_error("HTTPError 503").category,
            FailureKind::Transient { .. }
        ));
    }
}
