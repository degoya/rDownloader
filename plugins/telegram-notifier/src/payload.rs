//! Shaping one message for the Telegram Bot API.
//!
//! Telegram is the destination that needs two values rather than one: a bot token, which is a
//! secret and lives in the vault, and a chat id, which is not and is configured in plain
//! sight. That split is why this plugin exists separately from the other two.

/// Telegram refuses a message over 4096 characters outright.
const MAX_TEXT: usize = 3800;

/// The address `sendMessage` is called at. The bot token is part of the path — Telegram has
/// no other way — so it stays in the vault and reaches the address as the reference-less
/// secret marker, which the host substitutes after checking the host name.
pub const ENDPOINT: &str = "https://api.telegram.org/bot{{secret}}/sendMessage";

/// The chat id, as configured. Telegram accepts a numeric id, a negative group id or an
/// `@channelusername`; all three are what people have in front of them, so all three pass.
///
/// Returns `None` for anything else rather than sending it: a chat id with a space or a
/// slash in it is a mistake somebody made, and Telegram's answer to it says nothing useful.
#[must_use]
pub fn chat_id(destination: &str) -> Option<&str> {
    let trimmed = destination.trim();
    if trimmed.is_empty() {
        return None;
    }
    let body = trimmed.strip_prefix('@').unwrap_or(trimmed);
    let numeric = body
        .strip_prefix('-')
        .unwrap_or(body)
        .chars()
        .all(|character| character.is_ascii_digit());
    let username = trimmed.starts_with('@')
        && body
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_');
    (numeric && !body.is_empty() || username).then_some(trimmed)
}

/// The message text, as HTML: a bold title and the body below it.
///
/// HTML rather than Markdown because Telegram's Markdown dialects each reserve a different
/// set of characters, and a file name that trips one of them is rejected with a parse error.
/// Escaping three characters for HTML is something this file can be sure it got right.
#[must_use]
pub fn text(title: &str, body: &str, severity: &str) -> String {
    let marker = match severity {
        "error" => "\u{26d4} ",
        "warning" => "\u{26a0} ",
        _ => "",
    };
    let text = format!("{marker}<b>{}</b>\n{}", escape(title), escape(body));
    truncate(&text, MAX_TEXT)
}

/// Escapes the three characters Telegram's HTML mode reserves.
#[must_use]
pub fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn truncate(value: &str, limit: usize) -> String {
    if value.chars().count() <= limit {
        return value.to_owned();
    }
    value.chars().take(limit).collect()
}

#[cfg(test)]
mod tests {
    use super::{ENDPOINT, chat_id, escape, text};

    #[test]
    fn the_bot_token_never_appears_in_the_plugin() {
        assert!(ENDPOINT.contains("{{secret}}"));
        assert!(ENDPOINT.starts_with("https://api.telegram.org/"));
    }

    #[test]
    fn the_three_shapes_of_a_chat_id_are_accepted() {
        assert_eq!(chat_id("123456"), Some("123456"));
        assert_eq!(chat_id("-1001234567890"), Some("-1001234567890"));
        assert_eq!(chat_id("@downloads"), Some("@downloads"));
        assert_eq!(chat_id(" 123456 "), Some("123456"));
    }

    #[test]
    fn anything_else_is_refused_rather_than_sent() {
        // Telegram's answer to a malformed chat id says nothing a person could act on, so
        // the mistake is named here instead.
        assert_eq!(chat_id(""), None);
        assert_eq!(chat_id("chat 42"), None);
        assert_eq!(chat_id("../../etc"), None);
        assert_eq!(chat_id("@bad name"), None);
    }

    #[test]
    fn html_is_escaped_so_a_file_name_cannot_become_markup() {
        assert_eq!(escape("a <b> & c"), "a &lt;b&gt; &amp; c");
        let rendered = text("<script>", "done & dusted", "info");
        assert!(rendered.contains("&lt;script&gt;"), "{rendered}");
        assert!(rendered.contains("done &amp; dusted"), "{rendered}");
    }

    #[test]
    fn an_overlong_message_is_cut_below_telegrams_limit() {
        let long = "x".repeat(9000);
        assert!(text("t", &long, "info").chars().count() <= 3800);
    }
}
