//! JSON that fits in an HTTP header.
//!
//! Dropbox's content endpoints take their argument in the `Dropbox-API-Arg` header, and a
//! header is ASCII: a file name with an umlaut in it has to be written as a `\uXXXX` escape or
//! the request is malformed. `serde_json` leaves non-ASCII characters as they are, so the
//! escaping is done here, once, for both the resolver that states the header and the tests
//! that read it back.

use serde_json::Value;

/// Serialises `value` with every non-ASCII character escaped, as the header needs it.
#[must_use]
pub fn ascii_json(value: &Value) -> String {
    let text = value.to_string();
    let mut ascii = String::with_capacity(text.len());
    for character in text.chars() {
        if character.is_ascii() {
            ascii.push(character);
        } else {
            let mut units = [0_u16; 2];
            for unit in character.encode_utf16(&mut units) {
                ascii.push_str(&format!("\\u{unit:04x}"));
            }
        }
    }
    ascii
}

#[cfg(test)]
mod tests {
    use super::ascii_json;

    #[test]
    fn a_name_that_is_not_ascii_is_escaped_rather_than_sent_raw() {
        let value = serde_json::json!({"path": "/Ωmega — 🎬.mkv"});
        let header = ascii_json(&value);
        assert!(header.is_ascii(), "{header}");
        assert_eq!(
            header,
            concat!(
                "{\"path\":\"/",
                "\\u03a9",
                "mega ",
                "\\u2014",
                " ",
                "\\ud83c\\udfac",
                ".mkv\"}"
            )
        );
        // And it is still the same document.
        let read: serde_json::Value = serde_json::from_str(&header).expect("JSON");
        assert_eq!(read, value);
    }
}
