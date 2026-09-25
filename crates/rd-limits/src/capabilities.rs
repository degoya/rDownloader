//! What each transport can actually enforce.
//!
//! Mirrors the torrent capability matrix: a limit the engine behind a runner cannot apply
//! is reported as unenforced instead of being accepted and silently ignored.

use rd_core::DownloadKind;
use serde::Serialize;
use utoipa::ToSchema;

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct RunnerLimitSupport {
    pub kind: DownloadKind,
    /// Whether the download limit reaches this transport at all.
    pub download_enforced: bool,
    /// Whether the transport can be limited per host, account or category, or only as a
    /// whole. External helper processes only take one rate for the whole job.
    pub scoped_enforced: bool,
    /// Short reason shown next to an unenforced entry; `None` when fully enforced.
    pub note: Option<&'static str>,
}

/// The capability matrix of the built-in transports.
#[must_use]
pub fn limit_capabilities() -> Vec<RunnerLimitSupport> {
    vec![
        RunnerLimitSupport {
            kind: DownloadKind::Http,
            download_enforced: true,
            scoped_enforced: true,
            note: None,
        },
        RunnerLimitSupport {
            kind: DownloadKind::Usenet,
            download_enforced: true,
            scoped_enforced: true,
            note: None,
        },
        RunnerLimitSupport {
            kind: DownloadKind::Torrent,
            download_enforced: true,
            // librqbit takes one session-wide rate, so a per-host or per-category torrent
            // limit cannot be expressed.
            scoped_enforced: false,
            note: Some("engine applies one session-wide rate"),
        },
        RunnerLimitSupport {
            kind: DownloadKind::Media,
            download_enforced: true,
            scoped_enforced: false,
            note: Some("yt-dlp takes one rate per job (--limit-rate)"),
        },
        RunnerLimitSupport {
            kind: DownloadKind::Gallery,
            download_enforced: true,
            scoped_enforced: false,
            note: Some("gallery-dl takes one rate per job (--limit-rate)"),
        },
        RunnerLimitSupport {
            kind: DownloadKind::Record,
            // streamlink has no rate option, and throttling a live stream would drop the
            // recording rather than slow it down.
            download_enforced: false,
            scoped_enforced: false,
            note: Some("streamlink has no rate limit; a live recording cannot be slowed"),
        },
    ]
}

#[cfg(test)]
mod tests {
    use rd_core::DownloadKind;

    use super::limit_capabilities;

    #[test]
    fn every_transport_states_what_it_enforces() {
        let capabilities = limit_capabilities();
        for kind in [
            DownloadKind::Http,
            DownloadKind::Usenet,
            DownloadKind::Torrent,
            DownloadKind::Media,
            DownloadKind::Gallery,
            DownloadKind::Record,
        ] {
            let entry = capabilities
                .iter()
                .find(|entry| entry.kind == kind)
                .unwrap_or_else(|| panic!("{kind:?} is missing from the capability matrix"));
            // Anything not fully enforced has to say why, or the UI cannot explain it.
            if !entry.download_enforced || !entry.scoped_enforced {
                assert!(entry.note.is_some(), "{kind:?} needs a note");
            }
        }
    }
}
