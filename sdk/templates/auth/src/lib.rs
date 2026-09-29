//! A scaffold authentication provider. It compiles, packages and passes conformance as it is.
//!
//! It runs a PIN sign-in, the flow several debrid services offer: ask the provider for a PIN,
//! show the person where to confirm it, then ask until the provider hands over an API key.
//! Two requests against a made-up API:
//!
//! | Request | Answer |
//! | --- | --- |
//! | `GET https://api.example.com/v1/pin/get` | `{"pin": "ABCD", "check": "<secret>", "user_url": "https://example.com/pin?pin=ABCD", "expires_in": 600}` |
//! | `GET https://api.example.com/v1/pin/check?pin=&check=` | `{"activated": false}`, then `{"activated": true, "apikey": "..."}`; `{"error": "expired"}` once too late |
//!
//! Three things the host guarantees, which shape how this is written:
//!
//! - **You never see a stored credential.** `credential-ref` is an opaque handle, and the key
//!   you obtain goes back through `credentials.store-token`, which writes it where the
//!   account's provider keeps it. There is no call that reads one back.
//! - **The account you are handed is the only one you can write to.** Naming another is
//!   refused, so a flow cannot reach past the account it was started for.
//! - **You remember nothing between calls.** Whatever `poll` needs from `begin` goes back in
//!   `flow-state`, and the host hands it over verbatim on the next poll. That is what lets a
//!   sign-in survive a closed browser or a service restart.
//!
//! The layout follows one practical concern: everything that can be tested without a
//! WebAssembly toolchain lives here, outside the component. `cargo test` in a fresh scaffold
//! runs it on the host target; `guest` exists only on `wasm32`.

#[cfg(target_arch = "wasm32")]
mod guest;

/// Where a PIN is asked for.
pub const PIN_ENDPOINT: &str = "https://api.example.com/v1/pin/get";
/// Where the provider is asked whether the PIN was confirmed.
pub const CHECK_ENDPOINT: &str = "https://api.example.com/v1/pin/check";

/// What the provider answered to the PIN request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Pin {
    /// Shown to the person.
    pub pin: String,
    /// Proves the poll comes from whoever asked for the PIN. Never shown.
    pub check: String,
    pub user_url: String,
    pub expires_in: Option<u64>,
}

impl Pin {
    /// What `begin` hands the host for the next poll: the PIN and its check value.
    #[must_use]
    pub fn flow_state(&self) -> String {
        format!("{} {}", self.pin, self.check)
    }
}

/// The PIN and check value back out of `flow-state`.
#[must_use]
pub fn from_flow_state(state: &str) -> Option<(&str, &str)> {
    let (pin, check) = state.split_once(' ')?;
    (!pin.is_empty() && !check.is_empty()).then_some((pin, check))
}

/// What a poll came to.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Check {
    /// Confirmed: this is the API key to store.
    Activated(String),
    /// Not confirmed yet. Waiting, not failing.
    Waiting,
    /// The PIN ran out before anybody confirmed it.
    Expired,
    /// Nothing this plugin can read.
    Unreadable,
}

/// Reads the answer to the PIN request.
#[must_use]
pub fn read_pin(body: &str) -> Option<Pin> {
    Some(Pin {
        pin: string_field(body, "pin")?,
        check: string_field(body, "check")?,
        user_url: string_field(body, "user_url").filter(|url| url.starts_with("https://"))?,
        expires_in: number_field(body, "expires_in"),
    })
}

/// Reads the answer to one poll.
#[must_use]
pub fn read_check(body: &str) -> Check {
    if string_field(body, "error").as_deref() == Some("expired") {
        return Check::Expired;
    }
    match after_field(body, "activated") {
        Some(value) if value.starts_with("true") => string_field(body, "apikey")
            .filter(|key| !key.is_empty())
            .map_or(Check::Unreadable, Check::Activated),
        Some(value) if value.starts_with("false") => Check::Waiting,
        _ => Check::Unreadable,
    }
}

/// The string value of a flat JSON field. Enough for answers this small; escapes are not
/// decoded because nothing read here carries one.
fn string_field(body: &str, name: &str) -> Option<String> {
    let (value, _) = after_field(body, name)?
        .strip_prefix('"')?
        .split_once('"')?;
    Some(value.to_owned())
}

fn number_field(body: &str, name: &str) -> Option<u64> {
    let value = after_field(body, name)?;
    let end = value
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(value.len());
    value[..end].parse().ok()
}

fn after_field<'a>(body: &'a str, name: &str) -> Option<&'a str> {
    let needle = format!("\"{name}\"");
    let rest = &body[body.find(&needle)? + needle.len()..];
    Some(rest.trim_start().strip_prefix(':')?.trim_start())
}

#[cfg(test)]
mod tests {
    use super::{Check, from_flow_state, read_check, read_pin};

    #[test]
    fn a_pin_answer_becomes_a_prompt_and_a_flow_state() {
        let pin = read_pin(
            r#"{"pin": "ABCD", "check": "c0ffee", "user_url": "https://example.com/pin?pin=ABCD", "expires_in": 600}"#,
        )
        .expect("a pin");
        assert_eq!(pin.pin, "ABCD");
        assert_eq!(pin.expires_in, Some(600));
        assert_eq!(from_flow_state(&pin.flow_state()), Some(("ABCD", "c0ffee")));
    }

    #[test]
    fn a_pin_without_its_check_value_or_on_plain_http_is_no_pin() {
        assert_eq!(
            read_pin(r#"{"pin": "ABCD", "user_url": "https://example.com/pin"}"#),
            None
        );
        assert_eq!(
            read_pin(r#"{"pin": "A", "check": "c", "user_url": "http://example.com/pin"}"#),
            None
        );
        assert_eq!(from_flow_state(""), None);
    }

    #[test]
    fn a_poll_waits_until_the_key_arrives() {
        assert_eq!(read_check(r#"{"activated": false}"#), Check::Waiting);
        assert_eq!(
            read_check(r#"{"activated": true, "apikey": "k-1"}"#),
            Check::Activated("k-1".to_owned())
        );
        assert_eq!(read_check(r#"{"error": "expired"}"#), Check::Expired);
        // Confirmed without a key is not a sign-in.
        assert_eq!(read_check(r#"{"activated": true}"#), Check::Unreadable);
        assert_eq!(read_check("<html>"), Check::Unreadable);
    }
}
