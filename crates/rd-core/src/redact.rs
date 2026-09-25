//! Central redaction of credentials and signed-URL parameters.
//!
//! Every crate that turns a URL, a header or a transport error into something a human or a
//! client will see goes through here: `tracing` fields, [`Failure`] messages persisted in
//! `downloads.last_error_json`, the SSE payloads built from them, and REST error bodies.
//!
//! Two invariants the tests lock in:
//!
//! * **Idempotence.** The same value passes several call sites on its way out (engine →
//!   scheduler → database → SSE), so redacting an already-redacted string must be a no-op.
//! * **Structure preservation.** Parameter *names* survive; only values are replaced. A
//!   support log that has lost `X-Amz-Signature` entirely is much harder to read than one
//!   where the name is still visible with a placeholder value.

use chrono::{DateTime, NaiveDateTime, Utc};
use url::Url;

use crate::{Failure, error::MessageParams};

/// Replacement for every redacted value.
///
/// Deliberately bracketed so it cannot be mistaken for a real token, and deliberately free
/// of characters that `form_urlencoded` would encode differently on a second pass.
pub const REDACTION_PLACEHOLDER: &str = "[redacted]";

/// Query parameter names whose value is a credential or a signature.
///
/// Matched case-insensitively against the whole parameter name, never as a substring: a
/// substring rule would swallow `X-Amz-Expires` (because of `expires`) and cost us the
/// expiry we need in [`signed_url_expiry`].
pub const SIGNED_QUERY_SECRETS: &[&str] = &[
    // AWS SigV4 presigned (S3, CloudFront custom policy)
    "x-amz-signature",
    "x-amz-credential",
    "x-amz-security-token",
    // Azure Blob SAS
    "sig",
    "skoid",
    "sktid",
    "srt",
    // Google Cloud Storage V4 signed URLs
    "x-goog-signature",
    "x-goog-credential",
    // CloudFront canned and custom policies
    "signature",
    "policy",
    "key-pair-id",
    // Generic hoster and CDN conventions
    "token",
    "access_token",
    "auth_token",
    "api_key",
    "apikey",
    "auth",
    "authorization",
    "hash",
    "md5",
    "secret",
    "password",
    "passwd",
    "pwd",
    "session",
    "sid",
    "key",
    // Tracker, indexer and API-gateway spellings of the same thing (RD-120-57): the torrent
    // redaction already knew the first three, the subscription gate knows `rsstoken`, and the
    // shared list did not, so an address carrying one reached a log or a tool answer in clear.
    "passkey",
    "authkey",
    "torrent_pass",
    "rsstoken",
    "auth_key",
    "access_key",
    "secret_key",
    "client_secret",
    "private_token",
    "api-key",
    "x-api-key",
    // A Dropbox shared-link password, carried on the pasted address (RD-106-06)
    "link_password",
    // The same at Box, which spells its own parameter this way (RD-120-05). `password` above
    // does not cover it: the match is on the whole parameter name, not on a substring.
    "shared_link_password",
];

/// Parameter names whose presence marks a URL as signed or expiring.
///
/// Used to decide whether a stored transfer URL must be refreshed before old partial state
/// is reused; see the pre-resume refresh hook in `rd-scheduler`.
pub const SIGNED_QUERY_MARKERS: &[&str] = &[
    "x-amz-signature",
    "x-amz-date",
    "x-amz-expires",
    "x-goog-signature",
    "x-goog-date",
    "x-goog-expires",
    "sig",
    "se",
    "sp",
    "sv",
    "signature",
    "policy",
    "key-pair-id",
    "expires",
    "exp",
    "expire",
    "expiry",
    "validto",
];

/// Header names whose value is always a credential.
const CREDENTIAL_HEADERS: &[&str] = &[
    "cookie",
    "set-cookie",
    "authorization",
    "proxy-authorization",
    "www-authenticate",
];

/// Authentication schemes whose parameter is a credential.
const CREDENTIAL_SCHEMES: &[&str] = &["bearer ", "basic ", "digest "];

/// Whether a query parameter name carries a secret value.
#[must_use]
pub fn is_secret_parameter(name: &str) -> bool {
    let name = name.trim().to_ascii_lowercase();
    SIGNED_QUERY_SECRETS.contains(&name.as_str())
}

/// Whether the URL carries signature or expiry parameters, i.e. whether it is a
/// short-lived link rather than a stable one.
#[must_use]
pub fn is_signed_url(url: &Url) -> bool {
    url.query_pairs().any(|(name, _)| {
        let name = name.trim().to_ascii_lowercase();
        SIGNED_QUERY_MARKERS.contains(&name.as_str())
    })
}

/// Deadline encoded in a signed URL, if it can be read without guessing.
///
/// Covers the four conventions that actually appear in browser downloads: AWS SigV4
/// (`X-Amz-Date` + `X-Amz-Expires`), GCS V4 (`X-Goog-Date` + `X-Goog-Expires`), Azure SAS
/// (`se`, RFC 3339) and CloudFront (`Expires`, epoch seconds), plus the generic epoch
/// parameters hosters use. Returns `None` rather than a guess when nothing parses — the
/// caller treats an unreadable expiry as "signed but unknown", which is a weaker signal
/// than a wrong deadline.
#[must_use]
pub fn signed_url_expiry(url: &Url) -> Option<DateTime<Utc>> {
    let mut parameters: Vec<(String, String)> = Vec::new();
    for (name, value) in url.query_pairs() {
        parameters.push((name.trim().to_ascii_lowercase(), value.into_owned()));
    }
    let find = |wanted: &str| {
        parameters
            .iter()
            .find(|(name, _)| name == wanted)
            .map(|(_, value)| value.as_str())
    };

    // AWS and GCS both sign a start instant plus a lifetime in seconds.
    for (date_key, expires_key) in [
        ("x-amz-date", "x-amz-expires"),
        ("x-goog-date", "x-goog-expires"),
    ] {
        if let Some(date) = find(date_key)
            && let Some(expires) = find(expires_key)
            && let Some(signed_at) = parse_basic_iso8601(date)
            && let Ok(seconds) = expires.parse::<i64>()
            && let Some(deadline) = signed_at.checked_add_signed(chrono::TimeDelta::seconds(
                seconds.clamp(0, i64::from(u32::MAX)),
            ))
        {
            return Some(deadline);
        }
    }

    // Azure SAS states the end instant directly, as RFC 3339.
    if let Some(value) = find("se")
        && let Some(deadline) = parse_rfc3339(value)
    {
        return Some(deadline);
    }

    // CloudFront and most hosters use epoch seconds.
    for key in ["expires", "exp", "expire", "expiry", "validto"] {
        if let Some(value) = find(key) {
            if let Some(deadline) = parse_epoch_seconds(value) {
                return Some(deadline);
            }
            if let Some(deadline) = parse_rfc3339(value) {
                return Some(deadline);
            }
        }
    }
    None
}

/// `20240101T000000Z`, the basic ISO 8601 form AWS and GCS sign with.
fn parse_basic_iso8601(value: &str) -> Option<DateTime<Utc>> {
    NaiveDateTime::parse_from_str(value.trim(), "%Y%m%dT%H%M%SZ")
        .ok()
        .map(|naive| naive.and_utc())
}

fn parse_rfc3339(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value.trim())
        .ok()
        .map(|parsed| parsed.with_timezone(&Utc))
}

fn parse_epoch_seconds(value: &str) -> Option<DateTime<Utc>> {
    let seconds = value.trim().parse::<i64>().ok()?;
    // Reject values that are obviously not a second-precision epoch (a millisecond
    // timestamp, a version number, a byte count) rather than reporting the year 1970.
    if !(1_000_000_000..=32_503_680_000).contains(&seconds) {
        return None;
    }
    DateTime::from_timestamp(seconds, 0)
}

/// The URL with every credential-bearing query value and any userinfo replaced.
///
/// A URL with nothing to hide is returned byte-for-byte, so ordinary logs keep their exact
/// original text and only signed links change shape.
#[must_use]
pub fn redact_url(url: &Url) -> String {
    let mut redacted = url.clone();

    // `https://user:password@host/` appears in proxy endpoints and in some hoster links.
    if !url.username().is_empty() || url.password().is_some() {
        let _ = redacted.set_password(None);
        // A bracket-free marker, so a second pass re-writes the identical string instead of
        // percent-encoding the placeholder again.
        let _ = redacted.set_username("redacted");
    }

    let mut secrets_found = false;
    let pairs: Vec<(String, String)> = url
        .query_pairs()
        .map(|(name, value)| {
            if is_secret_parameter(&name) {
                secrets_found = true;
                (name.into_owned(), REDACTION_PLACEHOLDER.to_owned())
            } else {
                (name.into_owned(), value.into_owned())
            }
        })
        .collect();
    if secrets_found {
        redacted.query_pairs_mut().clear().extend_pairs(pairs);
    }
    redacted.into()
}

/// The header value with credentials removed, keyed by header name.
#[must_use]
pub fn redact_header_value(name: &str, value: &str) -> String {
    let lowered = name.trim().to_ascii_lowercase();
    if CREDENTIAL_HEADERS.contains(&lowered.as_str()) {
        return REDACTION_PLACEHOLDER.to_owned();
    }
    redact_text(value)
}

/// Free-text redaction for log lines, error messages and page excerpts.
///
/// Handles the four shapes a secret actually arrives in: a URL with signed query
/// parameters, an `Authorization`/`Cookie` header line, a bare `Bearer …` scheme, and a
/// `vault://` secret reference.
#[must_use]
pub fn redact_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for (index, line) in text.split_inclusive('\n').enumerate() {
        let _ = index;
        let (body, terminator) = match line.strip_suffix('\n') {
            Some(body) => (body, "\n"),
            None => (line, ""),
        };
        out.push_str(&redact_line(body));
        out.push_str(terminator);
    }
    out
}

/// Redacts one line, header-style patterns first.
fn redact_line(line: &str) -> String {
    if let Some(position) = line.find(':') {
        let name = line[..position].trim();
        if CREDENTIAL_HEADERS.contains(&name.to_ascii_lowercase().as_str()) {
            return format!("{}: {REDACTION_PLACEHOLDER}", line[..position].trim_end());
        }
    }
    redact_inline(line)
}

/// Replaces URLs, `vault://` references and auth schemes inside one line.
///
/// Scans by byte index; every marker searched for is ASCII, and an ASCII byte can never
/// occur inside a multi-byte UTF-8 sequence, so the slices are always on char boundaries.
fn redact_inline(line: &str) -> String {
    let lowered = line.to_ascii_lowercase();
    let mut out = String::with_capacity(line.len());
    let mut index = 0usize;

    while index < line.len() {
        // The remote transfer schemes are here because `ftp://user:password@host/` is the
        // ordinary way people paste an FTP link; without them the password would survive
        // into every log line and error message that quotes the source URL.
        if let Some(marker) = [
            "https://", "http://", "vault://", "ftps://", "ftp://", "sftp://",
        ]
        .into_iter()
        .find(|marker| lowered[index..].starts_with(marker))
        {
            let end = token_end(line, index);
            let token = &line[index..end];
            if marker == "vault://" {
                out.push_str("vault://");
                out.push_str(REDACTION_PLACEHOLDER);
            } else if let Ok(url) = Url::parse(token) {
                // The parsed form is normalised (`https://Host` becomes `https://host/`), so a
                // URL with nothing to hide keeps the text it arrived with rather than that.
                let redacted = redact_url(&url);
                if redacted == url.as_str() {
                    out.push_str(token);
                } else {
                    out.push_str(&redacted);
                }
            } else {
                out.push_str(token);
            }
            index = end;
            continue;
        }

        if let Some(scheme) = CREDENTIAL_SCHEMES
            .iter()
            .find(|scheme| lowered[index..].starts_with(**scheme))
            && is_word_start(line, index)
        {
            let value_start = index + scheme.len();
            let value_end = token_end(line, value_start);
            if value_end > value_start {
                out.push_str(&line[index..value_start]);
                out.push_str(REDACTION_PLACEHOLDER);
                index = value_end;
                continue;
            }
        }

        let next = next_char_boundary(line, index);
        out.push_str(&line[index..next]);
        index = next;
    }
    out
}

/// Whether the byte at `index` starts a word, so `member` does not look like `basic `.
fn is_word_start(line: &str, index: usize) -> bool {
    if index == 0 {
        return true;
    }
    line[..index]
        .chars()
        .next_back()
        .is_none_or(|previous| !previous.is_alphanumeric() && previous != '-' && previous != '_')
}

/// End of a URL-ish token: stops at whitespace or a delimiter, then gives back trailing
/// sentence punctuation so `see https://host/f.` does not swallow the full stop.
fn token_end(line: &str, start: usize) -> usize {
    let mut end = start;
    for (offset, character) in line[start..].char_indices() {
        if character.is_whitespace()
            || matches!(character, '"' | '\'' | '<' | '>' | '`' | '\\' | '|')
        {
            end = start + offset;
            break;
        }
        end = start + offset + character.len_utf8();
    }
    while end > start {
        // Never strip the bracket that closes a placeholder we produced ourselves, or a
        // second pass would emit `vault://[redacted]]`.
        if line[start..end].ends_with(REDACTION_PLACEHOLDER) {
            break;
        }
        let last = line[start..end].chars().next_back().unwrap_or(' ');
        if matches!(last, '.' | ',' | ';' | ':' | '!' | '?' | ')' | ']' | '}') {
            end -= last.len_utf8();
        } else {
            break;
        }
    }
    end
}

fn next_char_boundary(line: &str, index: usize) -> usize {
    line[index..]
        .chars()
        .next()
        .map_or(line.len(), |character| index + character.len_utf8())
}

/// Redacts every parameter value in place.
pub fn redact_params(params: &mut MessageParams) {
    for value in params.values_mut() {
        *value = redact_text(value);
    }
}

/// Redacts a failure's message and parameters, leaving its category and stable code alone.
///
/// This is the boundary applied in `rd-db` before a failure is persisted and broadcast, so
/// nothing reaches `downloads.last_error_json` or the `download.state` SSE event unredacted.
#[must_use]
pub fn redact_failure(mut failure: Failure) -> Failure {
    failure.message = redact_text(&failure.message);
    redact_params(&mut failure.params);
    failure
}

/// Displays a URL redacted, without allocating until it is actually formatted.
///
/// Lets a call site write `tracing::warn!(url = %Redacted(&url), …)` instead of building a
/// redacted `String` on every code path.
pub struct Redacted<'a>(pub &'a Url);

impl core::fmt::Display for Redacted<'_> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(&redact_url(self.0))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        REDACTION_PLACEHOLDER, Redacted, is_signed_url, redact_header_value, redact_text,
        redact_url, signed_url_expiry,
    };
    use url::Url;

    fn url(input: &str) -> Url {
        input.parse().expect("url")
    }

    #[test]
    fn plain_urls_are_returned_unchanged() {
        // Byte-for-byte, so ordinary logs do not churn just because redaction exists.
        let plain = "https://example.com/dir/file.bin?page=2&sort=name";
        assert_eq!(redact_url(&url(plain)), plain);
    }

    /// Redaction deliberately leaves a fragment standing (RD-109-32 confirmed RD-108-07).
    ///
    /// A fragment is an anchor far more often than a secret, and this function is not only a
    /// log helper: `replay_handlers` puts its result into a REST answer and `collector_enqueue`
    /// into `downloads.source_path`, where a shortened address would simply be the wrong
    /// address. The one address that could carry a password -- a pasted share link -- loses its
    /// fragment before it is ever stored (`rd_core::candidate_url`), so nothing reaching here
    /// from a candidate has one left to hide.
    #[test]
    fn a_fragment_is_left_alone_because_it_is_an_anchor_far_more_often_than_a_secret() {
        let anchor = "https://example.com/manual#installation";
        assert_eq!(redact_url(&url(anchor)), anchor);
        assert_eq!(
            crate::candidate_url(&url(anchor)).as_str(),
            "https://example.com/manual",
            "the storage rule is where a fragment is dropped, not redaction"
        );
    }

    #[test]
    fn signature_values_are_replaced_and_names_kept() {
        let redacted = redact_url(&url(
            "https://cdn.example.com/f.bin?X-Amz-Algorithm=AWS4-HMAC-SHA256\
             &X-Amz-Signature=deadbeefcafe&X-Amz-Expires=900",
        ));
        assert!(redacted.contains("X-Amz-Signature="));
        assert!(!redacted.contains("deadbeefcafe"));
        // The lifetime is not a secret and is needed to decide staleness.
        assert!(redacted.contains("X-Amz-Expires=900"));
    }

    #[test]
    fn userinfo_is_stripped_from_proxy_urls() {
        let redacted = redact_url(&url("http://agent:hunter2@proxy.internal:8080/"));
        assert!(!redacted.contains("hunter2"));
        assert!(redacted.contains("proxy.internal"));
    }

    #[test]
    fn remote_transfer_urls_lose_their_password_in_free_text() {
        // An FTP link is normally pasted *with* its credentials, so this is the common
        // case rather than an exotic one.
        for (input, secret) in [
            (
                "connecting to ftp://bob:hunter2@files.example.com/pub/a.bin",
                "hunter2",
            ),
            (
                "sftp://root:s3cr3t@10.0.0.5/srv/backup.tar failed",
                "s3cr3t",
            ),
            ("ftps://u:p%40ss@files.example.com/x refused", "p%40ss"),
        ] {
            let redacted = redact_text(input);
            assert!(!redacted.contains(secret), "{input} -> {redacted}");
            assert!(redacted.contains("example.com") || redacted.contains("10.0.0.5"));
        }
    }

    #[test]
    fn redaction_is_idempotent() {
        // The same value passes engine -> scheduler -> database -> SSE.
        for input in [
            "https://cdn.example.com/f?X-Amz-Signature=abc&X-Amz-Expires=900",
            "http://user:pw@proxy.internal:8080/",
            "Authorization: Bearer sk-livetoken",
            "Cookie: session=abc; other=def",
            "fetched vault://0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b for the job",
            "error sending request for url (https://cdn.example.com/f?sig=zzz)",
            "nothing sensitive here at all",
        ] {
            let once = redact_text(input);
            assert_eq!(redact_text(&once), once, "not idempotent: {input}");
        }
    }

    #[test]
    fn credential_header_lines_lose_their_value() {
        assert_eq!(
            redact_text("Authorization: Bearer sk-livetoken"),
            format!("Authorization: {REDACTION_PLACEHOLDER}")
        );
        assert_eq!(
            redact_text("Cookie: session=abc; other=def"),
            format!("Cookie: {REDACTION_PLACEHOLDER}")
        );
        assert_eq!(
            redact_header_value("Set-Cookie", "session=abc"),
            REDACTION_PLACEHOLDER
        );
        // A non-credential header keeps its value.
        assert_eq!(redact_header_value("Accept", "text/html"), "text/html");
    }

    #[test]
    fn bare_auth_schemes_are_redacted_mid_sentence() {
        let redacted = redact_text("sent Bearer sk-livetoken to the origin");
        assert!(!redacted.contains("sk-livetoken"));
        assert!(redacted.contains("Bearer [redacted]"));
        // A word merely ending in the scheme name must not trigger.
        assert_eq!(redact_text("membasic value"), "membasic value");
    }

    #[test]
    fn vault_references_never_survive() {
        let redacted = redact_text("stored as vault://0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b.");
        assert!(!redacted.contains("0190a1b2"));
        assert!(redacted.ends_with('.'), "punctuation kept: {redacted}");
    }

    #[test]
    fn urls_embedded_in_text_and_json_are_found() {
        let redacted = redact_text(
            "error sending request for url (https://cdn.example.com/f?sig=zzz&e=1)\n\
             {\"source\":\"https://cdn.example.com/g?token=secretvalue\"}",
        );
        assert!(!redacted.contains("zzz"));
        assert!(!redacted.contains("secretvalue"));
        // Structure around the URL survives.
        assert!(redacted.contains("{\"source\":\""));
    }

    #[test]
    fn signed_urls_are_recognised() {
        assert!(is_signed_url(&url("https://c.example/f?X-Amz-Signature=a")));
        assert!(is_signed_url(&url(
            "https://c.example/f?Expires=1735689600"
        )));
        assert!(!is_signed_url(&url("https://c.example/f?page=2")));
    }

    #[test]
    fn expiry_is_read_from_each_signing_convention() {
        let aws = signed_url_expiry(&url(
            "https://c.example/f?X-Amz-Date=20240101T000000Z&X-Amz-Expires=900",
        ))
        .expect("aws expiry");
        assert_eq!(aws.to_rfc3339(), "2024-01-01T00:15:00+00:00");

        let azure = signed_url_expiry(&url("https://c.example/f?se=2024-01-01T00%3A15%3A00Z"))
            .expect("azure expiry");
        assert_eq!(azure.to_rfc3339(), "2024-01-01T00:15:00+00:00");

        let cloudfront =
            signed_url_expiry(&url("https://c.example/f?Expires=1704070800")).expect("cf expiry");
        assert_eq!(cloudfront.timestamp(), 1_704_070_800);

        // A millisecond timestamp is not a second-precision epoch and must not be believed.
        assert!(signed_url_expiry(&url("https://c.example/f?Expires=1704070800000")).is_none());
        assert!(signed_url_expiry(&url("https://c.example/f?page=2")).is_none());
    }

    #[test]
    fn indexer_and_tracker_keys_are_secret_parameters() {
        for name in [
            "apikey", "api_key", "API-Key", "token", "key", "passkey", "authkey", "rsstoken",
        ] {
            let address = url(&format!(
                "https://indexer.example/api?t=get&{name}=K3Y&id=7"
            ));
            let redacted = redact_url(&address);
            assert!(!redacted.contains("K3Y"), "{name}: {redacted}");
            assert!(redacted.contains("id=7"), "{name}: {redacted}");
        }
    }

    #[test]
    fn a_url_in_free_text_with_nothing_to_hide_keeps_its_exact_text() {
        let text = "see https://Example.COM and https://example.com/a?page=2.";
        assert_eq!(redact_text(text), text);
    }

    #[test]
    fn display_wrapper_matches_the_function() {
        let signed = url("https://c.example/f?X-Amz-Signature=abc");
        assert_eq!(Redacted(&signed).to_string(), redact_url(&signed));
    }
}
