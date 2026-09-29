//! Reading the provider's answer about one file, without a JSON parser.
//!
//! The answer this scaffold expects is `{"url": "...", "name": "...", "size": 123}`: where the
//! ciphertext can be fetched, and what the file is called. Scanned rather than parsed, the same
//! choice the crawler scaffold and the shipped plugins make — a guest pays for every dependency
//! in code size. Replace this module if your provider answers in anything that wants a parser.

/// What the provider said about a file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Stored {
    /// The address the ciphertext is served from. The host downloads it, so it has to lie on
    /// a domain `manifest.toml` grants — it refuses one that does not.
    pub url: String,
    pub name: Option<String>,
    pub size: Option<u64>,
}

/// The file the answer describes, or `None` when it names no usable address.
#[must_use]
pub fn stored(body: &str) -> Option<Stored> {
    let url = string_field(body, "url")?;
    // An address the provider sent is still only an address: `https` or nothing.
    if !url.starts_with("https://") {
        return None;
    }
    Some(Stored {
        url,
        name: string_field(body, "name").filter(|name| !name.is_empty()),
        size: number_field(body, "size"),
    })
}

/// The string value of a JSON field.
fn string_field(body: &str, name: &str) -> Option<String> {
    let mut rest = value_after(body, name)?.strip_prefix('"')?;
    let mut out = String::new();
    loop {
        let mut chars = rest.chars();
        let character = chars.next()?;
        rest = chars.as_str();
        match character {
            '"' => return Some(out),
            '\\' => {
                let mut escaped = rest.chars();
                match escaped.next()? {
                    'n' => out.push('\n'),
                    't' => out.push('\t'),
                    'r' => out.push('\r'),
                    other => out.push(other),
                }
                rest = escaped.as_str();
            }
            other => out.push(other),
        }
    }
}

/// The numeric value of a JSON field, quoted or not — providers spell sizes both ways.
fn number_field(body: &str, name: &str) -> Option<u64> {
    let rest = value_after(body, name)?;
    let rest = rest.strip_prefix('"').unwrap_or(rest);
    let end = rest
        .find(|character: char| !character.is_ascii_digit())
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

fn value_after<'a>(body: &'a str, name: &str) -> Option<&'a str> {
    let needle = format!("\"{name}\"");
    let mut from = 0;
    while let Some(at) = body[from..].find(&needle) {
        let after = &body[from + at + needle.len()..];
        if let Some(value) = after.trim_start().strip_prefix(':') {
            return Some(value.trim_start());
        }
        from += at + needle.len();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{Stored, stored};

    #[test]
    fn an_answer_names_the_address_the_name_and_the_size() {
        let body = r#"{"url": "https://eu1.storage.example.com/c/9f", "name": "clip.mkv", "size": "524288"}"#;
        assert_eq!(
            stored(body),
            Some(Stored {
                url: "https://eu1.storage.example.com/c/9f".to_owned(),
                name: Some("clip.mkv".to_owned()),
                size: Some(524_288),
            })
        );
    }

    #[test]
    fn an_answer_without_a_usable_address_is_no_answer() {
        assert_eq!(stored(r#"{"name": "clip.mkv"}"#), None);
        assert_eq!(
            stored(r#"{"url": "http://eu1.storage.example.com/c/9f"}"#),
            None
        );
        assert_eq!(stored("not json at all"), None);
    }

    #[test]
    fn name_and_size_are_optional() {
        let answer = stored(r#"{"url":"https://eu1.storage.example.com/c/9f","name":""}"#)
            .expect("an address");
        assert_eq!(answer.name, None);
        assert_eq!(answer.size, None);
    }
}
