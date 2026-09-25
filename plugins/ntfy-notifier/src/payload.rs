//! Shaping one message for ntfy.
//!
//! ntfy takes the body as the request body and everything else as a parameter, which it reads
//! from a header or, under its lowercase name, from the query string. The query is what is
//! used: the host sends only the headers on its allowlist (RD-120-60). So there is no JSON to
//! build here — only a small amount of care about what goes into a parameter.

/// Longest parameter value sent. ntfy truncates far above this; the limit exists so a package
/// name that is really a paragraph cannot produce a request the server refuses outright.
const MAX_FIELD: usize = 512;

/// Longest message body sent.
const MAX_BODY: usize = 4096;

/// ntfy's priority scale, 1 (min) to 5 (max). Only three of the five are used: a download
/// manager has nothing that deserves "max", which on a phone overrides do-not-disturb.
#[must_use]
pub fn priority(severity: &str) -> &'static str {
    match severity {
        "error" => "4",
        "warning" => "3",
        _ => "2",
    }
}

/// A parameter value ntfy will accept: single-line and bounded.
///
/// A title arrives here carrying a package name somebody else chose, and a line break has no
/// place in a notification title. Non-ASCII stays: the query string is percent-encoded, which
/// ntfy decodes as UTF-8 — the reason it used to be dropped was the header it travelled in.
#[must_use]
pub fn field_value(value: &str) -> String {
    let cleaned: String = value
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect();
    truncate(cleaned.trim(), MAX_FIELD)
}

/// The message body, bounded.
#[must_use]
pub fn body(value: &str) -> String {
    truncate(value, MAX_BODY)
}

/// The topic URL to post to.
///
/// A destination may be configured as a full URL — a self-hosted server's topic, too — or as
/// a bare topic name on `ntfy.sh`; both are what people have in front of them, so both are
/// accepted. Which of the two it is, is read exactly as the host reads it: the host narrows
/// the delivery to the host of an address and to `ntfy.sh` for anything else (RD-130-15), so a
/// disagreement here would send the request somewhere it may not go.
#[must_use]
pub fn endpoint(destination: &str) -> String {
    let destination = destination.trim();
    let written_as_address = ["https://", "http://"].iter().any(|scheme| {
        destination
            .get(..scheme.len())
            .is_some_and(|start| start.eq_ignore_ascii_case(scheme))
    });
    if written_as_address {
        return destination.to_owned();
    }
    format!("https://ntfy.sh/{}", destination.trim_start_matches('/'))
}

fn truncate(value: &str, limit: usize) -> String {
    if value.len() <= limit {
        return value.to_owned();
    }
    let mut end = limit;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}

#[cfg(test)]
mod tests {
    use super::{body, endpoint, field_value, priority};

    #[test]
    fn a_bare_topic_becomes_a_full_address() {
        assert_eq!(endpoint("downloads"), "https://ntfy.sh/downloads");
        assert_eq!(endpoint("/downloads"), "https://ntfy.sh/downloads");
        assert_eq!(
            endpoint("https://ntfy.sh/downloads"),
            "https://ntfy.sh/downloads"
        );
    }

    #[test]
    fn a_self_hosted_address_is_used_as_written() {
        assert_eq!(
            endpoint(" https://ntfy.example.org:8443/alerts "),
            "https://ntfy.example.org:8443/alerts"
        );
        assert_eq!(endpoint("http://ntfy.lan/alerts"), "http://ntfy.lan/alerts");
        // The scheme is read case-insensitively, as the host reads it.
        assert_eq!(
            endpoint("HTTPS://ntfy.example.org/alerts"),
            "HTTPS://ntfy.example.org/alerts"
        );
    }

    #[test]
    fn a_field_value_stays_on_one_line() {
        // The title carries a package name somebody else chose; a line break in it is
        // flattened rather than sent.
        assert_eq!(
            field_value("Holiday\r\nX-Injected: yes"),
            "Holiday  X-Injected: yes"
        );
    }

    #[test]
    fn non_ascii_is_kept_now_that_it_travels_percent_encoded() {
        // A header is ASCII, the query string is not limited to it: the title keeps its accent.
        assert_eq!(field_value("Caf\u{e9} release"), "Caf\u{e9} release");
        assert_eq!(body("Caf\u{e9} release"), "Caf\u{e9} release");
    }

    #[test]
    fn a_long_value_is_cut_on_a_character_boundary() {
        // Two bytes per character, so the cut lands mid-character unless it is guarded.
        let long = "\u{e9}".repeat(4000);
        let cut = body(&long);
        assert!(cut.len() <= 4096);
        assert!(cut.chars().all(|character| character == '\u{e9}'));
    }

    #[test]
    fn severity_maps_onto_ntfys_own_scale() {
        assert_eq!(priority("error"), "4");
        assert_eq!(priority("warning"), "3");
        assert_eq!(priority("info"), "2");
        assert_eq!(priority("something else"), "2");
    }
}
