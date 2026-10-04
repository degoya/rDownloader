//! Request building and failure classification for the shared logic.
//!
//! Everything here speaks [`plugin_common`] rather than either host's vocabulary, which is what
//! lets the module above it be compiled once for both targets.
//!
//! One thing differs from every site-specific plugin: there is no primary domain. A referer or a
//! link-host check has to be derived from the link being resolved, because which site this is
//! only becomes known when a URL arrives.

use plugin_common::{Failure, HttpResponse};

use crate::messages;

pub(crate) use xfs_common::glue::{coded, is_html, range_probe};

/// The XFS file code of a link, if the link is on a host this plugin claims.
///
/// The host list is consulted exactly, never by suffix. A suffix rule would claim
/// `xfs.example.org.attacker.test` and, worse, would make `matches()` answer yes for hosts the
/// sandbox never granted — which is the over-claiming the conformance check exists to catch.
pub(crate) fn file_code(url: &url::Url) -> Option<&str> {
    xfs_common::api::file_code(url, crate::HOSTERS)
}

/// Classifies a transport status with the mapping every plugin shares, keeping a 429's
/// `Retry-After` (`xfs_common::glue`, RD-191-07).
pub(crate) fn ensure_http_status(response: &HttpResponse) -> Result<(), Failure> {
    xfs_common::glue::ensure_http_status(response, messages::HTTP_ERROR, messages::http_error)
}

pub(crate) fn invalid_url(error: &url::ParseError) -> Failure {
    xfs_common::glue::invalid_url(error, messages::INVALID_URL, messages::invalid_url)
}
