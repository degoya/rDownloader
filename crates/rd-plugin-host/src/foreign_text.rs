//! Text somebody else wrote, made unable to spell a vault marker (RD-120-65).
//!
//! The host expands `{{secret}}`, `{{secret:…}}`, `{{basic:…}}`, `{{username}}` and
//! `{{client_id}}` wherever a plugin's request carries them: query values, header values, the
//! body, and — for the granted secret, in either spelling — the address. It cannot tell a marker
//! the plugin wrote from one that arrived inside a package name the plugin copied into the same
//! value, and a notification is made of exactly such names: a release title from an indexer
//! feed, a file name off a hoster's page. A release called `Release {{secret}}` was therefore
//! sent to Telegram with the bot token in its place.
//!
//! So the host takes the braces out of that text **before the plugin sees it**, in the one
//! place every notifier's input passes, rather than asking each plugin to escape it: a plugin
//! that forgot would leak, and the next one written would be the one that forgot. Every `{`
//! becomes U+2774 (`❴`, MEDIUM LEFT CURLY BRACKET ORNAMENT), which reads as a brace.
//!
//! Every one, not only the second of a pair, because plugins change text on its way: the ntfy
//! plugin used to drop non-ASCII, and `{é{secret}}` or `{{ {secret}}` would have come out of such
//! a filter as a marker although neither holds `{{`. With no ASCII `{` left, no dropping,
//! trimming or flattening can make one. The replacement is chosen for the same reason: it is not
//! ASCII, has no compatibility decomposition (NFKC would turn the full-width `｛` back into `{`)
//! and is neither whitespace nor a control character.
//!
//! `%7B`, in either case, goes the same way: the granted marker is also recognised in its
//! percent-encoded spelling `%7B%7Bsecret%7D%7D`, which is how it survives in an address. What
//! stays possible is a plugin that *decodes* foreign text — HTML entities, percent-encoding —
//! into a marker after it arrived; none of the bundled ones does, and that is a plugin defect
//! this cannot see.

use std::borrow::Cow;

/// What every `{` of foreign text becomes.
pub(crate) const INERT_BRACE: char = '\u{2774}';

/// `text` with every brace, plain or percent-encoded, made inert; borrowed when there was none.
pub(crate) fn inert(text: &str) -> Cow<'_, str> {
    if !text.contains('{') && !text.to_ascii_uppercase().contains("%7B") {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len() + 8);
    let mut rest = text;
    while let Some(character) = rest.chars().next() {
        if character == '{' {
            out.push(INERT_BRACE);
            rest = &rest[1..];
        } else if rest
            .get(..3)
            .is_some_and(|head| head.eq_ignore_ascii_case("%7B"))
        {
            out.push(INERT_BRACE);
            rest = &rest[3..];
        } else {
            out.push(character);
            rest = &rest[character.len_utf8()..];
        }
    }
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::inert;

    #[test]
    fn every_brace_becomes_inert() {
        assert_eq!(
            inert("Release {{secret}}"),
            "Release \u{2774}\u{2774}secret}}"
        );
        assert_eq!(
            inert("{{secret:tg}} {{basic:tg}} {{username}} {{client_id}}"),
            "\u{2774}\u{2774}secret:tg}} \u{2774}\u{2774}basic:tg}} \u{2774}\u{2774}username}} \
             \u{2774}\u{2774}client_id}}"
        );
        assert_eq!(inert("{single}"), "\u{2774}single}");
    }

    #[test]
    fn the_percent_encoded_brace_becomes_inert_too() {
        assert_eq!(inert("%7B%7Bsecret%7D%7D"), "\u{2774}\u{2774}secret%7D%7D");
        assert_eq!(inert("%7b%7Bsecret%7D%7D"), "\u{2774}\u{2774}secret%7D%7D");
    }

    #[test]
    fn text_without_an_opening_is_left_alone() {
        for text in [
            "",
            "example.iso",
            "closing} only}",
            "50% off",
            "Caf\u{e9} 100%",
        ] {
            assert!(
                matches!(inert(text), std::borrow::Cow::Borrowed(_)),
                "{text}"
            );
            assert_eq!(inert(text), text);
        }
    }

    #[test]
    fn what_plugins_do_to_text_does_not_bring_a_marker_back() {
        // Neither of the first two holds `{{`, and both would become one in a filter below.
        let made_inert = inert("{\u{e9}{secret}} {{ {secret}} %7B%7Bsecret%7D%7D");
        // Dropping non-ASCII, as the ntfy plugin once did with its header values.
        let ascii: String = made_inert.chars().filter(char::is_ascii).collect();
        // Dropping whitespace, lowercasing, trimming.
        let squeezed: String = made_inert.chars().filter(|c| !c.is_whitespace()).collect();
        for variant in [ascii, squeezed.to_lowercase(), made_inert.trim().to_owned()] {
            assert!(!variant.contains('{'), "{variant}");
            assert!(!variant.to_ascii_uppercase().contains("%7B"), "{variant}");
        }
    }
}
