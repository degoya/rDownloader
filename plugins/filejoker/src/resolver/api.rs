//! Request building and failure classification for the shared logic.
//!
//! Shorter than ddownload's counterpart: FileJoker exposes no JSON API, so there is no envelope
//! to classify and no key to carry — the cookie session is the whole credential.
//!
//! Everything here speaks [`plugin_common`] rather than either host's vocabulary, which is what
//! lets the module above it be compiled once for both.

use plugin_common::{Failure, HttpResponse};

use crate::messages;

/// FileJoker's own, and only, domain.
pub(crate) const PRIMARY_DOMAIN: &str = "filejoker.net";

/// Hosts a FileJoker link can carry, for [`xfs_common::api::file_code`].
pub(crate) const MATCH_HOSTS: &[&str] = &["filejoker.net", "www.filejoker.net"];

pub(crate) use xfs_common::glue::{coded, is_html, range_probe};

pub(crate) fn file_code(url: &url::Url) -> Option<&str> {
    xfs_common::api::file_code(url, MATCH_HOSTS)
}

/// Classifies a transport status with the mapping every plugin shares, keeping a 429's
/// `Retry-After` (`xfs_common::glue`, RD-191-07).
pub(crate) fn ensure_http_status(response: &HttpResponse) -> Result<(), Failure> {
    xfs_common::glue::ensure_http_status(response, messages::HTTP_ERROR, messages::http_error)
}

pub(crate) fn invalid_url(error: &url::ParseError) -> Failure {
    xfs_common::glue::invalid_url(error, messages::INVALID_URL, messages::invalid_url)
}
