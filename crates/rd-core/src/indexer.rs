//! Newznab indexers defined once, and the search parameters an indexer query carries
//! (RD-180-19, RD-180-20).
//!
//! An indexer is an address and an API key the person enters once and then searches from the
//! LinkGrabber or takes into an indexer subscription. The key lives in the vault like every
//! other credential: the row holds its `vault://` reference, a client is told only whether one
//! is stored.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use url::Url;
use utoipa::ToSchema;

use crate::IndexerId;

/// Shortest search term an indexer takes. Newznab servers answer a shorter one with error 201,
/// so it is refused here first, with a code the interface can translate.
pub const MIN_INDEXER_QUERY_CHARS: usize = 3;
/// Longest search term accepted.
pub const MAX_INDEXER_QUERY_CHARS: usize = 200;
/// Most days `maxage` may ask for; well beyond any Usenet retention.
pub const MAX_INDEXER_AGE_DAYS: u32 = 10_000;
/// Highest `pred` value passed on: the indexer defines 0, 1 and 2.
pub const MAX_INDEXER_PRETIME: u8 = 2;

/// A Newznab indexer the person defined once (RD-180-19).
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct Indexer {
    pub id: IndexerId,
    pub name: String,
    /// The API base address, e.g. `https://api.example.org/api`.
    #[schema(value_type = String, format = "uri")]
    pub url: Url,
    /// Vault reference of the API key. Never serialised.
    #[serde(skip_serializing)]
    #[schema(ignore)]
    pub secret_ref: Option<String>,
    /// Whether a key is stored, which is all a client is told about it.
    #[serde(default)]
    pub has_secret: bool,
    /// Categories a search asks for when it names none, sent as `cat`. Empty asks for all.
    #[serde(default)]
    pub categories: Vec<String>,
    /// A switched-off indexer is left out of "search every indexer".
    pub enabled: bool,
    /// How the LinkGrabber's search draws this indexer's hits (RD-190-16).
    #[serde(default)]
    pub list_style: IndexerListStyle,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// How the LinkGrabber's search draws one indexer's hits (RD-190-16).
///
/// Chosen per indexer, where it is defined, rather than in the result list: some indexers send
/// covers and film or series data with every hit, others nothing beyond the name, and only the
/// first kind is worth the taller row. Compact is the default because it is what every indexer
/// showed before the choice existed.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum IndexerListStyle {
    /// One line per hit: title, size, age, category, the password mark and the button.
    #[default]
    Compact,
    /// The same, plus a small cover beside the title and a line of the metadata the indexer sent.
    Detailed,
}

impl IndexerListStyle {
    /// The stored and serialised spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Compact => "compact",
            Self::Detailed => "detailed",
        }
    }

    /// Reads the stored spelling; anything unknown is compact, which is what the column defaults
    /// to and what a reader who knows no other style can always draw.
    #[must_use]
    pub fn from_stored(value: &str) -> Self {
        match value {
            "detailed" => Self::Detailed,
            _ => Self::Compact,
        }
    }
}

/// The search parameters an indexer query sends besides the address, the key and the paging
/// (RD-180-20).
///
/// Each one is sent only when it is set and the address does not already carry it: a saved
/// search copied out of the indexer's own RSS button keeps winning, as it always did. `query` is
/// the explicit search term and nothing else -- a subscription's title filter is never turned
/// into one (RD-106-10).
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(default)]
pub struct IndexerSearch {
    /// Sent as `q`, passed on as written, `!word` exclusions included; empty or at least three
    /// characters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    /// Sent as `maxage`: only releases posted within this many days.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_age_days: Option<u32>,
    /// Sent as `pw=2`: leave out releases the indexer marks as passworded.
    pub hide_passworded: bool,
    /// Sent as `pred`: the indexer's pre-time filter, 0, 1 or 2 as the indexer defines them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pretime: Option<u8>,
}

impl IndexerSearch {
    /// Whether nothing is set, which is how every subscription written before RD-180-20 reads.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }
}

#[cfg(test)]
mod tests {
    use super::{IndexerListStyle, IndexerSearch};

    #[test]
    fn a_list_style_round_trips_and_an_unknown_one_is_compact() {
        for style in [IndexerListStyle::Compact, IndexerListStyle::Detailed] {
            assert_eq!(IndexerListStyle::from_stored(style.as_str()), style);
            assert_eq!(
                serde_json::to_string(&style).expect("write"),
                format!("\"{}\"", style.as_str())
            );
        }
        assert_eq!(
            IndexerListStyle::from_stored("cards"),
            IndexerListStyle::Compact
        );
    }

    /// Every subscription row written before RD-180-20 carries `{}`.
    #[test]
    fn an_empty_document_is_an_empty_search() {
        let read: IndexerSearch = serde_json::from_str("{}").expect("parse");
        assert!(read.is_empty());
        assert_eq!(
            serde_json::to_string(&read).expect("write"),
            r#"{"hide_passworded":false}"#
        );
    }

    #[test]
    fn a_set_field_round_trips() {
        let search = IndexerSearch {
            query: Some("some show !german".to_owned()),
            max_age_days: Some(30),
            hide_passworded: true,
            pretime: Some(1),
        };
        let json = serde_json::to_string(&search).expect("write");
        let read: IndexerSearch = serde_json::from_str(&json).expect("read");
        assert_eq!(read, search);
        assert!(!read.is_empty());
    }
}
