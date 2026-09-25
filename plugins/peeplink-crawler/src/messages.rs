//! User-facing texts and stable failure codes.
//!
//! Each `(code, message)` pair exists exactly once, and `locales/` translates exactly these.
#![allow(dead_code)] // The native tests and the guest use different subsets.

/// The entry is gone: `404`, or the service redirected the identifier to its own front page.
///
/// Both were measured, and the service tells them apart itself: an identifier it never knew
/// answers `404`, one it has deleted answers `200` at the root. The same code carries the
/// defensive case of an address this plugin does not claim at all, which is reported as
/// `unsupported` so the selection carries the address on instead of ending the link.
pub(crate) const ENTRY_NOT_FOUND: (&str, &str) = (
    "peeplink_crawler.entry_not_found",
    "This PEEPLink entry does not exist any more",
);

/// The entry page was read and its `<article>` names no link to anywhere else.
pub(crate) const ENTRY_EMPTY: (&str, &str) = (
    "peeplink_crawler.entry_empty",
    "This PEEPLink entry holds no links",
);

/// The entry asks for its access password and the pasted address carried none.
pub(crate) const PASSWORD_REQUIRED: (&str, &str) = (
    "peeplink_crawler.password_required",
    "This PEEPLink entry is password protected: add the password to the address after a # to open it",
);

/// The password was sent and the entry asked for it again.
pub(crate) const PASSWORD_WRONG: (&str, &str) = (
    "peeplink_crawler.password_wrong",
    "The service did not accept the password given with this PEEPLink address",
);

/// The service did not answer, or answered with something that is not an entry page.
pub(crate) const SITE_UNREACHABLE: (&str, &str) = (
    "peeplink_crawler.site_unreachable",
    "This PEEPLink entry could not be read: the service did not answer",
);
