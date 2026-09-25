//! `POST /api/cache/check`, read once for the resolver and the transfers (RD-130-11).
//!
//! The resolver `plugins/premiumize/` asks it about hoster links during a link check
//! (RD-120-36); `plugins/premiumize-transfers/` asks it about magnets before anything is handed
//! over. Both send the same form body and read the same index-aligned arrays, so the body, the
//! answer's shape and the reading of one position live here, and each plugin only puts the
//! answer into its own world's vocabulary.

use serde::Deserialize;

use crate::listing::Flexible;

/// Most items one `cache/check` request carries.
///
/// The documented query limit is not stated; a hundred is the chunk the resolver has always
/// sent, and the host never hands the transfers plugin more than that in one call.
pub const MAX_ITEMS: usize = 100;

/// The `cache/check` answer: arrays aligned with the requested items by index.
#[derive(Debug, Deserialize)]
pub struct CacheCheckResponse {
    pub status: String,
    #[serde(default)]
    pub response: Vec<bool>,
    #[serde(default)]
    pub filename: Vec<Option<String>>,
    #[serde(default)]
    pub filesize: Vec<Option<Flexible>>,
    #[serde(default)]
    pub code: Option<String>,
    /// The provider's sentence. Classified, never quoted.
    #[serde(default)]
    pub message: Option<String>,
}

/// What Premiumize said about one item.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Holding {
    /// Held ready right now: `true`.
    Cached,
    /// Not held, but named: Premiumize knows the file and has not fetched it yet.
    Known,
    /// Nothing that says either.
    Unknown,
}

/// One position of a `cache/check` answer, before any plugin reads a status into it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CacheItem {
    /// The `response` entry; `None` when the answer is shorter than the request.
    pub cached: Option<bool>,
    /// The `filename` entry, with an empty name read as none.
    pub file_name: Option<String>,
    pub size: Option<u64>,
}

impl CacheItem {
    /// `true` is held; `false` with a name is known; everything else says nothing.
    #[must_use]
    pub fn holding(&self) -> Holding {
        match self.cached {
            Some(true) => Holding::Cached,
            Some(false) if self.file_name.is_some() => Holding::Known,
            _ => Holding::Unknown,
        }
    }
}

/// Exactly `count` items, one per requested position.
///
/// Positions the answer does not reach -- a shorter array, a missing one -- are items with
/// nothing in them, never a shifted neighbour: an index names the request it answers, and a
/// short array is read as "nothing said" for the tail, not as a reason to move anything up.
#[must_use]
pub fn items(count: usize, response: &CacheCheckResponse) -> Vec<CacheItem> {
    (0..count)
        .map(|index| CacheItem {
            cached: response.response.get(index).copied(),
            file_name: response
                .filename
                .get(index)
                .and_then(Clone::clone)
                .filter(|name| !name.is_empty()),
            size: response
                .filesize
                .get(index)
                .and_then(|size| size.as_ref().and_then(Flexible::as_u64)),
        })
        .collect()
}

/// The form body: one `items[]` per item, in order, `application/x-www-form-urlencoded`.
///
/// Encoded byte for byte as the `url` crate's form serializer does it -- alphanumerics and
/// `*-._` stay, a space becomes `+`, everything else is `%XX` -- so the resolver sends what it
/// sent before this moved here.
#[must_use]
pub fn check_body(items: &[String]) -> Vec<u8> {
    let mut body = String::new();
    for item in items {
        if !body.is_empty() {
            body.push('&');
        }
        encode_into(&mut body, "items[]");
        body.push('=');
        encode_into(&mut body, item);
    }
    body.into_bytes()
}

fn encode_into(body: &mut String, value: &str) {
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'*' | b'-' | b'.' | b'_' => {
                body.push(char::from(byte));
            }
            b' ' => body.push('+'),
            _ => {
                use std::fmt::Write;
                let _ = write!(body, "%{byte:02X}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CacheCheckResponse, CacheItem, Holding, check_body, items};

    fn parse(body: &str) -> CacheCheckResponse {
        serde_json::from_str(body).expect("a cache/check answer")
    }

    /// `true` is held, `false` with a name is known, and `false` without one says nothing.
    #[test]
    fn a_position_reads_as_held_known_or_nothing() {
        let response = parse(
            r#"{"status":"success","response":[true,false,false],"filename":["a.rar","b.rar",null],"filesize":["1024",2048,null]}"#,
        );
        let read = items(3, &response);
        assert_eq!(
            read.iter().map(CacheItem::holding).collect::<Vec<_>>(),
            [Holding::Cached, Holding::Known, Holding::Unknown]
        );
        assert_eq!(read[0].file_name.as_deref(), Some("a.rar"));
        assert_eq!(read[0].size, Some(1024));
        assert_eq!(read[1].size, Some(2048));
        assert_eq!(read[2].size, None);
    }

    /// A short answer leaves the tail empty rather than moving anything up, and an empty name
    /// is no name.
    #[test]
    fn a_short_answer_is_read_by_position_and_never_shifted() {
        let response =
            parse(r#"{"status":"success","response":[false],"filename":[""],"filesize":[]}"#);
        let read = items(3, &response);
        assert_eq!(read.len(), 3);
        assert_eq!(read[0].holding(), Holding::Unknown, "an empty name is none");
        assert_eq!(read[1].cached, None);
        assert_eq!(read[2].holding(), Holding::Unknown);
    }

    /// The body is the one the resolver sent through the `url` crate's form serializer.
    #[test]
    fn the_body_is_one_items_entry_per_item_in_order() {
        let body = check_body(&[
            "magnet:?xt=urn:btih:c8f1a0b2&dn=a b".to_owned(),
            "https://h.example/f~1".to_owned(),
        ]);
        assert_eq!(
            String::from_utf8(body).expect("utf-8"),
            "items%5B%5D=magnet%3A%3Fxt%3Durn%3Abtih%3Ac8f1a0b2%26dn%3Da+b\
             &items%5B%5D=https%3A%2F%2Fh.example%2Ff%7E1"
        );
        assert!(check_body(&[]).is_empty());
    }
}
