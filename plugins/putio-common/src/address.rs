//! The one address a Put.io file has, written in one place and read in one place.
//!
//! The remote-job plugin hands a finished transfer's files to the LinkGrabber as
//! `https://api.put.io/v2/files/<id>/download`, and the resolver claims exactly that address
//! again. That is deliberate and it is the whole reason this module exists rather than a
//! format string at each end:
//!
//! - **It is stable.** `GET /v2/files/<id>/url` answers with a signed storage address that
//!   stops working after a while. Writing one of those into a queue would mean every job that
//!   waits its turn for an hour fails with a refusal nobody can act on. The address below
//!   names the file and nothing else, so Put.io mints the short-lived one at the moment the
//!   bytes are actually fetched and rDownloader never holds an expiring URL at all.
//! - **It carries no credential.** The account's token reaches this address the way it reaches
//!   every other transfer of an OAuth provider: the host attaches it, towards the hosts the
//!   `putio` provider row declared and nowhere else. An address with `oauth_token=` in its
//!   query — which Put.io also accepts — would be a credential written into the database, into
//!   the job list and into every log line that quoted the source.
//! - **It is resumable.** The scheduler re-asks the resolver before it continues a partial
//!   file, and an address whose only identity was an expiring signature would have nothing
//!   left to ask about.

/// The Put.io API, and the only host any of these plugins reaches.
pub const API: &str = "https://api.put.io/v2";

/// The host that serves the API.
pub const API_HOST: &str = "api.put.io";

/// The host of the web interface, where a person copies an address from.
pub const APP_HOST: &str = "app.put.io";

/// The stable download address of one file.
#[must_use]
pub fn download_url(file_id: u64) -> String {
    format!("{API}/files/{file_id}/download")
}

/// The metadata address of one file.
#[must_use]
pub fn file_url(file_id: u64) -> String {
    format!("{API}/files/{file_id}")
}

/// The file id an address names, or `None` when it names no Put.io file.
///
/// Four spellings are accepted and they are the four that actually occur: the two the
/// remote-job sibling emits, and the two a person copies out of the web interface. Anything
/// else — a folder listing, a share, a stranger's host — is not claimed, because a resolver
/// that claimed an address it cannot turn into bytes would take it away from whatever could.
///
/// The id is parsed as a number rather than carried as text. Put.io's ids are integers, and
/// the value is spliced back into a request path: a "file id" of `../account/info` would be a
/// request to somewhere else on the one host this plugin is allowed to reach, and a parser
/// that only ever produces digits cannot express that.
#[must_use]
pub fn claim(url: &str) -> Option<u64> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let (host, path) = rest.split_once('/')?;
    // The query and the fragment are not part of the decision: `?notunnel=1` names the same
    // file, and an anchor names nothing.
    let path = path
        .split(['?', '#'])
        .next()
        .unwrap_or(path)
        .trim_end_matches('/');
    let segments: Vec<&str> = path.split('/').collect();
    match (host, segments.as_slice()) {
        // `https://api.put.io/v2/files/123` and `…/123/download`, the sibling's own address.
        (API_HOST, ["v2", "files", id]) | (API_HOST, ["v2", "files", id, "download"]) => {
            parse_id(id)
        }
        // `https://app.put.io/files/123`, what the web interface shows.
        (APP_HOST, ["files", id]) => parse_id(id),
        _ => None,
    }
}

/// A file id, if the text is one.
///
/// Leading zeroes and a leading `+` are refused rather than normalised: `0123` and `123` would
/// otherwise be two spellings of one file, and two spellings are two rows in a duplicate check
/// that compares addresses.
fn parse_id(text: &str) -> Option<u64> {
    if text.is_empty() || (text.len() > 1 && text.starts_with('0')) {
        return None;
    }
    if !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::{claim, download_url, file_url};

    #[test]
    fn the_address_the_remote_job_writes_is_the_one_the_resolver_claims() {
        assert_eq!(
            download_url(12345),
            "https://api.put.io/v2/files/12345/download"
        );
        assert_eq!(claim(&download_url(12345)), Some(12345));
        assert_eq!(claim(&file_url(12345)), Some(12345));
    }

    #[test]
    fn the_web_interfaces_own_address_is_claimed_too() {
        assert_eq!(claim("https://app.put.io/files/987"), Some(987));
        assert_eq!(claim("https://app.put.io/files/987/"), Some(987));
        assert_eq!(
            claim("https://api.put.io/v2/files/987?notunnel=1"),
            Some(987)
        );
    }

    #[test]
    fn nothing_else_is_claimed() {
        for address in [
            // A folder listing is not a file.
            "https://app.put.io/files/987/children",
            // Somebody else's host, spelled to look like Put.io's.
            "https://api.put.io.example.invalid/v2/files/1",
            "https://example.invalid/v2/files/1",
            // An id that is not a number, including the shape that would leave the path.
            "https://api.put.io/v2/files/../account/info",
            "https://api.put.io/v2/files/abc",
            "https://api.put.io/v2/files/",
            // Two spellings of one file would be two rows in an address comparison.
            "https://api.put.io/v2/files/0123",
            // Not an address at all.
            "magnet:?xt=urn:btih:da39a3ee5e6b4b0d3255bfef95601890afd80709",
            "ftp://api.put.io/v2/files/1",
        ] {
            assert_eq!(claim(address), None, "{address}");
        }
    }
}
