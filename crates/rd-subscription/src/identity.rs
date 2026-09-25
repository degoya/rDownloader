//! Canonical item identity (RD-080-07).
//!
//! The whole once-only guarantee rests on this key. It is stored with a UNIQUE index next to
//! the subscription, so "this item was already handled" is a database fact rather than a
//! comparison somebody has to remember to make.
//!
//! What the key must survive: a restart, a feed that reorders itself, a title that gets
//! edited, a URL that gains a tracking parameter, and the same item appearing under both
//! `http` and `https`. What it must *not* do is collapse two genuinely different items into
//! one, which is why the fallback hashes several fields rather than just the title.

use sha2::{Digest, Sha256};
use url::Url;

use rd_core::MAX_ITEM_KEY;

/// Everything known about an item at the moment its identity is decided.
#[derive(Clone, Debug, Default)]
pub struct ItemIdentity<'a> {
    /// The source's own id — a video id, a `<guid>`, an indexer's `id`. Strongest signal.
    pub source_id: Option<&'a str>,
    pub url: Option<&'a Url>,
    pub title: Option<&'a str>,
    /// Publication instant as the source stated it, used only in the last-resort hash.
    pub published: Option<&'a str>,
    /// What the item *is*, read out of a release name (RD-110-21). Stronger than all three
    /// above, because two releases of one episode are one thing under three addresses.
    pub release: Option<&'a str>,
}

/// Derives the canonical key for one item.
///
/// Preference order, strongest first:
///
/// 1. what a release name says the item *is* (RD-110-21), which is the only signal that
///    recognises one episode across the several releases that carry it;
/// 2. the source's own id, which is what it uses to mean "the same thing";
/// 3. the normalised URL, which is stable when an id is missing;
/// 4. a hash of title and publication date, for a feed that offers neither.
///
/// The release key is deliberately above the source's own id and not below it: on a release
/// page every posting has an id and an address of its own, so both of them say "new" about
/// an episode somebody already has. Only the adapter that read a release name ever sets it,
/// so no other kind of subscription changes behaviour by one character.
///
/// The result is always non-empty and never longer than [`MAX_ITEM_KEY`].
#[must_use]
pub fn item_key(identity: &ItemIdentity<'_>) -> String {
    if let Some(release) = identity
        .release
        .map(str::trim)
        .filter(|release| !release.is_empty())
    {
        return truncate(&format!("release:{release}"));
    }
    if let Some(id) = identity
        .source_id
        .map(str::trim)
        .filter(|id| !id.is_empty())
    {
        return truncate(&format!("id:{id}"));
    }
    if let Some(url) = identity.url {
        return truncate(&format!("url:{}", normalize_url(url)));
    }
    // Nothing stable was offered, so the item is identified by what it says about itself.
    // A hash rather than the raw text: the title alone would collide across episodes of the
    // same series, and the pair is both fields' worth of distinctness in a bounded string.
    let mut hasher = Sha256::new();
    hasher.update(identity.title.unwrap_or_default().trim().as_bytes());
    hasher.update([0]);
    hasher.update(identity.published.unwrap_or_default().trim().as_bytes());
    format!("hash:{:x}", hasher.finalize())
}

/// Normalises a URL so cosmetic differences do not create a second identity.
///
/// Deliberately conservative: only the parts that provably do not change *which item this
/// is* are removed. Query parameters other than the known trackers are kept, because on
/// plenty of sites the query is the entire address of the item.
#[must_use]
pub fn normalize_url(url: &Url) -> String {
    let mut normalized = url.clone();
    normalized.set_fragment(None);
    // A session or campaign parameter is about how the link was shared, not about what it
    // points at; the same video arriving from two newsletters is one video.
    let kept: Vec<(String, String)> = normalized
        .query_pairs()
        .filter(|(key, _)| !is_tracking_parameter(key))
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    if kept.is_empty() {
        normalized.set_query(None);
    } else {
        let mut serializer = url::form_urlencoded::Serializer::new(String::new());
        for (key, value) in &kept {
            serializer.append_pair(key, value);
        }
        normalized.set_query(Some(&serializer.finish()));
    }
    // `http` and `https` for the same address are the same item; the host is compared
    // case-insensitively because DNS is.
    let scheme = if normalized.scheme() == "http" {
        "https"
    } else {
        normalized.scheme()
    };
    let host = normalized
        .host_str()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let port = normalized
        .port()
        .map(|port| format!(":{port}"))
        .unwrap_or_default();
    let path = normalized.path().trim_end_matches('/');
    let query = normalized
        .query()
        .map(|query| format!("?{query}"))
        .unwrap_or_default();
    format!("{scheme}://{host}{port}{path}{query}")
}

fn is_tracking_parameter(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    key.starts_with("utm_")
        || matches!(
            key.as_str(),
            "fbclid" | "gclid" | "mc_cid" | "mc_eid" | "igshid" | "si" | "ref" | "ref_src"
        )
}

fn truncate(value: &str) -> String {
    if value.len() <= MAX_ITEM_KEY {
        return value.to_owned();
    }
    // A truncated key would collide with every other key sharing its prefix, so the tail is
    // replaced by a hash of the whole thing rather than simply cut off.
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    let digest = format!("{:x}", hasher.finalize());
    let head_len = MAX_ITEM_KEY - digest.len() - 1;
    let mut head = value[..head_len].to_owned();
    head.push('#');
    head.push_str(&digest);
    head
}

#[cfg(test)]
mod tests {
    use super::{ItemIdentity, item_key, normalize_url};
    use rd_core::MAX_ITEM_KEY;
    use url::Url;

    fn url(input: &str) -> Url {
        input.parse().expect("url")
    }

    #[test]
    fn a_source_id_wins_over_everything_else() {
        let page = url("https://example.test/watch?v=abc");
        let with_id = item_key(&ItemIdentity {
            source_id: Some("abc"),
            url: Some(&page),
            title: Some("Episode 1"),
            published: None,
            release: None,
        });
        // A renamed, re-shared item keeps its identity.
        let renamed = item_key(&ItemIdentity {
            source_id: Some("abc"),
            url: Some(&url("https://example.test/embed/abc")),
            title: Some("Episode 1 (remastered)"),
            published: Some("2026-01-01"),
            release: None,
        });
        assert_eq!(with_id, renamed);
    }

    #[test]
    fn cosmetic_url_differences_do_not_create_a_second_item() {
        let variants = [
            "https://example.test/watch?v=abc",
            "http://example.test/watch?v=abc",
            "https://EXAMPLE.test/watch?v=abc#t=30",
            "https://example.test/watch?v=abc&utm_source=newsletter",
            "https://example.test/watch/?v=abc",
        ];
        let keys: Vec<String> = variants
            .iter()
            .map(|value| {
                let parsed = url(value);
                item_key(&ItemIdentity {
                    url: Some(&parsed),
                    ..ItemIdentity::default()
                })
            })
            .collect();
        assert!(
            keys.windows(2).all(|pair| pair[0] == pair[1]),
            "same item got different keys: {keys:?}"
        );
    }

    #[test]
    fn a_meaningful_query_parameter_is_kept() {
        // On plenty of sites the query *is* the address of the item; stripping it would
        // collapse a whole channel into one entry.
        let first = url("https://example.test/index.php?id=1");
        let second = url("https://example.test/index.php?id=2");
        assert_ne!(normalize_url(&first), normalize_url(&second));
    }

    #[test]
    fn different_items_without_an_id_or_url_stay_distinct() {
        let make = |title: &str, published: &str| {
            item_key(&ItemIdentity {
                title: Some(title),
                published: Some(published),
                release: None,
                ..ItemIdentity::default()
            })
        };
        assert_ne!(
            make("Episode 1", "2026-01-01"),
            make("Episode 2", "2026-01-01")
        );
        // The same title on two dates is two episodes, which is exactly why the date is in
        // the hash: a weekly show often reuses its title.
        assert_ne!(make("Weekly", "2026-01-01"), make("Weekly", "2026-01-08"));
        assert_eq!(
            make("Episode 1", "2026-01-01"),
            make("Episode 1", "2026-01-01")
        );
    }

    #[test]
    fn a_key_is_never_empty_and_never_too_long() {
        let empty = item_key(&ItemIdentity::default());
        assert!(!empty.is_empty());

        let long = "x".repeat(4_000);
        let key = item_key(&ItemIdentity {
            source_id: Some(&long),
            ..ItemIdentity::default()
        });
        assert!(key.len() <= MAX_ITEM_KEY, "{}", key.len());
        // Truncation must not make two different ids equal.
        let other = item_key(&ItemIdentity {
            source_id: Some(&format!("{long}y")),
            ..ItemIdentity::default()
        });
        assert_ne!(key, other);
    }

    #[test]
    fn a_release_name_outranks_the_id_and_the_address() {
        // RD-110-21. Two postings of one episode: each has its own id and its own address,
        // and both say "new" about something the person already has. Only the release key
        // recognises them as one thing.
        let first = item_key(&ItemIdentity {
            source_id: Some("post-1"),
            url: Some(&url("https://board.test/tv/one/")),
            release: Some("the expanse|s05e03"),
            ..ItemIdentity::default()
        });
        let second = item_key(&ItemIdentity {
            source_id: Some("post-2"),
            url: Some(&url("https://board.test/tv/two/")),
            release: Some("the expanse|s05e03"),
            ..ItemIdentity::default()
        });
        assert_eq!(first, second);
        assert!(first.starts_with("release:"), "{first}");
        // Nothing else changes: an item with no release key is identified as it always was.
        let without = item_key(&ItemIdentity {
            source_id: Some("post-1"),
            url: Some(&url("https://board.test/tv/one/")),
            ..ItemIdentity::default()
        });
        assert_eq!(without, "id:post-1");
        // A blank key is no key.
        assert_eq!(
            item_key(&ItemIdentity {
                source_id: Some("post-1"),
                release: Some("  "),
                ..ItemIdentity::default()
            }),
            "id:post-1"
        );
    }

    #[test]
    fn a_blank_source_id_falls_through_to_the_url() {
        let page = url("https://example.test/watch?v=abc");
        let blank = item_key(&ItemIdentity {
            source_id: Some("   "),
            url: Some(&page),
            ..ItemIdentity::default()
        });
        let none = item_key(&ItemIdentity {
            url: Some(&page),
            ..ItemIdentity::default()
        });
        assert_eq!(blank, none);
    }
}
