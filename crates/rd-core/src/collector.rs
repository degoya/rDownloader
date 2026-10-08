use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use url::Url;
use utoipa::ToSchema;

use crate::{
    BatchId, ByteCount, CandidateId, CategoryId, CollectorPackageId, DownloadPriority, LinkStatus,
    PluginLinkCheck, PostprocessLevel, ResolverRoute,
};

#[path = "collector_categories.rs"]
mod categories;
#[path = "collector_mirror.rs"]
mod mirror;

pub use categories::{
    Category, CategoryRule, CategoryRuleNameTarget, HotFolderConfig, HotFolderExecutor, ImportMode,
    StorageRootConfig,
};
pub use mirror::{CandidateMirror, MirrorFacet, MirrorHint, MirrorPreference, MirrorSource};

/// Origin of a batch submitted to the LinkGrabber.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum IngressSource {
    Manual,
    Clipboard,
    ClickAndLoad,
    Api,
    Nzb,
    HotFolder,
    /// Submitted by the browser extension (context menu / popup).
    BrowserExtension,
    /// An intercepted regular browser download handed over by the extension.
    BrowserDownload,
    /// Discovered by a subscription poll (RD-080-07). A routing rule can target it, so a
    /// feed's items can be sent somewhere other than manually pasted links.
    Subscription,
}

/// Link analysis status.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum LinkCandidateState {
    /// Claimed by the enqueue handler (short-lived lock).
    Resolving,
    /// Being probed by the link check service.
    Checking,
    Online,
    Offline,
    Unsupported,
    /// The address answered, and what it answered with is not a file (RD-110-07).
    ///
    /// Deliberately its own state rather than a shade of `Offline` or of `Unsupported`:
    /// `Offline` says the hoster reports the file as gone, `Unsupported` says nothing was
    /// checked at all, and both are still worth a try. This one says the check ran, reached
    /// the address and got a page back -- markup, or a text body too small to be the payload.
    /// Queueing it can only store somebody's error page under a file's name, so it is the one
    /// state the review list offers no way out of.
    Unresolvable,
    Duplicate,
    Enqueued,
    Error,
}

impl LinkCandidateState {
    /// The states a link may be queued from.
    ///
    /// The rule is that only a link the application is *currently working on* may be refused:
    /// `Resolving` and `Checking` are short-lived locks, and `Enqueued` has already been handed
    /// on. Everything else is the user's call.
    ///
    /// That includes the three states where nothing good is known about the link.
    /// `Offline` was always queueable — a hoster that reports a file as gone is often simply
    /// wrong, and `LinkStatus::Unknown` is stored as `Online` for the same reason. `Unsupported`
    /// followed, because the user may still start a link no resolver claims. `Error` is the last
    /// of them and was the odd one out: a check that fails outright — an account whose sign-in
    /// does not work, say — says something about the *check*, not about the file, so refusing
    /// the link left it worse off than one reported gone.
    ///
    /// [`Self::Unresolvable`] is the one state deliberately left out, and it is the exception
    /// that shows the rule: every state in the list means nothing good is *known*, while that
    /// one means something bad is -- the address was reached and answered with a page
    /// (RD-110-07).
    pub const ENQUEUEABLE: &'static [Self] = &[
        Self::Online,
        Self::Duplicate,
        Self::Offline,
        Self::Unsupported,
        Self::Error,
    ];

    /// Whether a link in this state may be queued.
    #[must_use]
    pub fn is_enqueueable(self) -> bool {
        Self::ENQUEUEABLE.contains(&self)
    }
}

/// One persisted LinkGrabber submission.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct CollectorBatch {
    pub id: BatchId,
    pub source: IngressSource,
    pub source_label: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// An analyzed link awaiting review or enqueue.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct LinkCandidate {
    pub id: CandidateId,
    pub batch_id: BatchId,
    #[schema(value_type = String, format = "uri")]
    pub url: Url,
    pub state: LinkCandidateState,
    pub file_name: Option<String>,
    /// Whether `file_name` came from the source rather than from the link's address.
    ///
    /// Intake substitutes the address's last segment when the source names none, and the two
    /// look alike afterwards; only this says which it was.
    #[serde(default = "crate::default_true")]
    pub file_name_declared: bool,
    pub size: Option<ByteCount>,
    pub provider: Option<String>,
    pub category_id: Option<CategoryId>,
    #[serde(default)]
    pub priority: DownloadPriority,
    pub route: Option<ResolverRoute>,
    pub error: Option<String>,
    /// Stable code for `error`, translated by the interface as `server.codes.<code>`.
    ///
    /// The text stays beside it as the English fallback, for a row written before the code
    /// existed and for a message a plugin worded itself. A candidate that carries no code
    /// reaches the reader as that text, exactly as every candidate used to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    /// LinkGrabber package the link is grouped into.
    pub package_id: Option<CollectorPackageId>,
    /// Manual order inside the package.
    #[serde(default)]
    pub position: i64,
    /// When the last online check finished.
    pub checked_at: Option<DateTime<Utc>>,
    /// When a provider last said it holds this file in its own cache (RD-120-36).
    ///
    /// Set only by a check that answered [`LinkStatus::Cached`], and cleared by every check
    /// that did not, so it is always the time of the latest check. A cache
    /// changes without telling anybody, so the interface shows the time with it and treats the
    /// value as a measurement, never as a promise. The candidate's state stays `Online`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_at: Option<DateTime<Utc>>,
    /// Which provider gave the cache answer `cached_at` records, by its slug (RD-130-11).
    ///
    /// Set and cleared together with `cached_at`, never without it. `None` next to a time is
    /// an answer stamped before the column existed, and the interface then names nobody
    /// rather than guessing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_by: Option<String>,
    /// When the link was added to the collector.
    pub created_at: DateTime<Utc>,
    /// Extractor metadata and variant choice for media links.
    #[serde(default)]
    pub media: Option<crate::MediaInfo>,
    /// Request metadata of an intercepted browser download; `None` for every other source.
    #[serde(default)]
    pub request: Option<crate::CapturedRequest>,
    /// Consent a person gave for replaying this request, if any.
    ///
    /// The encrypted body's `vault://` reference deliberately has **no** field here: it
    /// lives only in `link_candidates.replay_body_ref`, so it cannot be serialized into a
    /// REST or SSE payload by widening this struct.
    #[serde(default)]
    pub replay_consent: Option<crate::ReplayConsent>,
    /// Bounded torrent summary; the full file tree is fetched per candidate so a list
    /// response never carries thousands of entries.
    #[serde(default)]
    pub torrent: Option<crate::TorrentCandidateSummary>,
    /// Bounded remote-directory summary for `ftp`/`sftp`/`webdav` links; the full listing
    /// is fetched per candidate for the same reason as the torrent file tree.
    #[serde(default)]
    pub listing: Option<crate::RemoteListingSummary>,
    /// Stored login the online check used to reach the server; carried to the queue row so
    /// the transfer authenticates the same way the probe did.
    #[serde(default)]
    pub remote_credential_id: Option<crate::RemoteCredentialId>,
    /// Cookie/authentication profile this link is queued with (RD-080-04).
    ///
    /// Chosen before queueing so the first attempt at a private page already carries the
    /// right session, instead of failing once and being corrected afterwards. Defaults to
    /// [`crate::AuthProfileSelection::Auto`], which is what every link did before.
    #[serde(default)]
    pub auth_profile: crate::AuthProfileSelection,
    /// Fields an enricher plugin added (RD-090-14).
    ///
    /// Beside the core fields, never merged into them: what a plugin contributed stays
    /// distinguishable from what the application resolved itself, so it can be shown with its
    /// source and so a core field can never be quietly replaced.
    #[serde(default)]
    pub enrichment: Vec<EnrichmentField>,
    /// Which mirror group this link belongs to, if any (RD-110-18).
    ///
    /// `None` is the ordinary case: a link nothing else points at the same file with is a
    /// download, not a mirror of anything. See [`CandidateMirror`] for why this is separate
    /// from [`LinkCandidateState::Duplicate`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mirror: Option<CandidateMirror>,
    /// Whether the vault holds the fragment this link's address arrived with (RD-110-38).
    ///
    /// A flag, not the reference and never the fragment itself. The `vault://` reference
    /// lives only in `link_candidates.secret_fragment_ref`, for the same reason
    /// `replay_body_ref` does: a column cannot be serialized into a REST or SSE payload by
    /// somebody widening this struct. What a reader is entitled to know is that a key is
    /// being kept for this link, and that is exactly what this says.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub secret_fragment: bool,
    /// The mirrors a Metalink parser stated for this link (RD-150-03), redacted and in the
    /// order the transfer will try them, so they can be reviewed before the link is queued.
    /// Empty for a link without a source set.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<crate::CandidateSource>,
}

/// What the online check leaves on a candidate: the English sentence, and the stable code
/// the interface translates it by.
///
/// Free text alone was the defect behind RD-109-43. `Some("Check result missing")` was passed
/// for every candidate of a provider batch, so the sentence stood both for a URL the plugin
/// never spoke about and for a URL the plugin answered `Unknown` about — and it was wrong in
/// the second case, which is the common one. A code cannot be reused by accident in the same
/// way: naming the situation is what building the message now costs.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct CandidateMessage {
    /// Stable identifier, e.g. `collector.check_unknown`.
    pub code: Option<String>,
    /// English wording, shown when no catalogue translates the code.
    pub text: String,
}

impl CandidateMessage {
    /// A message with a stable code.
    #[must_use]
    pub fn coded(code: &str, text: impl Into<String>) -> Self {
        Self {
            code: Some(code.to_owned()),
            text: text.into(),
        }
    }

    /// A message without one, for text somebody else worded — a plugin, or an SSH host key
    /// fingerprint that is only useful spelled out.
    #[must_use]
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            code: None,
            text: text.into(),
        }
    }
}

impl From<&crate::Failure> for CandidateMessage {
    /// Keeps the code a failure already carries instead of flattening it to prose.
    fn from(failure: &crate::Failure) -> Self {
        Self {
            code: failure.code.clone(),
            text: failure.message.clone(),
        }
    }
}

/// One field an enricher plugin contributed to a link.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct EnrichmentField {
    /// Namespaced by the plugin, e.g. `sponsorblock.sponsor_seconds`.
    pub name: String,
    pub value: String,
    /// Which plugin said so. Shown next to the value: a field whose source is invisible is
    /// indistinguishable from something the application knows itself.
    pub plugin_id: String,
    /// When it was looked up, so a stale value is recognisable as one.
    pub fetched_at: DateTime<Utc>,
}

/// A group of links in the LinkGrabber that becomes one download package.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct CollectorPackage {
    pub id: CollectorPackageId,
    pub batch_id: BatchId,
    pub name: String,
    /// `true` while the name was derived automatically (regrouping may rename it).
    pub auto_named: bool,
    pub category_id: Option<CategoryId>,
    pub priority: DownloadPriority,
    pub position: i64,
    pub has_password: bool,
    /// The stored archive password, in clear; see `DownloadPackage::password` (RD-104-04).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    pub created_at: DateTime<Utc>,
    /// Explicit post-processing level for the enqueued package; `None` inherits.
    #[serde(default)]
    pub postprocess_level: Option<PostprocessLevel>,
    #[serde(default)]
    pub script: Option<String>,
    /// The name the package gets in the queue when the package-name rules change it
    /// (RD-1140-05); `None` when it keeps `name` as it is. Only a name the LinkGrabber derived
    /// itself is tidied: one somebody stated or renamed stays.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue_name: Option<String>,
}

/// Which table a LinkGrabber entry belongs to.
///
/// The list shows two kinds of row in one manual order, and an id alone does not say which: both
/// are UUIDs, and a collector package id would silently match nothing in `nzb_imports` (and the
/// other way round), producing an order that writes positions for rows nobody named.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum GrabberEntryKind {
    Collector,
    Nzb,
}

/// One entry of the LinkGrabber's single manual order.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize, ToSchema)]
pub struct GrabberEntryRef {
    pub kind: GrabberEntryKind,
    /// The row's id; which table it belongs to is [`Self::kind`], never guessed from the value.
    #[schema(value_type = String, format = Uuid)]
    pub id: uuid::Uuid,
}

/// Result of probing one link without downloading it.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct LinkCheckResult {
    #[schema(value_type = String, format = "uri")]
    pub url: Url,
    pub status: LinkStatus,
    pub file_name: Option<String>,
    pub size: Option<ByteCount>,
    /// Media metadata when the link was probed by the media provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media: Option<crate::MediaInfo>,
}

/// A resolver's answer as the service stores it: no plugin reports media metadata.
impl From<PluginLinkCheck> for LinkCheckResult {
    fn from(check: PluginLinkCheck) -> Self {
        Self {
            url: check.url,
            status: check.status,
            file_name: check.file_name,
            size: check.size,
            media: None,
        }
    }
}

/// The address a link candidate is stored under: the pasted one, without its fragment.
///
/// **A fragment never survives into a row (RD-109-32).** A share password reaches a crawler as
/// the fragment of the pasted address (RD-108-07), and for a link a crawler claims that
/// password is encrypted into an auth profile while the address is stored bare. A link no
/// crawler claims took the other path and kept its fragment, so one wrong letter in a host
/// name was enough to write the password into `link_candidates.url` in clear text and print it
/// in every LinkGrabber row.
///
/// Nothing distinguishes a password in a fragment from an anchor name, so the rule is the
/// blunt one rather than a guess — and it costs almost nothing, because a fragment is the one
/// part of a URL that is never sent to a server (RFC 3986 §3.5). The plugins that do read a
/// fragment read it before this point: a crawler is handed the pasted address whole.
///
/// The fragment is dropped, not stored encrypted. The vault needs an owner, and an unclaimed
/// link gives none: no username, no verified scope, nothing that could ever read the secret
/// back. `rd_plugin_ext::share_login` derives both from what a crawler *answered*.
#[must_use]
pub fn candidate_url(url: &Url) -> Url {
    split_candidate_url(url, false).0
}

/// The same rule, plus the one case in which the fragment is kept instead of thrown away.
///
/// **The fragment goes into the vault, never into the row** (RD-110-38). A provider that
/// encrypts on the client keeps the file key out of its own reach by putting it in the
/// fragment of the link somebody was given: MEGA is the first, and every provider of that
/// shape arrives at the same wall. Dropping it made such a link unresolvable; keeping it in
/// `link_candidates.url` would write a decryption key into every LinkGrabber row, which is
/// the leak RD-109-32 closed. So the address is shortened exactly as before, and the
/// fragment is handed back to the caller to put away under a reference.
///
/// `fragment_is_secret` is not decided here and there is no host list in this crate. The
/// caller asks the provider registry, which is filled solely from installed plugin
/// manifests: a provider *declares* that its key travels in the fragment, and without that
/// declaration this behaves exactly like [`candidate_url`]. No service is a special case in
/// the code.
///
/// An empty fragment yields `None`: `…/a#` carries nothing to keep, and storing an empty
/// secret is refused by the vault anyway.
#[must_use]
pub fn split_candidate_url(url: &Url, fragment_is_secret: bool) -> (Url, Option<String>) {
    let Some(fragment) = url.fragment() else {
        return (url.clone(), None);
    };
    let kept = if fragment_is_secret && !fragment.is_empty() {
        Some(fragment.to_owned())
    } else {
        None
    };
    let mut stored = url.clone();
    stored.set_fragment(None);
    (stored, kept)
}

#[cfg(test)]
#[path = "collector_candidate_url_tests.rs"]
mod candidate_url_tests;

#[cfg(test)]
#[path = "collector_enqueueable_tests.rs"]
mod enqueueable_tests;
