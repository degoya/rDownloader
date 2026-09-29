//! Translation between our packages and qBittorrent's torrent vocabulary.

use rd_core::{DownloadFile, DownloadPackage, DownloadState, PackageState};

/// One torrent as this adapter sees it: a package, its single torrent job and its info hash.
///
/// The hash is the real BitTorrent info hash out of the stored metadata, never a derived id.
/// An automation client computes the hash itself from the torrent it handed over and looks
/// the download up by that value; a synthesized id would leave it waiting forever.
pub(crate) struct TorrentView {
    pub package: DownloadPackage,
    pub download: DownloadFile,
    pub hash: String,
    /// The package's category name, or the empty string qBittorrent uses for "none".
    ///
    /// Reported back as it is, because a client filters `torrents/info` by the category it
    /// configured and expects to find its own torrents there.
    pub category: String,
}

/// qBittorrent's `state` word for one of our downloads.
///
/// The vocabulary is qBittorrent's and the mapping is lossy in the places where ours is
/// finer. What a client acts on is the distinction between "still working", "done" and
/// "broken", plus whether it is paused; `pausedUP` in particular is what tells it a torrent
/// finished and may be imported.
#[must_use]
pub(crate) fn state(package: &DownloadPackage, download: &DownloadFile) -> &'static str {
    match package.state {
        PackageState::Completed => "pausedUP",
        PackageState::Failed => "error",
        _ => match download.state {
            DownloadState::Paused => "pausedDL",
            DownloadState::Queued => "queuedDL",
            DownloadState::Resolving => "metaDL",
            DownloadState::Verifying => "checkingDL",
            DownloadState::Repairing | DownloadState::Extracting => "checkingUP",
            DownloadState::Seeding => "uploading",
            DownloadState::Completed => "pausedUP",
            DownloadState::Failed | DownloadState::Cancelled => "error",
            DownloadState::Blocked => "stalledDL",
            DownloadState::RetryWait => "stalledDL",
            // Waiting on another link to the same file, which reads the same way from
            // outside: not running, not failed, nothing for the caller to do.
            DownloadState::Skipped => "stalledDL",
            DownloadState::Downloading => "downloading",
        },
    }
}

/// Whether a package has finished and its files may be imported.
#[must_use]
pub(crate) fn is_finished(package: &DownloadPackage) -> bool {
    package.state == PackageState::Completed
}

/// Progress as qBittorrent reports it: a fraction between 0 and 1.
#[must_use]
pub(crate) fn progress(download: &DownloadFile) -> f64 {
    let total = download.total_bytes.map_or(0, rd_core::ByteCount::get);
    if total == 0 {
        return 0.0;
    }
    (download.committed_bytes.get() as f64 / total as f64).clamp(0.0, 1.0)
}

/// The info hash carried by a magnet link, if it has a usable one.
///
/// Needed because a magnet has no metadata until the engine has talked to a peer, while the
/// client that just handed it over starts polling for its hash immediately — it computed
/// that hash itself from the same magnet. Without this a freshly added magnet would be
/// invisible for as long as metadata resolution takes, and the client would give up.
///
/// Only the 40-character v1 form is accepted. A base32 or v2 `btmh` value is left alone
/// rather than half-converted: a wrong hash is worse than a missing one, because the client
/// would then wait on a torrent that will never match.
#[must_use]
pub(crate) fn hash_from_magnet(source: &url::Url) -> Option<String> {
    if source.scheme() != "magnet" {
        return None;
    }
    source
        .query_pairs()
        .filter(|(key, _)| key == "xt")
        .find_map(|(_, value)| {
            let hash = value.strip_prefix("urn:btih:")?.to_ascii_lowercase();
            (hash.len() == 40 && hash.chars().all(|c| c.is_ascii_hexdigit())).then_some(hash)
        })
}

/// Normalises a hash the way qBittorrent does: lower-case hex, whitespace trimmed.
#[must_use]
pub(crate) fn normalize_hash(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::{hash_from_magnet, normalize_hash};

    #[test]
    fn a_v1_magnet_hash_is_recognised_before_metadata_arrives() {
        let magnet = url::Url::parse(
            "magnet:?xt=urn:btih:ABCDEF0123456789ABCDEF0123456789ABCDEF01&dn=Example",
        )
        .expect("magnet");
        assert_eq!(
            hash_from_magnet(&magnet).as_deref(),
            Some("abcdef0123456789abcdef0123456789abcdef01")
        );
    }

    #[test]
    fn anything_but_a_v1_hex_hash_is_left_alone() {
        // A base32 v1 hash and a v2 `btmh` are both real magnet forms. Guessing at a
        // conversion would hand a client a hash that never matches, which is worse than
        // showing it nothing until metadata arrives.
        for magnet in [
            "magnet:?xt=urn:btih:MFRGGZDFMZTWQ2LKNNWG23TPOBYXE43UOM",
            "magnet:?xt=urn:btmh:1220caf1e1c30e81cb361b9ee167c4aa64228a7fa4fa9f6105232b28ad099f3a302e",
            "magnet:?dn=NoHashAtAll",
            "magnet:?xt=urn:btih:tooshort",
        ] {
            let parsed = url::Url::parse(magnet).expect(magnet);
            assert_eq!(hash_from_magnet(&parsed), None, "{magnet}");
        }
        let http = url::Url::parse("https://example.com/x.torrent").expect("url");
        assert_eq!(hash_from_magnet(&http), None);
    }

    #[test]
    fn hashes_compare_case_insensitively() {
        // Clients differ on the case they send back; qBittorrent itself answers lower-case.
        assert_eq!(
            normalize_hash("  ABCDEF0123456789ABCDEF0123456789ABCDEF01 "),
            "abcdef0123456789abcdef0123456789abcdef01"
        );
    }
}
