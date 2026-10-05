//! Links that turn out to be documents: HLS/DASH manifests, and the torrent, NZB or playlist
//! behind an address that did not announce itself (RD-080-06, RD-080-11, RD-130-18).

use super::*;

impl LinkCheckService {
    /// Probes a media page; playlists fan out into additional candidates of the package.
    /// Classifies a direct HLS/DASH manifest before it is handed to the extractor
    /// (RD-080-06).
    ///
    /// Returns `true` when the candidate has been dealt with here — refused as DRM, or
    /// routed to the recorder as a live stream — and `false` when it is an ordinary VOD the
    /// yt-dlp probe should describe as usual.
    /// `force` skips the cheap gate below, for a link whose address already ends in
    /// `.m3u8`/`.mpd` and is therefore worth reading in full.
    pub(super) async fn classify_manifest(
        &self,
        candidate: &LinkCandidate,
        force: bool,
        guard: Option<&rd_http::AddressPolicy>,
    ) -> bool {
        let Ok(network) = self.client_for(&candidate.url, guard).await else {
            return false;
        };
        // Reading the body of every direct link to see whether it might be a playlist would
        // mean downloading a slice of every file in the queue. A HEAD first keeps that to
        // the responses that could plausibly be one: a manifest media type, or something
        // text-ish and small enough to be a document rather than a video.
        if !force {
            let probed = rd_http::probe_with_headers(
                &network.client,
                candidate.url.clone(),
                &network.headers,
            )
            .await;
            let Ok(probed) = probed else { return false };
            if !manifest_plausible(probed.content_type.as_deref(), probed.total_bytes) {
                return false;
            }
        }
        // Verbatim, not `peek_body_text`: that one strips tags and collapses whitespace to
        // summarise a hoster's HTML error page, which would leave a playlist as one line and
        // an MPD as nothing at all.
        let body = rd_http::fetch_text_verbatim(
            &network.client,
            candidate.url.clone(),
            &network.headers,
            rd_media::MAX_MANIFEST_BYTES,
        )
        .await;
        let Some(body) = body else { return false };
        let Some(kind) =
            rd_media::detect_manifest(&body.final_url, body.content_type.as_deref(), &body.text)
        else {
            return false;
        };
        // Relative URIs resolve against where the body actually came from, not against the
        // address that was pasted: a redirect to another host would otherwise make every
        // segment URL point at the wrong origin.
        let base = &body.final_url;
        let report = match kind {
            rd_media::ManifestKind::Hls => rd_media::parse_hls(&body.text, base),
            rd_media::ManifestKind::Dash => match rd_media::parse_dash(&body.text, base) {
                Ok(report) => report,
                Err(_) => return false,
            },
        };

        // A protected stream is a stated non-goal, so it ends here rather than as an obscure
        // ffmpeg failure ten minutes into a download. Deliberately without a code: the
        // protection scheme is the whole content of the sentence and no catalogue can carry
        // it, the same reason the host-key fingerprint stays untranslated below.
        if let Some(reason) = report.drm {
            tracing::info!(
                candidate_id = %candidate.id,
                detail = %reason.detail(),
                "manifest is DRM-protected; refusing"
            );
            self.record(
                candidate,
                None,
                Some(rd_core::CandidateMessage::plain(format!(
                    "DRM-protected stream ({})",
                    reason.detail()
                ))),
            )
            .await;
            return true;
        }

        // Credentials belong to the manifest's own origin. A CDN on a second hostname is
        // ordinary, but it does not inherit this link's session.
        let foreign = report.foreign_origins(base);
        if !foreign.is_empty() {
            tracing::debug!(
                candidate_id = %candidate.id,
                count = foreign.len(),
                "manifest references other origins; credentials stay with the manifest host"
            );
        }

        // A live manifest has no end, so it belongs to the recorder. Handing it to the file
        // downloader produces a job that can never reach 100 %.
        if report.class == rd_media::ManifestClass::Live {
            tracing::info!(candidate_id = %candidate.id, "manifest is live; routing to the recorder");
            if let Err(error) = self
                .inner
                .database
                .set_candidate_provider(candidate.id, rd_core::RECORD_PROVIDER.to_owned())
                .await
            {
                tracing::warn!(%error, "live manifest could not be routed to the recorder");
                return false;
            }
            let result = LinkCheckResult {
                url: candidate.url.clone(),
                status: LinkStatus::Online,
                file_name: None,
                size: None,
                media: None,
            };
            self.record(candidate, Some(result), None).await;
            return true;
        }
        false
    }

    /// Re-routes a direct link the server declares to be an NZB or a torrent
    /// (RD-080-11).
    ///
    /// A HEAD is enough: the content type is the whole signal, and fetching the document
    /// here would mean asking the indexer for it twice. Both Newznab and Torznab hand out
    /// download links with the identity in the header rather than the path, and so do the
    /// NZBHydra and Prowlarr proxies in front of them.
    ///
    /// Answers whether the link was re-routed, which the caller needs since RD-110-07: a
    /// small XML document is not `looks_downloadable`, and an NZB that has just been sent to
    /// its import path must not also be recorded as a page that is not a file.
    ///
    /// A torrent is read here as well (RD-120-68): its `info.name` is the release name, and
    /// the address it came from ends in whatever token the site hands out. The file tree is
    /// stored on the candidate the way an uploaded `.torrent` stores it, so the review and
    /// the info hash exist before the row does.
    pub(super) async fn reclassify_document(
        &self,
        candidate: &LinkCandidate,
        guard: Option<&rd_http::AddressPolicy>,
    ) -> Rerouted {
        let Ok(network) = self.client_for(&candidate.url, guard).await else {
            return Rerouted::No;
        };
        let Ok(probed) =
            rd_http::probe_with_headers(&network.client, candidate.url.clone(), &network.headers)
                .await
        else {
            return Rerouted::No;
        };
        let essence = probed
            .content_type
            .as_deref()
            .map(|value| {
                value
                    .split(';')
                    .next()
                    .unwrap_or(value)
                    .trim()
                    .to_ascii_lowercase()
            })
            .unwrap_or_default();
        let mut sniffed_torrent = None;
        let provider = if rd_core::NZB_CONTENT_TYPES.contains(&essence.as_str()) {
            rd_core::NZB_PROVIDER
        } else if rd_core::TORRENT_CONTENT_TYPES.contains(&essence.as_str()) {
            rd_core::TORRENT_PROVIDER
        } else if let Some(sniffed) = crate::link_check_probe::sniff_document(
            &network.client,
            &network.headers,
            &candidate.url,
        )
        .await
        {
            match sniffed {
                crate::link_check_probe::Sniffed::Nzb => rd_core::NZB_PROVIDER,
                crate::link_check_probe::Sniffed::Torrent(bytes) => {
                    sniffed_torrent = bytes;
                    rd_core::TORRENT_PROVIDER
                }
            }
        } else {
            // Logged rather than swallowed: this is where an indexer link that ends up being
            // downloaded as a document is decided, and the content type it answered with is
            // the one thing needed to work out why.
            tracing::debug!(
                url = %rd_core::redact_url(&candidate.url),
                content_type = %essence,
                "link was not recognised as a container"
            );
            return Rerouted::No;
        };
        tracing::info!(
            url = %rd_core::redact_url(&candidate.url),
            provider,
            "link re-routed to its import path"
        );
        if let Err(error) = self
            .inner
            .database
            .set_candidate_provider(candidate.id, provider.to_owned())
            .await
        {
            tracing::warn!(%error, provider, "link could not be re-routed to its import path");
        }
        if provider != rd_core::TORRENT_PROVIDER {
            return Rerouted::Container;
        }
        let parsed = crate::link_check_probe::read_torrent(
            &network.client,
            &network.headers,
            candidate,
            &self.inner.torrent,
            sniffed_torrent,
        )
        .await;
        if let Some(parsed) = &parsed
            && let Err(error) = self
                .inner
                .database
                .set_candidate_torrent_state(
                    candidate.id,
                    rd_core::TorrentCandidateState::ready(parsed.metadata.clone()),
                )
                .await
        {
            tracing::warn!(%error, "file tree of a re-routed torrent could not be stored");
        }
        Rerouted::Torrent(parsed.map(Box::new))
    }
}
