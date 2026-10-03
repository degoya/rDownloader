//! An interactive indexer search's answer, read for a person to choose from (RD-180-19).
//!
//! The same RSS document a subscription poll reads, through the same feed parser and the same
//! attribute gate; what differs is that nothing is filtered or archived here. Every hit goes back
//! to the interface as it came, and the person picks which ones become NZB imports.
//!
//! An indexer answers a refusal inside a `200 OK` as `<error code=… description=…/>`, so
//! [`indexer_refusal`] is asked before a body is parsed as results, and its code is what the
//! interface translates: `100` is a credential problem, `201` a query the indexer will not run.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use url::Url;

use crate::feed::parse_feed;

/// Most results one search page may ask an indexer for.
pub const MAX_SEARCH_LIMIT: u32 = 500;

/// The facts a detailed result row shows (RD-190-16), each under the name it is sent as and
/// the attribute names it is read from, the first one present winning.
///
/// A fixed few rather than the whole attribute block: every search answer carries them for every
/// hit, to the interface and to an MCP agent alike, and a plot of a thousand characters per hit
/// is not what a result list is read for.
const METADATA: &[(&str, &[&str])] = &[
    ("year", &["year", "imdbyear"]),
    ("genre", &["genre"]),
    ("imdbscore", &["imdbscore"]),
    ("language", &["language"]),
    ("resolution", &["resolution"]),
    ("description", &["imdbtagline", "imdbplot"]),
];

/// Longest description a hit carries, in characters; the row shows one line of it.
pub const MAX_DESCRIPTION_CHARS: usize = 300;

/// One hit of a search.
///
/// `download` is the address the indexer gave, which usually carries the API key; it is never
/// handed to a client as it is (see the API's search handler).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchHit {
    pub title: String,
    /// The indexer's own id for the release, when it gives one.
    pub guid: Option<String>,
    pub download: Url,
    pub size_bytes: Option<u64>,
    pub published_at: Option<DateTime<Utc>>,
    /// The first category the indexer assigned, as it named it (`5040`).
    pub category: Option<String>,
    /// How often the release was fetched, when the indexer says.
    pub grabs: Option<u64>,
    /// Whether the indexer marks the release as passworded. The password itself, when an
    /// indexer announces one, is never part of a hit.
    pub passworded: bool,
    /// Year, genre, IMDb score, language, resolution and a short description, as far as the
    /// indexer sent them (RD-190-16); the choice is `METADATA` in this module.
    pub metadata: BTreeMap<String, String>,
    /// The cover, only ever an absolute `http`/`https` address: it ends up in an `<img src>`.
    pub cover_url: Option<Url>,
}

/// One page of a search.
#[derive(Clone, Debug, Default)]
pub struct SearchPage {
    pub hits: Vec<SearchHit>,
    /// Entries the page carried, usable or not; a page shorter than the limit is the last one.
    pub announced: usize,
    /// The total the indexer reports in `<newznab:response total=…>`, when it does.
    pub total: Option<u64>,
}

/// Reads a search answer. Entries without a download address are counted but not returned.
pub fn parse_search(body: &str, base: &Url) -> anyhow::Result<SearchPage> {
    let feed = parse_feed(body, base)?;
    let announced = feed.items.len();
    let hits = feed
        .items
        .into_iter()
        .filter_map(|item| {
            let download = item.download_url().cloned()?;
            // The same gate a subscription's hits pass: a key inside an attribute is dropped,
            // and a real password is split off from the flag and not kept here.
            let kept = crate::retain_attributes(&item.attributes, item.size_bytes);
            Some(SearchHit {
                title: item.title,
                guid: item.id,
                download,
                size_bytes: kept
                    .attributes
                    .get("size")
                    .and_then(|value| value.parse().ok()),
                published_at: item.published_at,
                category: item.categories.first().cloned(),
                grabs: kept
                    .attributes
                    .get("grabs")
                    .and_then(|value| value.parse().ok()),
                passworded: kept.attributes.contains_key("password"),
                metadata: metadata_of(&kept.attributes),
                cover_url: cover_of(&kept.attributes),
            })
        })
        .collect();
    Ok(SearchPage {
        hits,
        announced,
        total: response_total(body),
    })
}

/// The facts of [`METADATA`] the attributes carry.
fn metadata_of(attributes: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    METADATA
        .iter()
        .filter_map(|(name, sources)| {
            let value = sources
                .iter()
                .find_map(|source| attributes.get(*source).filter(|value| !value.is_empty()))?;
            let value = if *name == "description" {
                value.chars().take(MAX_DESCRIPTION_CHARS).collect()
            } else {
                value.clone()
            };
            Some(((*name).to_owned(), value))
        })
        .collect()
}

/// The cover address, checked here again although the attribute gate already did: whatever
/// reaches a hit is drawn by the browser, so the rule stands where the field is filled.
fn cover_of(attributes: &BTreeMap<String, String>) -> Option<Url> {
    let url = Url::parse(attributes.get("coverurl")?).ok()?;
    (matches!(url.scheme(), "http" | "https") && url.host_str().is_some()).then_some(url)
}

/// A Newznab/Torznab `<error …>` document, as the indexer wrote it.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct IndexerRefusal {
    /// The `code` attribute as written.
    pub code: Option<String>,
    pub description: Option<String>,
}

impl IndexerRefusal {
    /// The code as a number, when it is one.
    #[must_use]
    pub fn number(&self) -> Option<u32> {
        self.code
            .as_deref()
            .and_then(|code| code.trim().parse().ok())
    }

    /// One line for a log or a stored error.
    #[must_use]
    pub fn message(&self) -> String {
        match (&self.description, &self.code) {
            (Some(description), Some(code)) => format!("{description} (code {code})"),
            (Some(description), None) => description.clone(),
            (None, Some(code)) => format!("code {code}"),
            (None, None) => "unspecified error".to_owned(),
        }
    }
}

/// The refusal a body is, if it is one.
///
/// Deliberately a string scan rather than a parse: the error document is tiny, and running the
/// full parser first would mean a malformed *error* was reported as a malformed feed.
#[must_use]
pub fn indexer_refusal(body: &str) -> Option<IndexerRefusal> {
    let start = body.find("<error")?;
    let rest = &body[start..];
    let end = rest.find('>')?;
    let tag = &rest[..end];
    Some(IndexerRefusal {
        code: extract_attribute(tag, "code"),
        description: extract_attribute(tag, "description"),
    })
}

/// The `total` of `<newznab:response offset="0" total="1234"/>`, when the answer carries one.
fn response_total(body: &str) -> Option<u64> {
    let start = body.find(":response")?;
    let rest = &body[start..];
    let end = rest.find('>')?;
    extract_attribute(&rest[..end], "total")?
        .trim()
        .parse()
        .ok()
}

fn extract_attribute(tag: &str, name: &str) -> Option<String> {
    let marker = format!("{name}=\"");
    let start = tag.find(&marker)? + marker.len();
    let rest = &tag[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_owned())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{indexer_refusal, parse_search};
    use url::Url;

    fn base() -> Url {
        "https://indexer.test/api".parse().expect("url")
    }

    const ANSWER: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
    <rss xmlns:newznab="http://www.newznab.com/DTD/2010/feeds/attributes/">
      <channel>
        <newznab:response offset="0" total="1234"/>
        <item>
          <title>Some.Show.S01E01.1080p.WEB</title>
          <guid>abc</guid>
          <pubDate>Tue, 29 Sep 2026 10:00:00 +0000</pubDate>
          <enclosure url="https://indexer.test/getnzb/abc.nzb&amp;apikey=SECRET" length="1500"
                     type="application/x-nzb"/>
          <newznab:attr name="category" value="5040"/>
          <newznab:attr name="grabs" value="12"/>
          <newznab:attr name="password" value="1"/>
          <newznab:attr name="coverurl" value="https://indexer.test/covers/abc.jpg"/>
          <newznab:attr name="imdbyear" value="2024"/>
          <newznab:attr name="genre" value="Drama"/>
          <newznab:attr name="imdbscore" value="7.5"/>
          <newznab:attr name="resolution" value="1920x1080"/>
          <newznab:attr name="imdbplot" value="A plot."/>
          <newznab:attr name="imdbtagline" value="A tagline."/>
          <newznab:attr name="tvdbid" value="12345"/>
        </item>
        <item>
          <title>Another.Release</title>
          <guid>def</guid>
          <enclosure url="https://indexer.test/getnzb/def.nzb" type="application/x-nzb"/>
          <newznab:attr name="size" value="2048"/>
          <newznab:attr name="password" value="hunter2"/>
          <newznab:attr name="coverurl" value="/covers/relative.jpg"/>
        </item>
        <item><title>No address</title><guid>ghi</guid></item>
      </channel>
    </rss>"#;

    #[test]
    fn a_search_answer_becomes_hits_with_the_columns_the_list_shows() {
        let page = parse_search(ANSWER, &base()).expect("page");
        assert_eq!(page.announced, 3);
        assert_eq!(page.total, Some(1234));
        assert_eq!(
            page.hits.len(),
            2,
            "the entry without an address is left out"
        );
        let first = &page.hits[0];
        assert_eq!(first.title, "Some.Show.S01E01.1080p.WEB");
        assert_eq!(first.guid.as_deref(), Some("abc"));
        assert_eq!(first.size_bytes, Some(1500));
        assert_eq!(first.category.as_deref(), Some("5040"));
        assert_eq!(first.grabs, Some(12));
        assert!(first.passworded);
        assert!(first.published_at.is_some());
        let second = &page.hits[1];
        assert_eq!(second.size_bytes, Some(2048));
        // An announced password marks the hit and goes no further.
        assert!(second.passworded);
        assert!(!format!("{second:?}").contains("hunter2"));
    }

    /// RD-190-16: the detailed row's facts and cover come out of the attribute block, a fixed
    /// few of them, and a cover that is not an absolute http(s) address never does.
    #[test]
    fn a_hit_carries_its_metadata_and_only_an_absolute_cover() {
        let page = parse_search(ANSWER, &base()).expect("page");
        let first = &page.hits[0];
        assert_eq!(
            first.cover_url.as_ref().map(url::Url::as_str),
            Some("https://indexer.test/covers/abc.jpg")
        );
        let facts: Vec<(&str, &str)> = first
            .metadata
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect();
        assert_eq!(
            facts,
            [
                ("description", "A tagline."),
                ("genre", "Drama"),
                ("imdbscore", "7.5"),
                ("resolution", "1920x1080"),
                ("year", "2024"),
            ]
        );
        let second = &page.hits[1];
        assert_eq!(second.cover_url, None, "a relative cover is dropped");
        assert!(second.metadata.is_empty());
    }

    #[test]
    fn a_long_description_is_cut_on_a_character() {
        let plot = "\u{e4}".repeat(super::MAX_DESCRIPTION_CHARS + 50);
        let attributes: BTreeMap<String, String> =
            [("imdbplot".to_owned(), plot)].into_iter().collect();
        let metadata = super::metadata_of(&attributes);
        assert_eq!(
            metadata["description"].chars().count(),
            super::MAX_DESCRIPTION_CHARS
        );
    }

    #[test]
    fn a_cover_needs_an_http_scheme_and_a_host() {
        for bad in [
            "data:image/png;base64,AAAA",
            "javascript:alert(1)",
            "file:///etc/passwd",
        ] {
            let attributes: BTreeMap<String, String> = [("coverurl".to_owned(), bad.to_owned())]
                .into_iter()
                .collect();
            assert_eq!(super::cover_of(&attributes), None, "{bad}");
        }
        let attributes: BTreeMap<String, String> =
            [("coverurl".to_owned(), "http://covers.test/a.jpg".to_owned())]
                .into_iter()
                .collect();
        assert!(super::cover_of(&attributes).is_some());
    }

    #[test]
    fn a_refusal_names_its_code_and_description() {
        let body = r#"<?xml version="1.0"?><error code="201" description="Incorrect parameter (search too short)"/>"#;
        let refusal = indexer_refusal(body).expect("refusal");
        assert_eq!(refusal.number(), Some(201));
        assert_eq!(
            refusal.message(),
            "Incorrect parameter (search too short) (code 201)"
        );
        assert!(indexer_refusal(ANSWER).is_none());
        let bare = indexer_refusal("<error/>").expect("refusal");
        assert_eq!(bare.number(), None);
        assert_eq!(bare.message(), "unspecified error");
    }
}
