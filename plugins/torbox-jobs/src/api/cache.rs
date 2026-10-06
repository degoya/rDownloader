//! What `checkcached` says TorBox holds (RD-130-11).

/// One thing `checkcached` says TorBox holds.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CachedEntry {
    /// The digest it is held under, as TorBox spelled it. Compared without regard to case.
    pub hash: String,
    pub name: Option<String>,
    pub size: Option<u64>,
}

/// What a `checkcached` answer says is held, or `None` when the answer is not one.
///
/// Tolerant on purpose, because the shape is not pinned down anywhere: TorBox's OpenAPI
/// document leaves the response schema empty, and its SDK says `data` is a dictionary of
/// `{name, size, hash}` without saying what it is keyed by. So `data` is read as an object
/// keyed by hash, as a list of entries, or as a single entry; `null`, `false`, `{}` and `[]`
/// all mean "nothing held". Only an answer that is not JSON, or carries no `data` at all, is
/// refused. TorBox names only what it holds, so nothing here ever says "known but not held".
#[must_use]
pub fn cached_entries(body: &[u8]) -> Option<Vec<CachedEntry>> {
    let envelope: serde_json::Value = serde_json::from_slice(body).ok()?;
    let data = envelope.as_object()?.get("data")?;
    Some(match data {
        serde_json::Value::Object(map) => {
            if map.get("hash").is_some_and(serde_json::Value::is_string) {
                cached_entry(data, None).into_iter().collect()
            } else {
                map.iter()
                    .filter_map(|(key, value)| match value {
                        serde_json::Value::Object(_) => cached_entry(value, Some(key)),
                        serde_json::Value::Bool(true) => cached_entry(value, Some(key)),
                        _ => None,
                    })
                    .collect()
            }
        }
        serde_json::Value::Array(items) => items
            .iter()
            .filter_map(|item| match item {
                serde_json::Value::String(hash) => cached_entry(item, Some(hash)),
                _ => cached_entry(item, None),
            })
            .collect(),
        _ => Vec::new(),
    })
}

/// One entry, with the hash taken from the entry itself or, failing that, from its key.
fn cached_entry(value: &serde_json::Value, key: Option<&str>) -> Option<CachedEntry> {
    let hash = value
        .get("hash")
        .and_then(serde_json::Value::as_str)
        .or(key)
        .map(str::trim)
        .filter(|hash| !hash.is_empty())?;
    Some(CachedEntry {
        hash: hash.to_owned(),
        name: value
            .get("name")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_owned),
        size: value.get("size").and_then(cached_size),
    })
}

/// A size as TorBox states it: an integer, or a float in its SDK's model. Truncated; a
/// negative or non-finite one is no size at all.
// `as` saturates for a float above `u64::MAX`, and the checks rule out the two cases where it
// would invent a number.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn cached_size(value: &serde_json::Value) -> Option<u64> {
    if let Some(size) = value.as_u64() {
        return Some(size);
    }
    let size = value.as_f64()?;
    (size.is_finite() && size >= 0.0).then_some(size as u64)
}
