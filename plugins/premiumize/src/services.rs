//! Hoster catalogue helpers shared by both builds.

/// Lower-cases, deduplicates and sorts the hoster domains of both capability lists.
#[must_use]
pub(crate) fn merge_hosters(cache: Vec<String>, directdl: Vec<String>) -> Vec<String> {
    let mut hosters: Vec<String> = cache
        .into_iter()
        .chain(directdl)
        .map(|host| host.trim().to_ascii_lowercase())
        .filter(|host| !host.is_empty())
        .collect();
    hosters.sort();
    hosters.dedup();
    hosters
}
