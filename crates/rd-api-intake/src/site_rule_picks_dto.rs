//! Wire types of the pick board (RD-1170-03): a series page's entries, what was chosen, and
//! how far resolving it got.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// One entry of a listed page.
#[derive(Debug, Serialize, ToSchema)]
pub struct CollectorPickEntryResponse {
    /// The entry's place in the list; what `resolve` names.
    pub index: usize,
    /// The release name the rule read, or the page's own when it read none.
    pub label: Option<String>,
    /// What the rule's `pick` read from the entry: `season`, `episode`, `resolution`,
    /// `language`, `hoster`, or any other name the rule gives. A name it did not match is
    /// absent -- a season pack has no `episode`.
    pub attributes: BTreeMap<String, String>,
    /// `pending` (nothing fetched: never picked, stopped, or its captcha went unanswered --
    /// `code` says which), `queued`, `resolving`, `captcha` (being resolved and waiting for a
    /// person to solve its captcha in the broker), `done` or `failed`.
    pub state: String,
    /// Why the entry came back `pending` or ended `failed`, as a stable code.
    pub code: Option<String>,
    /// How many links it put into the LinkGrabber, once `done`.
    pub links: u32,
}

/// A page whose entries wait for a choice.
#[derive(Debug, Serialize, ToSchema)]
pub struct CollectorPickResponse {
    pub id: String,
    /// The name of the rule that listed it.
    pub rule: String,
    pub rule_id: String,
    /// The address that was crawled.
    pub address: String,
    /// The page's own name.
    pub package_name: Option<String>,
    /// When the page was listed, RFC 3339.
    pub created_at: String,
    /// Whether entries are being resolved right now.
    pub running: bool,
    /// The entries of the current round, and how many of them have finished: "3 of 8".
    pub total: u32,
    pub finished: u32,
    /// Whether the entry being resolved waits for a captcha.
    pub waiting_for_captcha: bool,
    pub entries: Vec<CollectorPickEntryResponse>,
}

/// Every page on the board, oldest first.
#[derive(Debug, Serialize, ToSchema)]
pub struct CollectorPicksResponse {
    pub pages: Vec<CollectorPickResponse>,
}

/// Lists one page's entries without resolving any.
#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateCollectorPickRequest {
    /// The page, an http or https address a two-stage site rule claims.
    pub address: String,
}

/// The entries to resolve.
#[derive(Debug, Deserialize, ToSchema)]
pub struct ResolveCollectorPickRequest {
    /// Indices as the page lists them. An entry already done, queued or being resolved is
    /// left as it is.
    pub entries: Vec<usize>,
}
