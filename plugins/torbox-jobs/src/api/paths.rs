//! The three sets of paths and the field names TorBox spells differently per job kind.

use super::API_BASE;
use crate::source::Kind;

/// `POST` here to create a job of this kind.
#[must_use]
pub const fn create_path(kind: Kind) -> &'static str {
    match kind {
        Kind::Torrent => "/torrents/createtorrent",
        Kind::Usenet => "/usenet/createusenetdownload",
        Kind::Web => "/webdl/createwebdownload",
    }
}

/// `GET` here to read one job of this kind, or the account's list of them.
#[must_use]
pub const fn list_path(kind: Kind) -> &'static str {
    match kind {
        Kind::Torrent => "/torrents/mylist",
        Kind::Usenet => "/usenet/mylist",
        Kind::Web => "/webdl/mylist",
    }
}

/// `GET` here to mint a download address for one file of this kind of job.
#[must_use]
pub const fn request_path(kind: Kind) -> &'static str {
    match kind {
        Kind::Torrent => "/torrents/requestdl",
        Kind::Usenet => "/usenet/requestdl",
        Kind::Web => "/webdl/requestdl",
    }
}

/// `POST` here to delete a job of this kind.
#[must_use]
pub const fn control_path(kind: Kind) -> &'static str {
    match kind {
        Kind::Torrent => "/torrents/controltorrent",
        Kind::Usenet => "/usenet/controlusenetdownload",
        Kind::Web => "/webdl/controlwebdownload",
    }
}

/// `GET` here to ask whether TorBox holds content of this kind ready (RD-130-11).
#[must_use]
pub const fn check_cached_path(kind: Kind) -> &'static str {
    match kind {
        Kind::Torrent => "/torrents/checkcached",
        Kind::Usenet => "/usenet/checkcached",
        Kind::Web => "/webdl/checkcached",
    }
}

/// The query parameter `requestdl` names the job by.
#[must_use]
pub const fn request_id_field(kind: Kind) -> &'static str {
    match kind {
        Kind::Torrent => "torrent_id",
        Kind::Usenet => "usenet_id",
        Kind::Web => "web_id",
    }
}

/// The field the control endpoint names the job by.
///
/// Deliberately its own function rather than [`request_id_field`]: TorBox spells the web
/// download's identifier `web_id` when it mints an address and `webdl_id` when it deletes one,
/// and a single spelling would be wrong at one of the two ends.
#[must_use]
pub const fn control_id_field(kind: Kind) -> &'static str {
    match kind {
        Kind::Torrent => "torrent_id",
        Kind::Usenet => "usenet_id",
        Kind::Web => "webdl_id",
    }
}

/// The multipart field a source of this kind is submitted under, when it is submitted as text.
#[must_use]
pub const fn text_field(kind: Kind) -> &'static str {
    match kind {
        // A magnet; a container of this kind goes in as a file instead.
        Kind::Torrent => "magnet",
        Kind::Usenet => "link",
        Kind::Web => "link",
    }
}

/// The generic file name a part of this kind carries; see [`crate::upload`] for a container's.
///
/// TorBox reads the bytes, not the name, but a multipart part has to carry one and a name that
/// says what the part is beats a generic one in anybody's server log.
#[must_use]
pub const fn container_name(kind: Kind) -> &'static str {
    match kind {
        Kind::Torrent => "upload.torrent",
        Kind::Usenet => "upload.nzb",
        Kind::Web => "upload.bin",
    }
}

/// The stable address one finished file is fetched from.
///
/// **Without the token**, deliberately. `requestdl` needs the account's API key as a query
/// parameter and this plugin has none: it names secrets, it never holds them. So what travels
/// is the address that identifies the file and nothing else, `plugins/torbox/` claims it, and
/// the key is added by the host on every resolve -- which is also what makes the short-lived
/// ticket behind it renewable rather than a one-shot value written into a row.
#[must_use]
pub fn download_address(kind: Kind, remote_id: &str, file_id: u32) -> String {
    format!(
        "{API_BASE}{}?{}={remote_id}&file_id={file_id}",
        request_path(kind),
        request_id_field(kind)
    )
}
