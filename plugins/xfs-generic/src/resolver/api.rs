//! Request building and failure classification for the shared logic.
//!
//! Everything here speaks [`plugin_common`] rather than either host's vocabulary, which is what
//! lets the module above it be compiled once for both targets.
//!
//! One thing differs from every site-specific plugin: there is no primary domain. A referer or a
//! link-host check has to be derived from the link being resolved, because which site this is
//! only becomes known when a URL arrives.

use plugin_common::Failure;

use crate::messages;

pub(crate) use xfs_common::glue::coded;

/// The XFS file code of a link, if the link is on a host this plugin claims.
///
/// The host list is consulted exactly, never by suffix. A suffix rule would claim
/// `xfs.example.org.attacker.test` and, worse, would make `matches()` answer yes for hosts the
/// sandbox never granted — which is the over-claiming the conformance check exists to catch.
pub(crate) fn file_code(url: &url::Url) -> Option<&str> {
    xfs_common::api::file_code(url, crate::HOSTERS)
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
