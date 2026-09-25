//! The one address a Seedr file has, and the one way a Seedr request is authenticated.
//!
//! The remote-job plugin hands a finished transfer's files to the LinkGrabber as
//! `https://www.seedr.cc/rest/file/<id>`, and the resolver claims exactly that address again.
//! That is deliberate, and it is the whole reason this module exists rather than a format
//! string at each end:
//!
//! - **It is stable.** Seedr's documented file endpoint *is* the download — "Download file",
//!   followed with `-L` in its own example — so there is no signed one-shot address to mint and
//!   nothing in the queue can go stale. A provider whose finished job only ever yielded
//!   short-lived addresses would have a real problem here; this one does not, and that is a
//!   fact about Seedr rather than a choice this plugin made.
//! - **It is resumable.** The scheduler re-asks the resolver before it continues a partial
//!   file, and an address whose only identity was an expiring signature would have nothing left
//!   to ask about.
//!
//! **It does not carry a credential; the transfer adds one.** A plugin never holds the
//! password, so it cannot state an `Authorization` value for the queue. The `seedr` provider
//! row declares `transfer_auth = "basic"` instead, and the download engine attaches the Basic
//! pair of the account's e-mail address and password itself -- over TLS, to `www.seedr.cc`
//! only, decided again for the address the bytes actually come from, so a redirect onto
//! Seedr's storage or anywhere else does not inherit it (RD-120-38). No second authentication
//! profile holding the same password is needed.

/// Seedr's REST API, and the only address any of these plugins reaches.
pub const API: &str = "https://www.seedr.cc/rest";

/// The host that serves it.
pub const API_HOST: &str = "www.seedr.cc";

/// The bare domain, which a person may well paste instead.
pub const BARE_HOST: &str = "seedr.cc";

/// The vault reference the `seedr` provider keeps the account password under.
///
/// Named here so the two plugins cannot spell it differently; neither ever sees its value.
pub const PASSWORD_REFERENCE: &str = "seedr_password";

/// The `Authorization` value both plugins send, and the only credential shape Seedr has.
///
/// Seedr's own documentation states the limit plainly — "Rest API v1 is only available to use
/// with HTTP basic auth" — and there is no token variant in 2026: the page still announces a v2
/// and an OAuth API that have not arrived since its 2021 copyright. So every request carries
/// this template, the host expands it into base64 of the account's e-mail and password on the
/// way out, towards `www.seedr.cc` and nowhere else, and neither plugin holds either half.
pub const AUTHORIZATION_TEMPLATE: &str = "Basic {{basic:seedr_password}}";

/// Largest file or folder identifier that is read out of an address.
///
/// Seedr's identifiers are ordinary positive integers. The bound is not about Seedr: it is
/// about what a pasted address may contain, and `u64::MAX` digits repeated is not an id.
const MAX_ID_DIGITS: usize = 19;

/// The download address of one file, written in exactly one place.
#[must_use]
pub fn file_url(file_id: u64) -> String {
    format!("{API}/file/{file_id}")
}

/// The listing address of one folder; the root folder has no identifier at all.
#[must_use]
pub fn folder_url(folder_id: Option<u64>) -> String {
    match folder_id {
        Some(id) => format!("{API}/folder/{id}"),
        None => format!("{API}/folder"),
    }
}

/// `POST /rest/transfer/magnet`: hands a magnet to the account.
#[must_use]
pub fn add_magnet_url() -> String {
    format!("{API}/transfer/magnet")
}

/// `DELETE /rest/transfer/{id}`: removes a transfer that is still running.
#[must_use]
pub fn transfer_url(transfer_id: u64) -> String {
    format!("{API}/transfer/{transfer_id}")
}

/// `GET /rest/user`: what the account is.
#[must_use]
pub fn user_url() -> String {
    format!("{API}/user")
}

/// The file identifier in a Seedr address, or `None` when the address is not one of ours.
///
/// Read rather than trusted. The identifier comes back out of an address a person may have
/// pasted, and it goes into a request path, so anything that is not a plain positive integer is
/// refused here instead of being spliced into a URL somewhere else on the one host this plugin
/// may reach.
#[must_use]
pub fn claim(url: &str) -> Option<u64> {
    let rest = url
        .strip_prefix("https://www.seedr.cc/rest/file/")
        .or_else(|| url.strip_prefix("https://seedr.cc/rest/file/"))?;
    let id = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    parse_id(id)
}

/// A Seedr identifier, or `None` for anything that is not one.
///
/// Leading zeroes and a leading `+` are refused rather than normalised: `0123` and `123` would
/// be two spellings of one file, and two spellings mean two rows for one download.
#[must_use]
pub fn parse_id(value: &str) -> Option<u64> {
    if value.is_empty()
        || value.len() > MAX_ID_DIGITS
        || !value.bytes().all(|byte| byte.is_ascii_digit())
        || (value.len() > 1 && value.starts_with('0'))
    {
        return None;
    }
    value.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::{claim, file_url, folder_url, parse_id};

    #[test]
    fn the_address_one_plugin_writes_is_the_one_the_other_reads() {
        assert_eq!(file_url(42), "https://www.seedr.cc/rest/file/42");
        assert_eq!(claim(&file_url(42)), Some(42));
        assert_eq!(claim("https://seedr.cc/rest/file/7"), Some(7));
    }

    #[test]
    fn the_root_folder_has_no_identifier_and_a_subfolder_does() {
        assert_eq!(folder_url(None), "https://www.seedr.cc/rest/folder");
        assert_eq!(folder_url(Some(9)), "https://www.seedr.cc/rest/folder/9");
    }

    #[test]
    fn nothing_that_is_not_a_seedr_file_address_is_claimed() {
        for foreign in [
            "https://www.seedr.cc/rest/folder/42",
            "https://www.seedr.cc/files/42",
            "https://example.invalid/rest/file/42",
            "http://www.seedr.cc/rest/file/42",
            "magnet:?xt=urn:btih:da39a3ee",
        ] {
            assert_eq!(claim(foreign), None, "{foreign}");
        }
    }

    /// An identifier goes into a request path on the one host this plugin may reach, so a
    /// traversal attempt has to stop at the reader rather than at the URL builder.
    #[test]
    fn an_identifier_that_is_not_a_plain_number_is_refused() {
        for bad in [
            "",
            "0123",
            "+1",
            "1e3",
            "../user",
            "1/../user",
            &"9".repeat(25),
        ] {
            assert_eq!(parse_id(bad), None, "{bad}");
        }
        assert_eq!(parse_id("0"), Some(0));
        assert_eq!(claim("https://www.seedr.cc/rest/file/../user"), None);
    }
}
