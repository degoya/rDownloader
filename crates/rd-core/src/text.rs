//! Cutting free text to a bound without splitting a character (RD-1280-06).
//!
//! Error details, log values and feed attributes are bounded before they are stored or shown;
//! these two functions are the one place that cut is made.

/// The longest prefix of `text` that is at most `max_bytes` bytes long and ends on a character
/// boundary; `text` itself when it fits.
#[must_use]
pub fn truncate(text: &str, max_bytes: usize) -> &str {
    &text[..text.floor_char_boundary(max_bytes)]
}

/// The first `max_chars` characters of `text`; `text` itself when it has no more.
#[must_use]
pub fn truncate_chars(text: &str, max_chars: usize) -> &str {
    match text.char_indices().nth(max_chars) {
        Some((end, _)) => &text[..end],
        None => text,
    }
}

#[cfg(test)]
mod tests {
    use super::{truncate, truncate_chars};

    #[test]
    fn a_byte_bound_is_cut_without_splitting_a_character() {
        let text = "quota café exceeded";
        // Byte 10 lands inside the two-byte "é", so the cut steps back to byte 9.
        assert_eq!(truncate(text, 10), "quota caf");
        // Byte 11 is the boundary right after "é", so it is kept.
        assert_eq!(truncate(text, 11), "quota café");
        // A short text is returned unchanged.
        assert_eq!(truncate("short", 300), "short");
        assert_eq!(truncate("é", 1), "");
        assert_eq!(truncate("", 0), "");
    }

    #[test]
    fn a_character_bound_counts_characters_not_bytes() {
        assert_eq!(truncate_chars("café au lait", 4), "café");
        assert_eq!(truncate_chars("café", 4), "café");
        assert_eq!(truncate_chars("café", 10), "café");
        assert_eq!(truncate_chars("café", 0), "");
        let long = "x".repeat(250);
        assert_eq!(
            truncate_chars(&long, 200),
            long.chars().take(200).collect::<String>()
        );
    }
}
