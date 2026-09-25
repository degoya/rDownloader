//! Reading a device-flow answer.
//!
//! The two documents are small and flat — five fields between them — so they are scanned
//! rather than parsed. Pulling a JSON library into a sandboxed guest to read five strings
//! would be more code and more surface for no more capability.

/// The client id the provider issued for rDownloader. Public by design in a device flow: it
/// identifies the application, not the person, and cannot authorise anything on its own.
pub const CLIENT_ID: &str = "cvE7ck1s0lRJTfWkPGmyDA";

/// What `POST /oauth/device/code` answered.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_url: String,
    pub expires_in: Option<u64>,
    pub interval: Option<u64>,
}

/// The string value of a JSON field, without a parser.
///
/// Deliberately reads out of the original text rather than a whitespace-stripped copy: the
/// spaces inside a value are part of it, and squeezing them out turns "The PIN has expired"
/// into something nobody wrote.
#[must_use]
pub fn string_field(body: &str, name: &str) -> Option<String> {
    let mut rest = value_after(body, name)?;
    rest = rest.strip_prefix('"')?;
    let mut out = String::new();
    let mut chars = rest.chars();
    while let Some(character) = chars.next() {
        match character {
            '"' => return Some(out),
            '\\' => match chars.next()? {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'r' => out.push('\r'),
                other => out.push(other),
            },
            other => out.push(other),
        }
    }
    None
}

/// The numeric value of a JSON field, without a parser.
#[must_use]
pub fn number_field(body: &str, name: &str) -> Option<u64> {
    let rest = value_after(body, name)?;
    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

/// The text just after `"name":`, with the separating whitespace skipped.
fn value_after<'a>(body: &'a str, name: &str) -> Option<&'a str> {
    let needle = format!("\"{name}\"");
    let mut from = 0;
    while let Some(at) = body[from..].find(&needle) {
        let after = &body[from + at + needle.len()..];
        let trimmed = after.trim_start();
        if let Some(value) = trimmed.strip_prefix(':') {
            return Some(value.trim_start());
        }
        from += at + needle.len();
    }
    None
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

/// The stand-in `poll` reports when the provider's answer had nothing in it this plugin
/// understands. Code-shaped on purpose: it travels the same path as a provider's own error
/// code, through `refusal_code` and `sanitize_error`, and prose would be dropped there.
const UNREADABLE: &str = "unreadable";

/// What a poll means.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PollOutcome {
    /// The person confirmed; the payload is the credential to store.
    Authorized(String),
    /// Not yet. The provider may name a longer interval than it did at the start.
    Pending(Option<u64>),
    /// Over: the code expired, or the person declined.
    Failed(String),
}

/// Reads a token-endpoint answer.
#[must_use]
pub fn poll(body: &str) -> PollOutcome {
    if let Some(token) = string_field(body, "access_token").filter(|token| !token.is_empty()) {
        return PollOutcome::Authorized(token);
    }
    match string_field(body, "error").unwrap_or_default().as_str() {
        // The two the specification defines as "keep waiting". Everything else has ended.
        "authorization_pending" => PollOutcome::Pending(None),
        "slow_down" => PollOutcome::Pending(Some(10)),
        "" => PollOutcome::Failed(UNREADABLE.to_owned()),
        other => PollOutcome::Failed(other.to_owned()),
    }
}

/// The translation code a refusal is reported under, so the interface can say it in the
/// language the person reads. `debridlink_auth` is this plugin's slug; the catalogue in `locales/`
/// has to carry each of these.
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
        DeviceCode, PollOutcome, UNREADABLE, device_code, number_field, poll, refusal_code,
        sanitize_error, string_field,
    };

    const STARTED: &str = r#"{
      "device_code": "abc123",
      "user_code": "WXYZ-1234",
      "verification_url": "https:\/\/debrid-link.com\/webapp\/authorize",
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
                verification_url: "https://debrid-link.com/webapp/authorize".to_owned(),
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
            Some("https://debrid-link.com/webapp/authorize".to_owned())
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
            poll(r#"{"access_token":"tok","token_type":"bearer"}"#),
            PollOutcome::Authorized("tok".to_owned())
        );
        assert_eq!(
            poll(r#"{"error":"authorization_pending"}"#),
            PollOutcome::Pending(None)
        );
        assert_eq!(
            poll(r#"{"error":"slow_down"}"#),
            PollOutcome::Pending(Some(10))
        );
        // Anything else has ended, and going on asking would keep a dead flow alive.
        assert_eq!(
            poll(r#"{"error":"expired_token"}"#),
            PollOutcome::Failed("expired_token".to_owned())
        );
        assert_eq!(
            poll(r#"{"error":"access_denied"}"#),
            PollOutcome::Failed("access_denied".to_owned())
        );
    }

    #[test]
    fn fields_are_read_out_of_pretty_printed_and_compact_documents_alike() {
        assert_eq!(string_field(r#"{"a":"b"}"#, "a").as_deref(), Some("b"));
        assert_eq!(
            string_field("{\n  \"a\" : \"b\"\n}", "a").as_deref(),
            Some("b")
        );
        assert_eq!(number_field(r#"{"n": 42, "m": 1}"#, "n"), Some(42));
        assert_eq!(number_field(r#"{"n":"42"}"#, "n"), None);
    }

    /// The four refusals this plugin can be told apart, so the person is told what to do
    /// rather than always the same thing.
    #[test]
    fn each_refusal_reports_the_code_that_fits_it() {
        assert_eq!(refusal_code("access_denied"), "consent_denied");
        assert_eq!(refusal_code("expired_token"), "flow_expired");
        assert_eq!(refusal_code("invalid_grant"), "sign_in_refused");
        assert_eq!(refusal_code("something_new"), "sign_in_refused");
        // An answer this plugin cannot read says so, rather than claiming the code expired.
        assert_eq!(refusal_code(UNREADABLE), "bad_reply");
    }

    /// A provider's error text never travels verbatim.
    ///
    /// The failure this builds reaches a log line and the interface, so an endpoint that
    /// echoed part of a token into its error document would otherwise publish it. The value
    /// is dropped whole rather than filtered: filtering would keep the digits of a token.
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
