use serde::{Deserialize, Serialize};
use url::Url;
use utoipa::ToSchema;

use crate::ByteCount;

/// Availability reported by an online check.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum LinkStatus {
    Online,
    Offline,
    Unknown,
    /// The address answered and what came back is not file content (RD-110-07).
    ///
    /// Told apart from `Unknown` on purpose. `Unknown` is "the check reached no conclusion",
    /// which is why it is stored as `Online` and stays queueable -- a hoster that refuses to
    /// be checked is still worth downloading from. This is a conclusion: the response was
    /// read and `rd_http::ProbeResult::looks_downloadable` rejected it.
    Unresolvable,
    /// The provider holds the file in its own cache at the moment of the check (RD-120-36).
    ///
    /// A stronger and shorter-lived statement than `Online`: the file exists *and* can be
    /// handed over at once, until the provider evicts it without telling anybody. Stored as
    /// an `Online` candidate with `cached_at` set to the time of the check, so the
    /// interface can say when it was measured.
    Cached,
}

/// What a resolver's link check reports for one address.
///
/// The plugin side of `rd_core::LinkCheckResult`, which adds the media metadata only the
/// service's own media probe fills; `rd-core` converts one into the other (RD-1190-08).
#[derive(Clone, Debug)]
pub struct PluginLinkCheck {
    pub url: Url,
    pub status: LinkStatus,
    pub file_name: Option<String>,
    pub size: Option<ByteCount>,
}
