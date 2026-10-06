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

/// The codes this plugin's free flow reports its dead ends under (`xfs_common::free`,
/// RD-1110-03).
pub(crate) const FREE: xfs_common::free::FreeWords = xfs_common::free::FreeWords {
    http_error: plugin_common::HttpError {
        code: messages::HTTP_ERROR,
        text: messages::http_error,
    },
    no_free_form: (messages::NO_FREE_FORM, messages::no_free_form),
    no_free_link: (messages::NO_FREE_LINK, messages::no_free_link),
    free_limit_reached: (messages::FREE_LIMIT_REACHED, messages::free_limit_reached),
};

pub(crate) fn invalid_url(error: &url::ParseError) -> Failure {
    plugin_common::failure::invalid_url(messages::INVALID_URL, error).into()
}
