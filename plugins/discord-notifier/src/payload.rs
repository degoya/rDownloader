//! Shaping one message for a Discord webhook.
//!
//! Discord takes JSON, so this file is mostly about two things: not exceeding the limits the
//! API enforces, and not letting a package name somebody else chose turn into markup.

/// Discord's own limits. `content` is capped at 2000 characters and an embed title at 256;
/// exceeding either is a 400, which would otherwise look like a broken destination.
const MAX_CONTENT: usize = 1900;
const MAX_TITLE: usize = 240;

/// The address a webhook destination posts to.
///
/// The token is the tail of the path, so it stays in the vault and reaches the address as the
/// reference-less secret marker: the host substitutes it after checking the host name and
/// checks again afterwards that the host did not change.
pub const ENDPOINT: &str = "https://discord.com/api/webhooks/{{secret}}";

/// The colour stripe on the embed, by severity. Discord takes a decimal integer.
#[must_use]
pub fn colour(severity: &str) -> u32 {
    match severity {
        "error" => 0x00e0_5252,
        "warning" => 0x00e0_a352,
        _ => 0x0052_a0e0,
    }
}

/// The JSON body of one webhook message.
///
/// Built by hand rather than with a serialiser: the document has four fields and pulling a
/// JSON crate into a sandboxed guest to write them would be more code, not less.
#[must_use]
pub fn body(title: &str, body_text: &str, event: &str, severity: &str) -> String {
    format!(
        r#"{{"embeds":[{{"title":"{}","description":"{}","color":{},"footer":{{"text":"{}"}}}}]}}"#,
        escape(&truncate(&plain(title), MAX_TITLE)),
        escape(&truncate(&plain(body_text), MAX_CONTENT)),
        colour(severity),
        escape(&truncate(&plain(event), MAX_TITLE)),
    )
}

/// Neutralises Discord's markdown so a file name cannot become formatting — or a link.
///
/// A package name is written by whoever produced the release, not by the person reading the
/// notification, so it is not text this plugin gets to trust.
#[must_use]
pub fn plain(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for character in value.chars() {
        if matches!(character, '*' | '_' | '~' | '`' | '|' | '>' | '#' | '@') {
            out.push('\\');
        }
        out.push(if character.is_control() {
            ' '
        } else {
            character
        });
    }
    out
}

/// Escapes a Rust string into a JSON string body.
#[must_use]
pub fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 8);
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            control if control.is_control() => {
                out.push_str(&format!("\\u{:04x}", control as u32));
            }
            other => out.push(other),
        }
    }
    out
}

fn truncate(value: &str, limit: usize) -> String {
    if value.chars().count() <= limit {
        return value.to_owned();
    }
    value.chars().take(limit).collect()
}

#[cfg(test)]
mod tests {
    use super::{ENDPOINT, body, colour, escape, plain};

    #[test]
    fn the_token_never_appears_in_the_plugin() {
        // The address carries a marker, not a value: the host substitutes it on the way out.
        assert!(ENDPOINT.contains("{{secret}}"));
        assert!(ENDPOINT.starts_with("https://discord.com/"));
    }

    #[test]
    fn markdown_in_a_package_name_is_neutralised() {
        // A release name is written by somebody else. Left alone it would become formatting,
        // and `@everyone` would become a mention of everybody in the channel.
        assert_eq!(plain("*bold* @everyone"), "\\*bold\\* \\@everyone");
    }

    #[test]
    fn a_quote_in_a_title_cannot_break_the_document() {
        let json = body("say \"hi\"", "done", "package.completed", "info");
        assert!(json.contains(r#"say \"hi\""#), "{json}");
        assert!(json.starts_with(r#"{"embeds":"#));
    }

    #[test]
    fn a_control_character_is_escaped_rather_than_sent() {
        assert_eq!(escape("a\u{1}b"), "a\\u0001b");
        assert_eq!(escape("line\nbreak"), "line\\nbreak");
    }

    #[test]
    fn severity_picks_the_stripe_colour() {
        assert_ne!(colour("error"), colour("info"));
        assert_eq!(colour("anything else"), colour("info"));
    }

    #[test]
    fn an_overlong_body_is_cut_to_what_discord_accepts() {
        let long = "x".repeat(5000);
        let json = body("t", &long, "e", "info");
        assert!(json.len() < 2600, "{}", json.len());
    }
}
