//! Translation between rDownloader ids, states and sizes and their SABnzbd spellings.

use rd_core::{DownloadPackage, PackageState, PostprocessStage};

/// Prefix that makes a SABnzbd job id recognisable as one of ours.
///
/// SABnzbd's own ids look like `SABnzbd_nzo_abc123`. Clients treat the value as opaque and
/// only ever hand it back, so a stable derivation from the package id is enough and needs no
/// second table: the mapping survives a restart because the package id does.
const NZO_PREFIX: &str = "rd_nzo_";

/// The SABnzbd job id of a package.
#[must_use]
pub(crate) fn nzo_id(package: &DownloadPackage) -> String {
    format!("{NZO_PREFIX}{}", package.id)
}

/// The package id behind a SABnzbd job id, if it is one of ours and well formed.
#[must_use]
pub(crate) fn package_id(nzo_id: &str) -> Option<rd_core::PackageId> {
    nzo_id.strip_prefix(NZO_PREFIX)?.parse().ok()
}

/// SABnzbd's queue status word for a package.
///
/// Deliberately lossy: SABnzbd has no separate repair, unpack and verify states, and a
/// client that polls the queue only distinguishes "still working" from "waiting" from
/// "broken". The finer stages stay in the native API.
#[must_use]
pub(crate) fn queue_status(package: &DownloadPackage) -> &'static str {
    match package.state {
        PackageState::Queued => "Queued",
        PackageState::Downloading => "Downloading",
        PackageState::Postprocessing => match package.postprocess.stage {
            Some(PostprocessStage::Repairing) => "Repairing",
            Some(PostprocessStage::Verifying) => "Verifying",
            Some(PostprocessStage::Extracting) => "Extracting",
            // Everything after unpacking — cleanup, remux, script, upload — reads as
            // "still busy" to a client that only waits for the slot to leave the queue.
            _ => "Running",
        },
        PackageState::Completed => "Completed",
        PackageState::Failed => "Failed",
    }
}

/// Whether a package belongs in the history rather than the queue.
///
/// SABnzbd splits the two: anything still being worked on is a queue slot, anything that
/// reached an end state is a history slot. A client that imports finished downloads polls
/// only the history, so a failed package has to appear there instead of lingering in the
/// queue where it would be waited on forever.
#[must_use]
pub(crate) fn is_history(package: &DownloadPackage) -> bool {
    matches!(
        package.state,
        PackageState::Completed | PackageState::Failed
    )
}

/// Megabytes as SABnzbd writes them: a decimal string, never a number.
#[must_use]
pub(crate) fn megabytes(bytes: u64) -> String {
    format!("{:.2}", bytes as f64 / (1024.0 * 1024.0))
}

/// A byte count in SABnzbd's human-readable spelling (`1.4 GB`).
#[must_use]
pub(crate) fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Seconds as `H:MM:SS`, SABnzbd's `timeleft` format.
#[must_use]
pub(crate) fn time_left(seconds: u64) -> String {
    format!(
        "{}:{:02}:{:02}",
        seconds / 3600,
        (seconds % 3600) / 60,
        seconds % 60
    )
}

#[cfg(test)]
mod tests {
    use super::{human_size, megabytes, package_id, time_left};

    #[test]
    fn a_job_id_round_trips_through_its_prefix() {
        let id = rd_core::PackageId::new();
        let package = package_for(id);
        let encoded = super::nzo_id(&package);
        assert!(encoded.starts_with("rd_nzo_"));
        assert_eq!(package_id(&encoded), Some(id));
    }

    #[test]
    fn a_foreign_or_malformed_job_id_is_refused() {
        // A client that kept an id from a real SABnzbd, or sent a truncated one, must not
        // resolve to some package of ours; there is no fallback parse.
        for value in [
            "SABnzbd_nzo_abc123",
            "rd_nzo_",
            "rd_nzo_not-a-uuid",
            "",
            "rd_nzo_00000000-0000-7000-8000",
        ] {
            assert_eq!(package_id(value), None, "{value} resolved to a package");
        }
    }

    #[test]
    fn sizes_use_sabnzbd_spellings() {
        assert_eq!(megabytes(1024 * 1024), "1.00");
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1024), "1.0 KB");
        assert_eq!(human_size(1024 * 1024 * 1024), "1.0 GB");
        assert_eq!(time_left(0), "0:00:00");
        assert_eq!(time_left(3661), "1:01:01");
    }

    fn package_for(id: rd_core::PackageId) -> rd_core::DownloadPackage {
        rd_core::DownloadPackage {
            id,
            name: "Example".to_owned(),
            state: rd_core::PackageState::Queued,
            created_at: chrono::Utc::now(),
            destination: "/downloads".to_owned(),
            category_id: None,
            priority: rd_core::DownloadPriority::Normal,
            position: 0,
            has_password: false,
            password: None,
            kind: rd_core::DownloadKind::Usenet,
            nzb_import_id: None,
            completed_at: None,
            postprocess_level: None,
            script: None,
            postprocess: rd_core::PostprocessStatus::default(),
            extraction_result: None,
            enrichment: Vec::new(),
        }
    }
}
