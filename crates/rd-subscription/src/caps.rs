//! Newznab/Torznab capability discovery (`t=caps`) (RD-080-11).
//!
//! An indexer describes itself: which search types it answers, what it caps `limit` at, and
//! the category tree it files releases under. Two things depend on that:
//!
//! * **The category map.** A category id like `5030` means nothing on its own, and it means
//!   something *different* on the next indexer. Fetching the tree is the only way to offer a
//!   list of names instead of asking someone to type numbers they would have to look up.
//! * **The test action.** `t=caps` is the cheapest request that proves the address and the
//!   API key are both right, so it doubles as "test this indexer" without pulling results.
//!
//! Parsing only, and XXE-safe by the same rule as the feed parser: a DOCTYPE with an
//! internal subset is refused rather than trusted to `quick_xml`.

use anyhow::{Result, bail};
use quick_xml::{Reader, events::Event};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Largest caps document accepted. A big indexer's tree is tens of kilobytes.
pub const MAX_CAPS_BYTES: usize = 2 * 1024 * 1024;
/// Most categories taken from one document.
pub const MAX_CAPS_CATEGORIES: usize = 500;

/// One category an indexer files releases under.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct IndexerCategory {
    /// The numeric id used in `cat=`; a string because it is an opaque token, not a number
    /// we do arithmetic on.
    pub id: String,
    pub name: String,
    /// Id of the parent category for a subcategory; `None` for a top-level one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
}

impl IndexerCategory {
    /// `TV / HD` for a subcategory, `TV` for a top-level one.
    #[must_use]
    pub fn label(&self, all: &[Self]) -> String {
        match self
            .parent_id
            .as_deref()
            .and_then(|parent| all.iter().find(|entry| entry.id == parent))
        {
            Some(parent) => format!("{} / {}", parent.name, self.name),
            None => self.name.clone(),
        }
    }
}

/// What an indexer says it can do.
#[derive(Clone, Debug, Default, Deserialize, Serialize, ToSchema)]
pub struct IndexerCaps {
    /// Server name, when it gives one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server: Option<String>,
    /// Largest `limit` the indexer accepts, when it states one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit_max: Option<u32>,
    /// Search types the indexer answers, e.g. `search`, `tv-search`, `movie-search`.
    pub searching: Vec<String>,
    pub categories: Vec<IndexerCategory>,
}

impl IndexerCaps {
    /// Whether the indexer answers a given `t=` value.
    #[must_use]
    pub fn supports(&self, search_type: &str) -> bool {
        self.searching
            .iter()
            .any(|entry| entry.eq_ignore_ascii_case(search_type))
    }
}

/// Parses a `t=caps` response.
pub fn parse_caps(body: &str) -> Result<IndexerCaps> {
    if body.len() > MAX_CAPS_BYTES {
        bail!("caps document exceeds the size limit");
    }
    // An indexer answers a bad key here too, and with the same error document.
    if let Some(message) = crate::indexer::indexer_error(body) {
        bail!("indexer refused the capability request: {message}");
    }

    let mut reader = Reader::from_str(body);
    reader.config_mut().trim_text(true);
    let mut caps = IndexerCaps::default();
    // The id of the `<category>` currently open, so its `<subcat>` children know their
    // parent. Newznab nests exactly one level.
    let mut parent: Option<String> = None;
    let mut depth = 0_usize;

    loop {
        match reader.read_event()? {
            Event::Eof => {
                if depth != 0 {
                    bail!("caps document ended inside an element");
                }
                break;
            }
            Event::DocType(doctype) => {
                let value = doctype.decode()?;
                let trimmed = value.trim();
                if trimmed.contains(['[', ']']) || trimmed.to_ascii_lowercase().contains("<!entity")
                {
                    bail!("caps doctype declares an internal subset");
                }
            }
            Event::Start(element) => {
                let name = local_name(element.name().as_ref());
                apply(&mut caps, &mut parent, &name, &element, true);
                depth += 1;
            }
            Event::Empty(element) => {
                let name = local_name(element.name().as_ref());
                apply(&mut caps, &mut parent, &name, &element, false);
            }
            Event::End(element) => {
                if local_name(element.name().as_ref()) == "category" {
                    parent = None;
                }
                depth = depth.saturating_sub(1);
            }
            _ => {}
        }
    }
    Ok(caps)
}

fn apply(
    caps: &mut IndexerCaps,
    parent: &mut Option<String>,
    name: &str,
    element: &quick_xml::events::BytesStart<'_>,
    is_open: bool,
) {
    let attribute = |key: &str| -> Option<String> {
        element
            .attributes()
            .filter_map(Result::ok)
            .find(|value| local_name(value.key.as_ref()) == key)
            .map(|value| String::from_utf8_lossy(value.value.as_ref()).into_owned())
    };
    match name {
        "server" => caps.server = attribute("title").or_else(|| attribute("appversion")),
        "limits" => caps.limit_max = attribute("max").and_then(|value| value.parse().ok()),
        "category" => {
            let Some(id) = attribute("id") else { return };
            if caps.categories.len() < MAX_CAPS_CATEGORIES {
                caps.categories.push(IndexerCategory {
                    name: attribute("name").unwrap_or_else(|| id.clone()),
                    id: id.clone(),
                    parent_id: None,
                });
            }
            if is_open {
                *parent = Some(id);
            }
        }
        "subcat" => {
            let Some(id) = attribute("id") else { return };
            if caps.categories.len() < MAX_CAPS_CATEGORIES {
                caps.categories.push(IndexerCategory {
                    name: attribute("name").unwrap_or_else(|| id.clone()),
                    id,
                    parent_id: parent.clone(),
                });
            }
        }
        // The searching block lists one element per type, each with `available="yes|no"`.
        "search" | "tv-search" | "movie-search" | "audio-search" | "book-search" => {
            let available = attribute("available")
                .map(|value| value.eq_ignore_ascii_case("yes"))
                .unwrap_or(true);
            if available {
                caps.searching.push(name.to_owned());
            }
        }
        _ => {}
    }
}

fn local_name(raw: &[u8]) -> String {
    String::from_utf8_lossy(raw).to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::{MAX_CAPS_CATEGORIES, parse_caps};

    const CAPS: &str = r#"<?xml version="1.0"?>
    <caps>
      <server version="1.0" title="Example Indexer"/>
      <limits max="100" default="50"/>
      <searching>
        <search available="yes" supportedParams="q"/>
        <tv-search available="yes" supportedParams="q,season,ep"/>
        <movie-search available="no" supportedParams="q"/>
      </searching>
      <categories>
        <category id="5000" name="TV">
          <subcat id="5030" name="SD"/>
          <subcat id="5040" name="HD"/>
        </category>
        <category id="2000" name="Movies">
          <subcat id="2040" name="HD"/>
        </category>
      </categories>
    </caps>"#;

    #[test]
    fn the_category_tree_is_flattened_with_its_parents() {
        let caps = parse_caps(CAPS).expect("caps");
        assert_eq!(caps.categories.len(), 5);
        let hd_tv = caps
            .categories
            .iter()
            .find(|category| category.id == "5040")
            .expect("5040");
        assert_eq!(hd_tv.parent_id.as_deref(), Some("5000"));
        // The label is what the UI shows, and it has to disambiguate: two categories are
        // both called "HD".
        assert_eq!(hd_tv.label(&caps.categories), "TV / HD");
        let hd_movies = caps
            .categories
            .iter()
            .find(|category| category.id == "2040")
            .expect("2040");
        assert_eq!(hd_movies.label(&caps.categories), "Movies / HD");
        assert_ne!(
            hd_tv.label(&caps.categories),
            hd_movies.label(&caps.categories)
        );
    }

    #[test]
    fn a_top_level_category_has_no_parent() {
        let caps = parse_caps(CAPS).expect("caps");
        let tv = caps
            .categories
            .iter()
            .find(|category| category.id == "5000")
            .expect("5000");
        assert!(tv.parent_id.is_none());
        assert_eq!(tv.label(&caps.categories), "TV");
    }

    #[test]
    fn only_the_search_types_the_indexer_offers_are_reported() {
        let caps = parse_caps(CAPS).expect("caps");
        assert!(caps.supports("search"));
        assert!(caps.supports("tv-search"));
        // `available="no"` means it will refuse the query, so offering it would be a lie.
        assert!(!caps.supports("movie-search"));
        assert!(!caps.supports("book-search"));
    }

    #[test]
    fn the_server_name_and_limit_are_read() {
        let caps = parse_caps(CAPS).expect("caps");
        assert_eq!(caps.server.as_deref(), Some("Example Indexer"));
        assert_eq!(caps.limit_max, Some(100));
    }

    #[test]
    fn a_refused_capability_request_is_an_error_not_empty_caps() {
        // Same trap as the search query: the indexer answers 200 with an error document, and
        // empty caps would look like an indexer that simply has no categories.
        let refused =
            r#"<?xml version="1.0"?><error code="100" description="Incorrect user credentials"/>"#;
        let error = parse_caps(refused).expect_err("should fail");
        assert!(error.to_string().contains("Incorrect user credentials"));
    }

    #[test]
    fn a_doctype_with_an_internal_subset_is_refused() {
        let bomb = r#"<!DOCTYPE caps [<!ENTITY lol "haha">]><caps/>"#;
        assert!(parse_caps(bomb).is_err());
    }

    #[test]
    fn a_truncated_document_is_an_error() {
        assert!(parse_caps("<caps><categories><category id=\"1\">").is_err());
    }

    #[test]
    fn the_category_count_is_bounded() {
        let entries = (0..MAX_CAPS_CATEGORIES + 50)
            .map(|index| format!("<category id=\"{index}\" name=\"c{index}\"/>"))
            .collect::<String>();
        let caps =
            parse_caps(&format!("<caps><categories>{entries}</categories></caps>")).expect("caps");
        assert_eq!(caps.categories.len(), MAX_CAPS_CATEGORIES);
    }
}
