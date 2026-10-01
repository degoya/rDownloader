//! The one builder of a Newznab/Torznab search address (RD-080-11, RD-180-19, RD-180-20).
//!
//! A subscription poll and an interactive search ask the same question of the same server, so
//! both go through [`build_indexer_query`]; `build_page_query` is the subscription's spelling of
//! it with no search parameters. A parameter is only ever *added*: whatever the stored address
//! already carries -- a saved search copied out of the indexer's own RSS button -- keeps
//! winning, and an unset parameter adds nothing, so an address built without search parameters
//! is byte for byte the one built before they existed.

use url::Url;

use rd_core::IndexerSearch;

/// What one request asks besides the address and the key.
#[derive(Clone, Copy, Debug)]
pub struct IndexerQuery<'a> {
    /// Results asked for; replaces a `limit` in the address.
    pub limit: u32,
    /// Where the page starts; replaces an `offset` in the address.
    pub offset: u32,
    /// Sent as `cat` when the address names none. Empty asks for everything.
    pub categories: &'a [String],
    /// `q`, `maxage`, `pw` and `pred`, each when set and not already in the address.
    pub search: &'a IndexerSearch,
}

/// Builds one request to an indexer.
///
/// `limit` and `offset` belong to the caller rather than to a copied saved-search URL: keeping
/// a stale offset would ask for the same page forever, and keeping a tiny limit would make the
/// five-request ceiling arbitrarily small. Every other parameter already in the address remains
/// exactly as supplied.
pub fn build_indexer_query(
    base: &Url,
    api_key: &str,
    query: &IndexerQuery<'_>,
) -> anyhow::Result<Url> {
    let mut url = base.clone();
    // Existing parameters win: a subscription URL is usually copied out of the indexer's own
    // "RSS feed" button and already carries `t`, `cat` and often `q`.
    let existing: Vec<(String, String)> = url
        .query_pairs()
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    let has = |name: &str| {
        existing
            .iter()
            .any(|(key, _)| key.eq_ignore_ascii_case(name))
    };
    {
        let mut pairs = url.query_pairs_mut();
        pairs.clear();
        for (key, value) in &existing {
            // The key is replaced rather than appended: a stale one copied out of a browser
            // would otherwise be sent alongside the stored one.
            if key.eq_ignore_ascii_case("apikey")
                || key.eq_ignore_ascii_case("api_key")
                || key.eq_ignore_ascii_case("limit")
                || key.eq_ignore_ascii_case("offset")
            {
                continue;
            }
            pairs.append_pair(key, value);
        }
        if !has("t") {
            pairs.append_pair("t", "search");
        }
        pairs.append_pair("limit", &query.limit.to_string());
        pairs.append_pair("offset", &query.offset.to_string());
        // Asks for the `<newznab:attr>` block that carries size and category.
        if !has("extended") {
            pairs.append_pair("extended", "1");
        }
        // Only when the address does not already say which categories it wants: a subscription
        // URL is usually copied out of the indexer's own RSS button, and one that already
        // carries `cat` is a saved search whose author meant it. Without this the categories a
        // subscription had chosen only ever sorted the results afterwards, so an indexer was
        // asked for everything and most of it was thrown away.
        if !has("cat") && !query.categories.is_empty() {
            pairs.append_pair("cat", &query.categories.join(","));
        }
        // The search parameters (RD-180-20). Passed on as written: `q` keeps its `!word`
        // exclusions, which are the indexer's syntax, not ours.
        let search = query.search;
        if let Some(term) = search.query.as_deref().map(str::trim)
            && !term.is_empty()
            && !has("q")
        {
            pairs.append_pair("q", term);
        }
        if let Some(days) = search.max_age_days
            && !has("maxage")
        {
            pairs.append_pair("maxage", &days.to_string());
        }
        if search.hide_passworded && !has("pw") {
            pairs.append_pair("pw", "2");
        }
        if let Some(pretime) = search.pretime
            && !has("pred")
        {
            pairs.append_pair("pred", &pretime.to_string());
        }
        pairs.append_pair("apikey", api_key);
    }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::{IndexerQuery, build_indexer_query};
    use rd_core::IndexerSearch;
    use url::Url;

    fn base(input: &str) -> Url {
        input.parse().expect("url")
    }

    fn pairs(url: &Url) -> Vec<(String, String)> {
        url.query_pairs()
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect()
    }

    /// The promise every existing subscription relies on: no search parameters, the address
    /// it always polled.
    #[test]
    fn an_empty_search_builds_the_address_it_always_built() {
        let categories = ["5040".to_owned()];
        let url = build_indexer_query(
            &base("https://indexer.test/api?t=tvsearch&q=kept"),
            "SECRET",
            &IndexerQuery {
                limit: 100,
                offset: 200,
                categories: &categories,
                search: &IndexerSearch::default(),
            },
        )
        .expect("query");
        assert_eq!(
            url.as_str(),
            "https://indexer.test/api?t=tvsearch&q=kept&limit=100&offset=200&extended=1\
             &cat=5040&apikey=SECRET"
        );
    }

    #[test]
    fn every_search_parameter_is_sent_when_set() {
        let search = IndexerSearch {
            query: Some("  some show !german ".to_owned()),
            max_age_days: Some(30),
            hide_passworded: true,
            pretime: Some(1),
        };
        let url = build_indexer_query(
            &base("https://indexer.test/api"),
            "SECRET",
            &IndexerQuery {
                limit: 500,
                offset: 0,
                categories: &[],
                search: &search,
            },
        )
        .expect("query");
        let pairs = pairs(&url);
        for expected in [
            ("t", "search"),
            ("q", "some show !german"),
            ("maxage", "30"),
            ("pw", "2"),
            ("pred", "1"),
            ("limit", "500"),
            ("extended", "1"),
        ] {
            assert!(
                pairs.contains(&(expected.0.to_owned(), expected.1.to_owned())),
                "{expected:?} missing from {url}"
            );
        }
        // The key goes last, as it always did.
        assert_eq!(
            pairs.last(),
            Some(&("apikey".to_owned(), "SECRET".to_owned()))
        );
    }

    /// A saved search keeps what its author wrote into it (the rule `cat` already follows).
    #[test]
    fn parameters_already_in_the_address_keep_winning() {
        let search = IndexerSearch {
            query: Some("mine".to_owned()),
            max_age_days: Some(30),
            hide_passworded: true,
            pretime: Some(2),
        };
        let url = build_indexer_query(
            &base("https://indexer.test/api?q=theirs&maxage=7&pw=0&pred=0"),
            "SECRET",
            &IndexerQuery {
                limit: 100,
                offset: 0,
                categories: &[],
                search: &search,
            },
        )
        .expect("query");
        let pairs = pairs(&url);
        for (name, value) in [("q", "theirs"), ("maxage", "7"), ("pw", "0"), ("pred", "0")] {
            let values: Vec<&str> = pairs
                .iter()
                .filter(|(key, _)| key == name)
                .map(|(_, value)| value.as_str())
                .collect();
            assert_eq!(values, [value], "{name} in {url}");
        }
    }

    #[test]
    fn a_blank_search_term_is_not_sent() {
        let search = IndexerSearch {
            query: Some("   ".to_owned()),
            ..IndexerSearch::default()
        };
        let url = build_indexer_query(
            &base("https://indexer.test/api"),
            "SECRET",
            &IndexerQuery {
                limit: 100,
                offset: 0,
                categories: &[],
                search: &search,
            },
        )
        .expect("query");
        assert!(url.query_pairs().all(|(key, _)| key != "q"), "{url}");
    }
}
