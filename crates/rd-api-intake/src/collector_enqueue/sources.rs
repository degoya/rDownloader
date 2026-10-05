//! Where a queue file is fetched from: the files of a reviewed remote directory or bucket, the
//! checksum a page declared, the address rule a proposed link keeps, and plugin schemes.

use super::*;

/// Expands a reviewed remote directory into one [`FileSpec`] per selected file.
///
/// Returns `None` for anything that is not a directory listing, so the caller falls back
/// to its ordinary one-candidate-one-file path. A single-file link also returns `None`:
/// there is nothing to expand, and keeping it on the normal path preserves the file name
/// the user may have edited.
pub(super) async fn remote_files(
    state: &AppState,
    candidate: &LinkCandidate,
    kind: rd_core::DownloadKind,
    reach: Option<bool>,
) -> Result<Option<Vec<FileSpec>>, ApiError> {
    let Some(stored) = state.database.candidate_listing(candidate.id).await? else {
        return Ok(None);
    };
    if stored.listing.single_file {
        return Ok(None);
    }
    // An object storage prefix: every selected key becomes its own `s3://`, `az://` or `gs://`
    // row, the profile hint of the link carried along so each one is signed by the same
    // profile.
    let object_base = rd_core::ObjectAddress::parse(&candidate.url).map(|address| {
        if address.is_prefix() {
            address
        } else {
            // The probe listed `shows` as the prefix `shows/` it meant.
            address.child("")
        }
    });
    let Some(base) = rd_core::RemoteTarget::parse(&candidate.url).and_then(|target| {
        // The queue row addresses the file itself, so the base is the collection URL the
        // listing was taken from.
        target.sanitized_url()
    }) else {
        return Ok(object_base.map(|base| object_files(candidate, &base, &stored, kind)));
    };
    let resolved = stored.resolve();
    let mut files = Vec::with_capacity(resolved.selected_files);
    for entry in resolved
        .entries
        .iter()
        .filter(|entry| !entry.is_dir && entry.included)
    {
        let Some(source) = rd_webdav::entry_url(&base, &entry.path) else {
            continue;
        };
        let file_name = entry
            .path
            .rsplit('/')
            .find(|segment| !segment.is_empty())
            .unwrap_or("download.bin")
            .to_owned();
        // Every file of a listing a stranger's link named keeps to that link's rule.
        let source_set = reach
            .and_then(|local_network| held_to_reach(kind, &source, local_network))
            .map(Box::new);
        files.push(FileSpec {
            source,
            file_name,
            size: entry.size,
            account_id: None,
            proxy_profile_id: None,
            // Every file of a remote listing inherits the candidate's choice.
            auth_profile: candidate.auth_profile,
            kind,
            media: None,
            replay: None,
            remote_credential_id: candidate.remote_credential_id,
            // Files of one remote listing are members of that listing, never alternatives
            // to each other.
            mirror_group: None,
            skipped: false,
            // Every file of the listing inherits what was found about the candidate.
            enrichment: candidate.enrichment.clone(),
            secret_fragment: None,
            source_set,
        });
    }
    Ok(Some(files))
}

/// The selected objects of a reviewed prefix, one queue row each.
pub(super) fn object_files(
    candidate: &LinkCandidate,
    base: &rd_core::ObjectAddress,
    stored: &rd_core::RemoteCandidateState,
    kind: rd_core::DownloadKind,
) -> Vec<FileSpec> {
    let resolved = stored.resolve();
    resolved
        .entries
        .iter()
        .filter(|entry| !entry.is_dir && entry.included)
        .filter_map(|entry| {
            let source = base.child(&entry.path).url()?;
            let file_name = entry
                .path
                .rsplit('/')
                .find(|segment| !segment.is_empty())
                .unwrap_or("download.bin")
                .to_owned();
            Some(FileSpec {
                source,
                file_name,
                size: entry.size,
                account_id: None,
                proxy_profile_id: None,
                auth_profile: candidate.auth_profile,
                kind,
                media: None,
                replay: None,
                remote_credential_id: None,
                mirror_group: None,
                skipped: false,
                enrichment: candidate.enrichment.clone(),
                secret_fragment: None,
                source_set: None,
            })
        })
        .collect()
}

/// The one source row a proposed link without mirrors is queued with (RD-150-03), for the
/// transports that fetch the link's own address: the HTTP transfer, FTP and SFTP, and the
/// tools that take a page address (yt-dlp, gallery-dl, streamlink). A torrent joins a swarm
/// its trackers name, and those are held to the rule where they are scraped; a transfer
/// plugin, Usenet and a bucket (whose endpoint is the person's own profile) have no address
/// of the link's to hold.
/// The single-source set of a link whose source declared the file's SHA-256 (RD-190-13): a
/// release file's digest, or its line in the release's checksum list.
///
/// The set is how a checksum reaches the download, and a set holds its source to an address
/// reach; a link the person added themselves keeps the local network open, as a pasted
/// Metalink's mirrors do. Without a declared checksum there is no set, and nothing changes.
pub(super) async fn declared_checksum(
    state: &AppState,
    candidate: &LinkCandidate,
    source: &url::Url,
    reach: Option<bool>,
) -> Result<Option<rd_core::SourceSet>, ApiError> {
    let attributes = state
        .database
        .candidate_source_attributes(candidate.id)
        .await?;
    let Some(value) = attributes
        .get("sha256")
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
    else {
        return Ok(None);
    };
    Ok(
        rd_core::SourceSet::of_link(source, reach.unwrap_or(true)).map(|mut set| {
            set.checksum = Some(rd_core::ExpectedChecksum {
                algorithm: rd_core::ChecksumAlgorithm::Sha256,
                value,
            });
            set
        }),
    )
}

pub(super) fn held_to_reach(
    kind: rd_core::DownloadKind,
    source: &url::Url,
    local_network: bool,
) -> Option<rd_core::SourceSet> {
    use rd_core::DownloadKind;
    match kind {
        DownloadKind::Http
        | DownloadKind::Ftp
        | DownloadKind::Sftp
        | DownloadKind::Media
        | DownloadKind::Gallery
        | DownloadKind::Record => rd_core::SourceSet::of_link(source, local_network),
        _ => None,
    }
}

/// Whether an installed transfer backend claims this URL scheme.
pub(super) fn claims_scheme(state: &AppState, scheme: &str) -> bool {
    state
        .plugin_transfer_schemes
        .iter()
        .any(|claimed| claimed.eq_ignore_ascii_case(scheme))
}
