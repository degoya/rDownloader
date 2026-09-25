//! The trace a session page nobody recognizes leaves behind (RD-120-46).
//!
//! When the session probe answers with a page that is neither signed in nor a guest page, the
//! check cannot tell what it saw — and that page is the only evidence from which the unmeasured
//! signed-in marker could ever be measured. Until RD-120-46 it was dropped without a word. So it
//! is now described in one log line: its title, its length and which of a fixed list of
//! markers it carries.
//!
//! **Never the body itself.** An account page carries the address, the balance and, on
//! DDownload, the API key in a form field. The markers are reported under their own names,
//! which are constants in this file, so the one thing that travels off the wire is the title,
//! bounded and stripped of control characters.
//!
//! The same file lives in `plugins/ddownload`, `plugins/katfile` and `plugins/filejoker`. Its
//! natural home is `xfs-common`, and putting it there would have staled every plugin that crate
//! serves for a helper three of them use.

/// Longest title the line carries.
const MAX_TITLE_CHARS: usize = 80;

/// What the line looks for, reported under these very strings. Case-insensitive and over the
/// whole body, scripts included: a sign-out link a script draws is exactly what the verdict
/// misses and what a measurement needs to know about.
const MARKERS: &[&str] = &[
    "logout",
    "log out",
    "sign out",
    "op=my_account",
    "my account",
    "op=login",
    "/login",
    "type=\"password\"",
    "premium",
    "api key",
    "cf-wrapper",
    "challenge-platform",
    "turnstile",
    "g-recaptcha",
    "h-captcha",
];

/// The one line an unrecognized session page leaves:
/// `{provider}: {page} settled nothing about the session - title "…", N bytes, markers: …`.
///
/// `provider` and `page` are the caller's constants, never values off the wire.
#[must_use]
pub(crate) fn unconfirmed_page_line(provider: &str, page: &str, html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let found: Vec<&str> = MARKERS
        .iter()
        .copied()
        .filter(|marker| lower.contains(marker))
        .collect();
    let markers = if found.is_empty() {
        "none".to_owned()
    } else {
        found.join(", ")
    };
    let title = match title(html) {
        Some(title) => format!("title \"{title}\""),
        None => "no title".to_owned(),
    };
    format!(
        "{provider}: {page} settled nothing about the session - {title}, {} bytes, markers: \
         {markers}",
        html.len()
    )
}

/// The page's `<title>`, whitespace collapsed, control characters dropped and bounded.
fn title(html: &str) -> Option<String> {
    // ASCII lowercasing keeps every byte offset, so positions found in `lower` index `html`.
    let lower = html.to_ascii_lowercase();
    let open = lower.find("<title")?;
    let start = open + lower[open..].find('>')? + 1;
    let end = start + lower[start..].find("</title")?;
    let text: String = html[start..end]
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .filter(|character| !character.is_control())
        .take(MAX_TITLE_CHARS)
        .collect();
    (!text.is_empty()).then_some(text)
}

#[cfg(test)]
#[path = "session_trace_tests.rs"]
mod tests;
