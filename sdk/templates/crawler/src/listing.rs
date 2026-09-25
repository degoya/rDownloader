//! Reading a directory listing, without a JSON parser.
//!
//! A sandboxed guest pays for every dependency in code size and in surface, and a listing is
//! a flat array of small objects. Scanned rather than parsed, therefore — the same choice the
//! shipped sign-in plugins make. Replace this module wholesale if the provider you are writing
//! for answers in XML, in HTML, or in anything else that wants a real parser.

/// One entry of a listing: either something to walk into, or a file to hand back.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Entry {
    Folder {
        id: String,
        name: String,
    },
    File {
        name: String,
        url: String,
        size: Option<u64>,
    },
}

/// The string value of a JSON field inside one object's text.
#[must_use]
pub fn string_field(body: &str, name: &str) -> Option<String> {
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
#[must_use]
pub fn number_field(body: &str, name: &str) -> Option<u64> {
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

/// Splits the objects of the first JSON array in `body`, respecting nesting and strings.
///
/// Written out rather than taken from a parser because the alternative — cutting on `},{` —
/// is wrong the moment an entry carries a nested object or a brace inside a file name, and
/// wrong in the direction that silently loses files.
#[must_use]
pub fn objects(body: &str, array_field: &str) -> Vec<String> {
    let Some(rest) = value_after(body, array_field).and_then(|rest| rest.strip_prefix('[')) else {
        return Vec::new();
    };
    let mut objects = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (index, character) in rest.char_indices() {
        if in_string {
            match character {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match character {
            '"' => in_string = true,
            '{' => {
                if depth == 0 {
                    start = index;
                }
                depth += 1;
            }
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    objects.push(rest[start..=index].to_owned());
                }
            }
            ']' if depth == 0 => break,
            _ => {}
        }
    }
    objects
}

/// Reads one entry, or `None` when the object describes neither a folder nor a usable file.
#[must_use]
pub fn entry(object: &str) -> Option<Entry> {
    let name = string_field(object, "name").unwrap_or_default();
    match string_field(object, "type").as_deref() {
        Some("folder") => {
            let id = string_field(object, "id").filter(|id| !id.is_empty())?;
            Some(Entry::Folder { id, name })
        }
        Some("file") => {
            // A file whose address is missing is dropped rather than reported: it is one
            // entry of a listing, and refusing the whole folder over it helps nobody.
            let url = string_field(object, "link").filter(|link| !link.is_empty())?;
            Some(Entry::File {
                name,
                url,
                size: number_field(object, "size"),
            })
        }
        _ => None,
    }
}

/// Reads a whole listing document into entries.
#[must_use]
pub fn entries(body: &str) -> Vec<Entry> {
    objects(body, "content")
        .iter()
        .filter_map(|object| entry(object))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{Entry, entries, number_field, objects};

    const LISTING: &str = r#"{
      "status": "success",
      "name": "Season 1",
      "content": [
        {"id": "f1", "name": "Extras", "type": "folder"},
        {"id": "x1", "name": "e01.mkv", "type": "file", "size": "1024", "link": "https://cdn.example.org/e01.mkv"},
        {"id": "x2", "name": "note {a}", "type": "file", "size": 7, "link": "https://cdn.example.org/n", "meta": {"a": 1}}
      ]
    }"#;

    #[test]
    fn a_listing_yields_its_folders_and_its_files() {
        assert_eq!(
            entries(LISTING),
            vec![
                Entry::Folder {
                    id: "f1".to_owned(),
                    name: "Extras".to_owned()
                },
                Entry::File {
                    name: "e01.mkv".to_owned(),
                    url: "https://cdn.example.org/e01.mkv".to_owned(),
                    size: Some(1024),
                },
                Entry::File {
                    name: "note {a}".to_owned(),
                    url: "https://cdn.example.org/n".to_owned(),
                    size: Some(7),
                },
            ]
        );
    }

    #[test]
    fn a_brace_in_a_name_does_not_split_an_entry() {
        // The reason `objects` counts depth instead of cutting on `},{`: both of these used
        // to lose the entry they sat in.
        assert_eq!(objects(LISTING, "content").len(), 3);
    }

    #[test]
    fn an_entry_without_what_it_needs_is_dropped_and_not_guessed() {
        assert!(entries(r#"{"content":[{"type":"file","name":"a"}]}"#).is_empty());
        assert!(entries(r#"{"content":[{"type":"folder","name":"a"}]}"#).is_empty());
        assert!(entries(r#"{"status":"error"}"#).is_empty());
    }

    #[test]
    fn a_size_is_read_whether_the_provider_quotes_it_or_not() {
        assert_eq!(number_field(r#"{"size":42}"#, "size"), Some(42));
        assert_eq!(number_field(r#"{"size":"42"}"#, "size"), Some(42));
        assert_eq!(number_field(r#"{"size":null}"#, "size"), None);
    }
}
