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
    nzo_id_of(package.id)
}

/// The SABnzbd job id of a package by its id alone, for one that left the queue and lives on
/// in the download history (RD-1100-04).
#[must_use]
pub(crate) fn nzo_id_of(id: rd_core::PackageId) -> String {
    format!("{NZO_PREFIX}{id}")
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

/// Whether a package is one this adapter reports and acts on: a Usenet one.
///
/// SABnzbd knows NZB jobs only, as the qBittorrent adapter knows torrents only. A torrent
/// listed here too, under the category both clients share, was imported twice by Sonarr and
/// credited to the wrong client by Radarr (RD-1490-13).
#[must_use]
pub(crate) fn is_usenet(package: &DownloadPackage) -> bool {
    package.kind == rd_core::DownloadKind::Usenet
}

/// What SABnzbd's `priority` of `addfile` asks for: the priority, and whether the job starts
/// paused.
///
/// SABnzbd's values: `-100` the category's default, `-2` paused, `-1` low, `0` normal, `1`
/// high, `2` force. Force has no rung of its own here and is high; anything else is normal.
#[must_use]
pub(crate) fn add_priority(value: Option<&str>) -> (rd_core::DownloadPriority, bool) {
    match value.map(str::trim) {
        Some("-2") => (rd_core::DownloadPriority::Normal, true),
        Some("-1") => (rd_core::DownloadPriority::Low, false),
        Some("1" | "2") => (rd_core::DownloadPriority::High, false),
        _ => (rd_core::DownloadPriority::Normal, false),
    }
}

/// SABnzbd's priority word of a queue slot.
#[must_use]
pub(crate) fn priority_word(priority: rd_core::DownloadPriority) -> &'static str {
    match priority {
        rd_core::DownloadPriority::Low => "Low",
        rd_core::DownloadPriority::Normal => "Normal",
        rd_core::DownloadPriority::High => "High",
    }
}

/// SABnzbd's category of a package: the name of its category, or `*` when it has none.
///
/// Reported as it is, because an automation client keeps only the queue and history entries
/// whose category is the one it set on `addfile`; a fixed `*` hid every job it added.
#[must_use]
pub(crate) fn category<'a>(
    package: &DownloadPackage,
    categories: &'a [rd_core::Category],
) -> &'a str {
    categories
        .iter()
        .find(|category| Some(category.id) == package.category_id)
        .map_or("*", |category| category.name.as_str())
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
    use super::{add_priority, human_size, megabytes, package_id, time_left};

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

    #[test]
    fn addfile_priorities_follow_sabnzbd_with_minus_two_paused() {
        use rd_core::DownloadPriority::{High, Low, Normal};
        assert_eq!(add_priority(Some("-2")), (Normal, true));
        assert_eq!(add_priority(Some("-1")), (Low, false));
        assert_eq!(add_priority(Some("0")), (Normal, false));
        assert_eq!(add_priority(Some("1")), (High, false));
        assert_eq!(add_priority(Some("2")), (High, false));
        for value in [Some("-100"), Some(""), Some("paused"), None] {
            assert_eq!(add_priority(value), (Normal, false), "{value:?}");
        }
    }

    #[test]
    fn only_a_usenet_package_is_a_sabnzbd_job() {
        let mut package = package_for(rd_core::PackageId::new());
        assert!(super::is_usenet(&package));
        for kind in [rd_core::DownloadKind::Torrent, rd_core::DownloadKind::Http] {
            package.kind = kind;
            assert!(!super::is_usenet(&package), "{kind:?}");
        }
    }

    fn package_for(id: rd_core::PackageId) -> rd_core::DownloadPackage {
        rd_core::DownloadPackage {
            id,
            name: "Example".to_owned(),
            state: rd_core::PackageState::Queued,
            created_at: rd_core::Timestamp::now(),
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
            start_after: None,
            download_window: None,
        }
    }
}
