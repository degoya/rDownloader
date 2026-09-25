//! A minimal bencode reader.
//!
//! Only what two jobs need: the `url-list` key of a `.torrent` (BEP 19) and the nested
//! counters of a tracker scrape response (BEP 48). librqbit's own decoder is not exposed
//! for either, and pulling a general-purpose bencode crate in for two lookups would be a
//! dependency for less code than this file.

/// A decoded bencode value.
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum Value<'a> {
    Integer(i64),
    Bytes(&'a [u8]),
    List(Vec<Value<'a>>),
    Dict(Vec<(&'a [u8], Value<'a>)>),
}

impl<'a> Value<'a> {
    /// The value behind a dictionary key.
    pub fn get(&self, key: &[u8]) -> Option<&Value<'a>> {
        match self {
            Self::Dict(entries) => entries
                .iter()
                .find(|(name, _)| *name == key)
                .map(|(_, value)| value),
            _ => None,
        }
    }

    /// The value as an integer, when it is one.
    pub fn integer(&self) -> Option<i64> {
        match self {
            Self::Integer(value) => Some(*value),
            _ => None,
        }
    }

    /// The value as a byte string, when it is one.
    pub fn bytes(&self) -> Option<&'a [u8]> {
        match self {
            Self::Bytes(value) => Some(value),
            _ => None,
        }
    }

    /// The entries of a dictionary, when it is one.
    pub fn entries(&self) -> Option<&[(&'a [u8], Value<'a>)]> {
        match self {
            Self::Dict(entries) => Some(entries),
            _ => None,
        }
    }

    /// The items of a list, when it is one.
    pub fn items(&self) -> Option<&[Value<'a>]> {
        match self {
            Self::List(items) => Some(items),
            _ => None,
        }
    }
}

/// Decodes one complete bencode document.
///
/// Trailing bytes after the first value are ignored, which is what tracker responses and
/// `.torrent` files in the wild sometimes carry.
pub(crate) fn decode(bytes: &[u8]) -> Option<Value<'_>> {
    read(bytes, 0).map(|(value, _)| value)
}

/// Reads the value at `start`, returning it and the offset just past it.
fn read(bytes: &[u8], start: usize) -> Option<(Value<'_>, usize)> {
    match bytes.get(start)? {
        b'i' => {
            let end = bytes[start..].iter().position(|byte| *byte == b'e')? + start;
            let value = std::str::from_utf8(bytes.get(start + 1..end)?)
                .ok()?
                .parse()
                .ok()?;
            Some((Value::Integer(value), end + 1))
        }
        b'0'..=b'9' => read_bytes(bytes, start).map(|(value, next)| (Value::Bytes(value), next)),
        b'l' => {
            let mut cursor = start + 1;
            let mut items = Vec::new();
            while *bytes.get(cursor)? != b'e' {
                let (item, next) = read(bytes, cursor)?;
                items.push(item);
                cursor = next;
            }
            Some((Value::List(items), cursor + 1))
        }
        b'd' => {
            let mut cursor = start + 1;
            let mut entries = Vec::new();
            while *bytes.get(cursor)? != b'e' {
                let (key, after_key) = read_bytes(bytes, cursor)?;
                let (value, after_value) = read(bytes, after_key)?;
                entries.push((key, value));
                cursor = after_value;
            }
            Some((Value::Dict(entries), cursor + 1))
        }
        _ => None,
    }
}

/// Reads a bencode byte string at `start`.
fn read_bytes(bytes: &[u8], start: usize) -> Option<(&[u8], usize)> {
    let colon = bytes[start..].iter().position(|byte| *byte == b':')? + start;
    let length: usize = std::str::from_utf8(bytes.get(start..colon)?)
        .ok()?
        .parse()
        .ok()?;
    let value_start = colon + 1;
    let value_end = value_start.checked_add(length)?;
    Some((bytes.get(value_start..value_end)?, value_end))
}

#[cfg(test)]
mod tests {
    use super::{Value, decode};

    #[test]
    fn scalars_decode() {
        assert_eq!(decode(b"i42e"), Some(Value::Integer(42)));
        assert_eq!(decode(b"i-7e"), Some(Value::Integer(-7)));
        assert_eq!(decode(b"4:spam"), Some(Value::Bytes(b"spam")));
    }

    #[test]
    fn a_nested_scrape_response_decodes() {
        // d5:filesd20:<hash>d8:completei5e10:downloadedi9e10:incompletei3eeee
        let mut bytes = b"d5:filesd20:".to_vec();
        bytes.extend_from_slice(&[0_u8; 20]);
        bytes.extend_from_slice(b"d8:completei5e10:downloadedi9e10:incompletei3eeee");
        let value = decode(&bytes).expect("decodes");
        let files = value.get(b"files").expect("files");
        let entries = files.entries().expect("dict");
        assert_eq!(entries.len(), 1);
        let counters = &entries[0].1;
        assert_eq!(counters.get(b"complete").and_then(Value::integer), Some(5));
        assert_eq!(
            counters.get(b"downloaded").and_then(Value::integer),
            Some(9)
        );
        assert_eq!(
            counters.get(b"incomplete").and_then(Value::integer),
            Some(3)
        );
    }

    #[test]
    fn a_list_of_byte_strings_decodes() {
        let value = decode(b"l3:one3:twoe").expect("decodes");
        let items = value.items().expect("list");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].bytes(), Some(&b"one"[..]));
    }

    #[test]
    fn truncated_and_malformed_input_is_rejected() {
        assert!(decode(b"d3:key").is_none());
        assert!(decode(b"5:ab").is_none());
        assert!(decode(b"x").is_none());
        assert!(decode(b"").is_none());
    }

    #[test]
    fn a_failure_response_carries_its_reason() {
        let value = decode(b"d14:failure reason9:not founde").expect("decodes");
        assert_eq!(
            value.get(b"failure reason").and_then(Value::bytes),
            Some(&b"not found"[..])
        );
    }
}
