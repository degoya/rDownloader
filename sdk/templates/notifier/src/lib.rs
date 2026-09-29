//! A scaffold notification destination. It compiles, packages and passes conformance as it is.
//!
//! It posts each message as JSON to a channel of a made-up chat service,
//! `https://api.example.com/v1/channels/<channel>/messages`; replace [`API`] and the body with
//! your service's.
//!
//! Two things the host guarantees, which shape how this is written:
//!
//! - **Retrying is not your decision.** Report a failure and say what kind it is; the delivery
//!   hub applies its own backoff and quiet hours, the same ones the built-in webhook uses.
//! - **A secret reaches your request without reaching you.** Put `{{secret}}` in a header, a
//!   query value or the body and the host substitutes the destination's token on the way out.
//!   Only `authorization`, `content-type`, `accept` and a few others are sent at all; anything
//!   else goes in the query or the body.
//!
//! The layout follows one practical concern: everything that can be tested without a
//! WebAssembly toolchain lives here, outside the component. `cargo test` in a fresh scaffold
//! runs it on the host target; `guest` exists only on `wasm32`.

#[cfg(target_arch = "wasm32")]
mod guest;

/// The service's API. Its host must be in `[capabilities.net_http] domains`.
pub const API: &str = "https://api.example.com/v1/channels/";

/// What a delivery attempt came to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Delivery {
    Delivered,
    /// Worth another attempt later — the service was down or asked us to slow down.
    Retry,
    /// The token was refused. Trying again will not change that; the person has to.
    Unauthorized,
    /// Anything else the service refused. Trying again will not change it either.
    Refused,
}

/// The address to post to, or `None` when the configured channel is not a plain name.
///
/// `destination` is whatever the person typed. Pasted into a path unchecked, `../admin` or a
/// `?` would send the request somewhere the person never configured.
#[must_use]
pub fn endpoint(destination: &str) -> Option<String> {
    let channel = destination.trim();
    let plain = !channel.is_empty()
        && channel.len() <= 64
        && channel
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    plain.then(|| format!("{API}{channel}/messages"))
}

/// The JSON body for one message.
///
/// `dedupe` carries the idempotency key: a delivery that failed while the answer was on its way
/// is tried again, and a service that deduplicates turns at-least-once into exactly-once.
#[must_use]
pub fn body(title: &str, text: &str, severity: &str, idempotency_key: &str) -> String {
    format!(
        "{{\"title\":{},\"text\":{},\"level\":{},\"dedupe\":{}}}",
        json_string(title),
        json_string(text),
        json_string(level(severity)),
        json_string(idempotency_key),
    )
}

/// The service's level for rDownloader's severity.
#[must_use]
pub fn level(severity: &str) -> &'static str {
    match severity {
        "error" => "high",
        "warning" => "normal",
        _ => "low",
    }
}

/// How the host should treat the service's answer.
#[must_use]
pub fn delivery(status: u16) -> Delivery {
    match status {
        200..=299 => Delivery::Delivered,
        401 | 403 => Delivery::Unauthorized,
        // 4xx will not become anything else by repeating it; 5xx and 429 might.
        429 | 500..=599 => Delivery::Retry,
        _ => Delivery::Refused,
    }
}

/// A JSON string literal. Titles carry package names somebody else chose, quotes included.
fn json_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            control if control.is_control() => out.push_str(&format!("\\u{:04x}", control as u32)),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::{Delivery, body, delivery, endpoint};

    #[test]
    fn a_channel_is_a_plain_name_and_nothing_else() {
        assert_eq!(
            endpoint(" downloads "),
            Some("https://api.example.com/v1/channels/downloads/messages".to_owned())
        );
        assert_eq!(endpoint("../admin"), None);
        assert_eq!(endpoint("a?b=c"), None);
        assert_eq!(endpoint(""), None);
    }

    #[test]
    fn the_body_is_valid_json_whatever_the_title_holds() {
        assert_eq!(
            body("Say \"hi\"\n", "done", "error", "key-1"),
            r#"{"title":"Say \"hi\"\n","text":"done","level":"high","dedupe":"key-1"}"#
        );
        assert!(body("\u{7}", "", "info", "k").contains(r#""title":"\u0007""#));
    }

    #[test]
    fn only_a_temporary_refusal_is_worth_retrying() {
        assert_eq!(delivery(204), Delivery::Delivered);
        assert_eq!(delivery(503), Delivery::Retry);
        assert_eq!(delivery(429), Delivery::Retry);
        assert_eq!(delivery(401), Delivery::Unauthorized);
        assert_eq!(delivery(400), Delivery::Refused);
    }
}
