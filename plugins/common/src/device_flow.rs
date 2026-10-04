//! Reading an OAuth device flow's answers (RFC 8628), once (RD-191-07, PLUG-08).
//!
//! `debridlink-auth` and `premiumize-auth` carried this file twice, differing in nothing but
//! the client id and the address in a test. What stays in a plugin is what really differs: the
//! endpoints, the form it posts, its client id and the slug its translation codes carry.
//!
//! The two documents are small and flat — five fields between them — so they are scanned with
//! [`crate::json`] rather than parsed. Plain Rust with no dependencies, so a guest that takes it
//! gains no import.

use crate::json::{number_field, string_field};

/// What the device-code endpoint answered.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_url: String,
    pub expires_in: Option<u64>,
    pub interval: Option<u64>,
}

/// Reads the device-code answer, or `None` when it is not one.
#[must_use]
pub fn device_code(body: &str) -> Option<DeviceCode> {
    let device_code = string_field(body, "device_code")?;
    let user_code = string_field(body, "user_code")?;
    // Providers spell this both ways; taking either avoids a flow that works everywhere
    // except where somebody chose the other name.
    let verification_url = string_field(body, "verification_url")
        .or_else(|| string_field(body, "verification_uri"))?;
    (!device_code.is_empty() && !user_code.is_empty() && !verification_url.is_empty()).then(|| {
        DeviceCode {
            device_code,
            user_code,
            verification_url,
            expires_in: number_field(body, "expires_in"),
            interval: number_field(body, "interval"),
        }
    })
}

/// The interval a flow polls at when the provider named none, in seconds (RFC 8628 §3.2).
pub const DEFAULT_INTERVAL: u64 = 5;

/// What a `slow_down` adds to the interval, in seconds (RFC 8628 §3.5).
pub const SLOW_DOWN_STEP: u64 = 5;

/// What a plugin hands the host to keep between polls: the device code, and the interval the
/// provider asked for at the start.
///
/// A guest is instantiated fresh for every call and remembers nothing, and the host hands the
/// flow state back verbatim, so the start interval travels in it — otherwise every poll after
/// the first would guess five seconds whatever the provider said.
#[must_use]
pub fn flow_state(code: &DeviceCode) -> String {
    format!("{}:{}", start_interval(code.interval), code.device_code)
}

/// The device code and the start interval a [`flow_state`] holds. A state without the interval
/// — a bare device code — reads with [`DEFAULT_INTERVAL`].
#[must_use]
pub fn read_flow_state(state: &str) -> (&str, u64) {
    match state.split_once(':') {
        Some((interval, device_code))
            if !interval.is_empty() && interval.bytes().all(|byte| byte.is_ascii_digit()) =>
        {
            (device_code, start_interval(interval.parse().ok()))
        }
        _ => (state, DEFAULT_INTERVAL),
    }
}

/// The interval a provider named, or [`DEFAULT_INTERVAL`] for none or `0`.
fn start_interval(interval: Option<u64>) -> u64 {
    interval
        .filter(|seconds| *seconds > 0)
        .unwrap_or(DEFAULT_INTERVAL)
}

/// The stand-in `poll` reports when the provider's answer had nothing in it a plugin
/// understands. Code-shaped on purpose: it travels the same path as a provider's own error
/// code, through [`refusal_code`] and [`sanitize_error`], and prose would be dropped there.
pub const UNREADABLE: &str = "unreadable";

/// What a poll means.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PollOutcome {
    /// The person confirmed; the payload is the credential to store.
    Authorized(String),
    /// Not yet; poll again after this many seconds.
    Pending(u64),
    /// Over: the code expired, or the person declined.
    Failed(String),
}

/// Reads a token-endpoint answer of HTTP `status`, polled at `interval` seconds (the start
/// interval [`read_flow_state`] gives back).
///
/// A `slow_down` waits five seconds longer than the interval (RFC 8628 §3.5). The contract has
/// no place for a guest to keep a grown interval, so the next `authorization_pending` waits
/// the start interval again; a provider that still finds it too fast says `slow_down` again,
/// and each of those waits the longer interval.
#[must_use]
pub fn poll(status: u16, body: &str, interval: u64) -> PollOutcome {
    if let Some(token) = string_field(body, "access_token").filter(|token| !token.is_empty()) {
        return PollOutcome::Authorized(token);
    }
    match string_field(body, "error").unwrap_or_default().as_str() {
        // The two the specification defines as "keep waiting". Everything else has ended.
        "authorization_pending" => PollOutcome::Pending(interval),
        "slow_down" => PollOutcome::Pending(interval.saturating_add(SLOW_DOWN_STEP)),
        // A provider that is down answers with its own error page rather than a document. That
        // ends this poll, not the sign-in: the person may be confirming right now, and the
        // flow's own expiry bounds the waiting.
        _ if (500..=599).contains(&status) => PollOutcome::Pending(interval),
        "" => PollOutcome::Failed(UNREADABLE.to_owned()),
        other => PollOutcome::Failed(other.to_owned()),
    }
}

/// The translation code a refusal is reported under, without the plugin's slug, so the
/// interface can say it in the language the person reads. A plugin's catalogue in `locales/`
/// carries each of these under its own slug.
///
/// Every refusal used to be reported as `flow_expired`, whatever it was — so a person whose
/// account was blocked was told their code had expired and went round again (RD-106-01).
#[must_use]
pub fn refusal_code(error: &str) -> &'static str {
    match error {
        "access_denied" | "consent_required" | "interaction_required" => "consent_denied",
        "expired_token" | "invalid_request" => "flow_expired",
        UNREADABLE => "bad_reply",
        _ => "sign_in_refused",
    }
}

/// A provider's `error` code, reduced to something that is safe to put in a message.
///
/// The point is not tidiness. Whatever a provider sends back travels into a log line and into
/// the failure the interface shows, and an endpoint that echoed part of a token into its error
/// document would otherwise publish it. RFC 6749 error codes are lowercase words joined by
/// underscores, so anything that is not exactly that shape is dropped whole rather than
/// filtered character by character — filtering would keep the digits of a leaked token.
#[must_use]
pub fn sanitize_error(error: &str) -> String {
    let trimmed = error.trim();
    let is_error_code = !trimmed.is_empty()
        && trimmed.len() <= 40
        && trimmed.chars().all(|c| c.is_ascii_lowercase() || c == '_');
    if is_error_code {
        trimmed.to_owned()
    } else {
        "refused".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_INTERVAL, DeviceCode, PollOutcome, UNREADABLE, device_code, flow_state, poll,
        read_flow_state, refusal_code, sanitize_error,
    };

    const STARTED: &str = r#"{
      "device_code": "abc123",
      "user_code": "WXYZ-1234",
      "verification_url": "https:\/\/provider.invalid\/webapp\/authorize",
      "expires_in": 600,
      "interval": 5
    }"#;

    #[test]
    fn a_device_code_answer_is_read_whole() {
        assert_eq!(
            device_code(STARTED),
            Some(DeviceCode {
                device_code: "abc123".to_owned(),
                user_code: "WXYZ-1234".to_owned(),
                verification_url: "https://provider.invalid/webapp/authorize".to_owned(),
                expires_in: Some(600),
                interval: Some(5),
            })
        );
    }

    #[test]
    fn either_spelling_of_the_address_field_is_accepted() {
        let other = STARTED.replace("verification_url", "verification_uri");
        assert_eq!(
            device_code(&other).map(|code| code.verification_url),
            Some("https://provider.invalid/webapp/authorize".to_owned())
        );
    }

    #[test]
    fn an_answer_missing_what_the_flow_needs_is_not_one() {
        assert_eq!(device_code(r#"{"error":"invalid_client"}"#), None);
        assert_eq!(device_code(r#"{"device_code":"abc"}"#), None);
    }

    #[test]
    fn only_the_two_defined_waiting_errors_keep_the_flow_open() {
        assert_eq!(
            poll(200, r#"{"access_token":"tok","token_type":"bearer"}"#, 5),
            PollOutcome::Authorized("tok".to_owned())
        );
        assert_eq!(
            poll(400, r#"{"error":"authorization_pending"}"#, 5),
            PollOutcome::Pending(5)
        );
        // Anything else has ended, and going on asking would keep a dead flow alive.
        assert_eq!(
            poll(400, r#"{"error":"expired_token"}"#, 5),
            PollOutcome::Failed("expired_token".to_owned())
        );
        assert_eq!(
            poll(200, "<html>oops</html>", 5),
            PollOutcome::Failed(UNREADABLE.to_owned())
        );
    }

    /// A wait follows the interval the provider asked for at the start, and a `slow_down` adds
    /// five seconds to it (RFC 8628 §3.5) rather than replacing it with ten (RA-PLG-06).
    #[test]
    fn a_wait_follows_the_providers_interval() {
        assert_eq!(
            poll(400, r#"{"error":"authorization_pending"}"#, 8),
            PollOutcome::Pending(8)
        );
        assert_eq!(
            poll(400, r#"{"error":"slow_down"}"#, 5),
            PollOutcome::Pending(10)
        );
        assert_eq!(
            poll(400, r#"{"error":"slow_down"}"#, 20),
            PollOutcome::Pending(25)
        );
        assert_eq!(
            poll(400, r#"{"error":"slow_down"}"#, u64::MAX),
            PollOutcome::Pending(u64::MAX)
        );
    }

    /// A provider's error page is an outage, not a refusal: the sign-in keeps polling, where it
    /// used to end as `bad_reply` (RA-PLG-06).
    #[test]
    fn a_server_error_page_keeps_the_flow_open() {
        assert_eq!(poll(502, "<html>502</html>", 5), PollOutcome::Pending(5));
        assert_eq!(poll(503, "", 7), PollOutcome::Pending(7));
        assert_eq!(
            poll(503, r#"{"error":"slow_down"}"#, 5),
            PollOutcome::Pending(10)
        );
    }

    /// The start interval travels in the flow state the host keeps, and a state without one
    /// reads with the default.
    #[test]
    fn the_start_interval_survives_between_polls() {
        let started = device_code(STARTED).expect("a device code");
        let kept = flow_state(&started);
        assert_eq!(read_flow_state(&kept), ("abc123", 5));
        let slow = DeviceCode {
            interval: Some(30),
            device_code: "with:colon".to_owned(),
            ..started.clone()
        };
        assert_eq!(read_flow_state(&flow_state(&slow)), ("with:colon", 30));
        let unnamed = DeviceCode {
            interval: None,
            ..started.clone()
        };
        assert_eq!(
            read_flow_state(&flow_state(&unnamed)),
            ("abc123", DEFAULT_INTERVAL)
        );
        let zero = DeviceCode {
            interval: Some(0),
            ..started
        };
        assert_eq!(
            read_flow_state(&flow_state(&zero)),
            ("abc123", DEFAULT_INTERVAL)
        );
        assert_eq!(
            read_flow_state("bare-code"),
            ("bare-code", DEFAULT_INTERVAL)
        );
        assert_eq!(read_flow_state("x:y"), ("x:y", DEFAULT_INTERVAL));
    }

    #[test]
    fn each_refusal_reports_the_code_that_fits_it() {
        assert_eq!(refusal_code("access_denied"), "consent_denied");
        assert_eq!(refusal_code("expired_token"), "flow_expired");
        assert_eq!(refusal_code("invalid_grant"), "sign_in_refused");
        assert_eq!(refusal_code("something_new"), "sign_in_refused");
        // An answer that cannot be read says so, rather than claiming the code expired.
        assert_eq!(refusal_code(UNREADABLE), "bad_reply");
    }

    /// A provider's error text never travels verbatim: the value is dropped whole rather than
    /// filtered, because filtering would keep the digits of a token.
    #[test]
    fn a_providers_error_text_never_travels_verbatim() {
        assert_eq!(sanitize_error("expired_token"), "expired_token");
        assert_eq!(sanitize_error("token AT-7f3c9 rejected"), "refused");
        assert_eq!(sanitize_error("<html>500</html>"), "refused");
        assert_eq!(sanitize_error("AT7f3c9abcdef0123456789"), "refused");
        assert_eq!(sanitize_error(""), "refused");
        assert_eq!(sanitize_error(&"x".repeat(200)), "refused");
    }
}
