//! Reading AllDebrid's PIN-flow answers.
//!
//! Two small documents, scanned rather than parsed: pulling a JSON library into a sandboxed
//! guest to read four strings would be more code and more surface for no more capability.

/// The application name AllDebrid's API asks for. It identifies the application, not the
/// person, and authorises nothing on its own.
pub const AGENT: &str = "rDownloader";

/// What `/pin/get` answered.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Pin {
    /// Shown to the person, who types it at the address below.
    pub pin: String,
    /// The address to visit. AllDebrid returns one that already carries the PIN.
    pub user_url: String,
    /// The opaque value the application polls with. Not shown to anybody.
    pub check: String,
    pub expires_in: Option<u64>,
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

/// Whether the answer reports failure, and under which code.
///
/// The `code` first and the free-text `message` only as a fallback (RD-106-01). It used to be
/// the other way round, and the message is the one field AllDebrid writes as prose — so the
/// prose was what travelled into the log line and the interface. `sanitize_error` drops
/// anything that is not code-shaped, which means reading the message first would have thrown
/// away the one part of the answer worth keeping.
#[must_use]
pub fn error(body: &str) -> Option<String> {
    (body.contains("\"status\"") && body.contains("\"error\""))
        .then(|| string_field(body, "code").or_else(|| string_field(body, "message")))
        .flatten()
}

/// Reads a `/pin/get` answer, or `None` when it is not one.
#[must_use]
pub fn pin(body: &str) -> Option<Pin> {
    let pin = string_field(body, "pin")?;
    let user_url = string_field(body, "user_url")?;
    let check = string_field(body, "check")?;
    (!pin.is_empty() && !user_url.is_empty() && !check.is_empty()).then(|| Pin {
        pin,
        user_url,
        check,
        expires_in: number_field(body, "expires_in"),
    })
}

/// The stand-in `poll` reports when the provider's answer had nothing in it this plugin
/// understands. Code-shaped on purpose: it travels the same path as a provider's own error
/// code, through `refusal_code` and `sanitize_error`, and prose would be dropped there.
const UNREADABLE: &str = "unreadable";

/// What a `/pin/check` answer means.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PollOutcome {
    /// The person confirmed; the payload is the credential to store.
    Authorized(String),
    /// Not yet.
    Pending,
    Failed(String),
}

/// Reads a `/pin/check` answer.
#[must_use]
pub fn poll(body: &str) -> PollOutcome {
    if let Some(message) = error(body) {
        return PollOutcome::Failed(message);
    }
    if let Some(key) = string_field(body, "apikey").filter(|key| !key.is_empty()) {
        return PollOutcome::Authorized(key);
    }
    // `activated: false` is the ordinary "not yet"; an answer with neither is one this plugin
    // does not understand, and treating that as "keep waiting" would hang the flow forever.
    if body.contains("\"activated\"") {
        return PollOutcome::Pending;
    }
    PollOutcome::Failed(UNREADABLE.to_owned())
}

/// The translation code a refusal is reported under, so the interface can say it in the
/// language the person reads. `alldebrid_auth` is this plugin's slug; the catalogue in
/// `locales/` has to carry each of these.
///
/// Every refusal used to be reported as `flow_expired`, whatever it was — so a person whose
/// account was blocked was told their PIN had expired and went round again (RD-106-01).
#[must_use]
pub fn refusal_code(error: &str) -> &'static str {
    match error {
        "PIN_EXPIRED" | "PIN_INVALID" => "flow_expired",
        "AUTH_BLOCKED" | "AUTH_USER_BANNED" => "consent_denied",
        UNREADABLE => "bad_reply",
        _ => "sign_in_refused",
    }
}

/// A provider's error code, reduced to something that is safe to put in a message.
///
/// The point is not tidiness. Whatever a provider sends back travels into a log line and into
/// the failure the interface shows, and an endpoint that echoed part of an API key into its
/// error document would otherwise publish it. AllDebrid's codes are words joined by
/// underscores, so anything that is not exactly that shape is dropped whole rather than
/// filtered character by character — filtering would keep the digits of a leaked key, and a
/// digit is what a key is mostly made of. Hence letters and underscores only, and the result
/// lowercased so a message reads as a message.
#[must_use]
pub fn sanitize_error(error: &str) -> String {
    let trimmed = error.trim();
    let is_error_code = !trimmed.is_empty()
        && trimmed.len() <= 40
        && trimmed.chars().all(|c| c.is_ascii_alphabetic() || c == '_');
    if is_error_code {
        trimmed.to_ascii_lowercase()
    } else {
        "refused".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::{Pin, PollOutcome, UNREADABLE, error, pin, poll, refusal_code, sanitize_error};

    const STARTED: &str = r#"{"status":"success","data":{
      "pin":"ABCD","check":"opaque-check-value",
      "expires_in":600,
      "user_url":"https:\/\/api.alldebrid.com\/pin\/?pin=ABCD"}}"#;

    #[test]
    fn a_pin_answer_is_read_whole() {
        assert_eq!(
            pin(STARTED),
            Some(Pin {
                pin: "ABCD".to_owned(),
                user_url: "https://api.alldebrid.com/pin/?pin=ABCD".to_owned(),
                check: "opaque-check-value".to_owned(),
                expires_in: Some(600),
            })
        );
    }

    #[test]
    fn a_confirmed_pin_yields_the_credential() {
        let body = r#"{"status":"success","data":{"activated":true,"apikey":"the-key"}}"#;
        assert_eq!(poll(body), PollOutcome::Authorized("the-key".to_owned()));
    }

    #[test]
    fn an_unconfirmed_pin_keeps_the_flow_open() {
        let body = r#"{"status":"success","data":{"activated":false,"expires_in":540}}"#;
        assert_eq!(poll(body), PollOutcome::Pending);
    }

    /// The code, not the prose: the code is the part this plugin can classify and the part
    /// that is safe to repeat.
    #[test]
    fn a_reported_error_ends_the_flow_and_says_why() {
        let body =
            r#"{"status":"error","error":{"code":"PIN_EXPIRED","message":"The PIN has expired"}}"#;
        assert_eq!(error(body).as_deref(), Some("PIN_EXPIRED"));
        assert_eq!(poll(body), PollOutcome::Failed("PIN_EXPIRED".to_owned()));
        assert_eq!(refusal_code("PIN_EXPIRED"), "flow_expired");
        assert_eq!(sanitize_error("PIN_EXPIRED"), "pin_expired");
    }

    /// A blocked account is not an expired PIN, and telling somebody to try the code again
    /// is the one answer that cannot help them.
    #[test]
    fn a_blocked_account_is_not_reported_as_an_expired_pin() {
        assert_eq!(refusal_code("AUTH_BLOCKED"), "consent_denied");
        assert_eq!(refusal_code("NO_SERVER"), "sign_in_refused");
    }

    /// A provider's error text never travels verbatim.
    ///
    /// AllDebrid's `message` is free prose and used to be quoted into the failure the
    /// interface shows. Anything that is not code-shaped is now dropped whole rather than
    /// filtered: filtering would keep the digits of a leaked API key.
    #[test]
    fn a_providers_error_text_never_travels_verbatim() {
        assert_eq!(sanitize_error("PIN_EXPIRED"), "pin_expired");
        assert_eq!(sanitize_error("The PIN has expired"), "refused");
        assert_eq!(sanitize_error("key 7f3c9ab2 rejected"), "refused");
        assert_eq!(sanitize_error("AT7f3c9abcdef0123456789"), "refused");
        assert_eq!(sanitize_error(""), "refused");
        assert_eq!(sanitize_error(&"x".repeat(200)), "refused");
    }

    #[test]
    fn an_answer_this_plugin_does_not_understand_is_not_treated_as_waiting() {
        // Treating an unknown shape as "keep waiting" would leave a flow polling for ever.
        assert_eq!(poll("{}"), PollOutcome::Failed(UNREADABLE.to_owned()));
        // And it says so as `bad_reply`, not as "your PIN expired".
        assert_eq!(refusal_code(UNREADABLE), "bad_reply");
        assert_eq!(pin("{}"), None);
    }
}
