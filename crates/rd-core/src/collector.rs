use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use url::Url;
use utoipa::ToSchema;

use crate::{
    BatchId, ByteCount, CandidateId, CaptureAgentId, CategoryId, CategoryRuleId,
    CollectorPackageId, DownloadPriority, HotFolderId, PostprocessLevel, ResolverRoute,
    StorageRootId,
};

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

/// Where a mirror group came from (RD-110-18).
///
/// Kept beside the group because the three are not equally strong, and the interface has to
/// be able to say so: a release page that states the five links are one file is a fact, two
/// links agreeing on name *and* size is evidence, and a bare name in common is a suggestion
/// somebody may want to overrule.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MirrorSource {
    /// The source said so — a site rule whose page is one release, or a crawler that named
    /// the group. Ordered first because it outranks anything derived from a file name.
    Declared,
    /// Same file name and a size that agrees, after the online check.
    NameAndSize,
    /// Same file name and nothing to corroborate it. A proposal, not a fact.
    Name,
}

/// What a source stated about one link's place among mirrors.
///
/// The input side of [`CandidateMirror`]: it travels with a link into the collector and is
/// stored as it arrived, so regrouping after an online check never loses what the page said.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct MirrorHint {
    /// The key the source used for "these links are the same file". Only compared with the
    /// keys of the same package, so a rule may use anything stable within one page.
    pub group: String,
    /// The quality the source named for this link, e.g. `1080p`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quality: Option<String>,
    /// The language the source named for this link, e.g. `German`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
}

/// What a person prefers when a mirror group offers a choice (RD-110-19).
///
/// Three facets, each optional and each combined with the others by conjunction. It is a
/// *preference*, not a rule: it decides which member of a group is the chosen one, it never
/// deletes a mirror and it never overrules a member somebody pinned by hand. Stored on the
/// server under its own settings key, so it applies to the next package that arrives and to
/// every package after a restart.
///
/// The three are compared case-insensitively against the values [`CandidateMirror`] carries,
/// which is what the source named or what the release name spells; the hoster is the link's
/// host without a leading `www.`.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(default)]
pub struct MirrorPreference {
    /// e.g. `1080p`. Matched against [`CandidateMirror::quality`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quality: Option<String>,
    /// e.g. `German`. Matched against [`CandidateMirror::language`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// e.g. `rapidgator.net`. Matched against the link's host.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hoster: Option<String>,
    /// Hosters the LinkGrabber hides (RD-130-21), each as [`Self::hoster`] spells one.
    ///
    /// Not a fourth facet. A facet narrows the list to what matches it; this takes named
    /// hosters out of it, several at once. Inside a mirror group it only ranks: a member at a
    /// hidden hoster is never the chosen one while a member at a shown hoster exists, and it
    /// stays in the group as the fallback the queue switches to. Stored with the facets because
    /// it is the same kind of standing decision and has to survive a restart the same way.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub hidden_hosters: Vec<String>,
}

impl MirrorPreference {
    /// Whether nothing is preferred, in which case the first member stays the chosen one.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.facets().next().is_none() && self.hidden_hosters.is_empty()
    }

    /// Whether this hoster is one of [`Self::hidden_hosters`], compared case-insensitively.
    #[must_use]
    pub fn hides(&self, hoster: &str) -> bool {
        !hoster.is_empty()
            && self
                .hidden_hosters
                .iter()
                .any(|hidden| hidden.trim().eq_ignore_ascii_case(hoster))
    }

    /// The facets actually set, trimmed and lowercased, as `(what, value)` pairs.
    ///
    /// Empty strings are dropped rather than matched: a select that was cleared sends `""`,
    /// and a preference for the empty quality would match nothing and hide the whole list.
    pub fn facets(&self) -> impl Iterator<Item = (MirrorFacet, String)> + '_ {
        [
            (MirrorFacet::Quality, self.quality.as_deref()),
            (MirrorFacet::Language, self.language.as_deref()),
            (MirrorFacet::Hoster, self.hoster.as_deref()),
        ]
        .into_iter()
        .filter_map(|(facet, value)| {
            let value = value.map(str::trim).filter(|value| !value.is_empty())?;
            Some((facet, value.to_ascii_lowercase()))
        })
    }
}

/// Which of the three dimensions a [`MirrorPreference`] entry speaks about.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MirrorFacet {
    Quality,
    Language,
    Hoster,
}

/// One candidate's membership in a mirror group.
///
/// A mirror is deliberately **not** a duplicate: [`LinkCandidateState::Duplicate`] means the
/// very same address is already in the collector and the copy adds nothing, while a mirror is
/// a *different* address for the same bytes and is kept precisely because it is needed the
/// moment the chosen one goes offline. Nothing here reads or writes that state, and two links
/// with the same address are never mirrors of each other.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct CandidateMirror {
    /// Key shared by the members of the group. Unique within the package and nowhere else:
    /// a group lives inside one package, because that is the unit the queue downloads.
    pub group: String,
    pub source: MirrorSource,
    /// Whether this is the member the package would use. Exactly one member of a group
    /// carries it.
    pub selected: bool,
    /// Whether a person chose this member by hand (RD-110-19).
    ///
    /// A pin outranks [`MirrorPreference`] and survives a regroup: a standing preference is a
    /// default, and a default that silently revises a decision somebody already made is worse
    /// than no default at all. At most one member of a group carries it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pinned: bool,
    /// The quality of this mirror, as the source named it or as its release name spells it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quality: Option<String>,
    /// The language of this mirror, on the same terms as [`Self::quality`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
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

/// User-defined destination category.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct Category {
    pub id: CategoryId,
    pub name: String,
    pub color: String,
    pub storage_root_id: StorageRootId,
    pub relative_path: String,
    pub is_default: bool,
    /// Default post-processing level for packages in this category; `None` = global default.
    #[serde(default)]
    pub postprocess_level: Option<PostprocessLevel>,
    /// Default post-processing script for packages in this category.
    #[serde(default)]
    pub script: Option<String>,
    /// Extensions (without dot) removed after unpacking packages in this category;
    /// `None` = use the global cleanup list.
    #[serde(default)]
    pub cleanup_extensions: Option<Vec<String>>,
    /// Whether packages in this category unpack nested archives recursively; `None` = global default.
    #[serde(default)]
    pub recursive_unpack: Option<bool>,
    /// Whether packages in this category verify `.sfv` checksums; `None` = global default.
    #[serde(default)]
    pub sfv_verify: Option<bool>,
    /// Whether a failed verification blocks unpacking for packages in this category;
    /// `None` = global default (RD-104-04).
    #[serde(default)]
    pub safe_postproc: Option<bool>,
    /// Whether packages in this category discard the PAR2 recovery set after a successful
    /// unpack; `None` = global default.
    #[serde(default)]
    pub delete_par2: Option<bool>,
    /// Whether packages in this category upload to rclone; `None` = global default.
    #[serde(default)]
    pub upload_enabled: Option<bool>,
    /// rclone target override in `remote:path` form; `None` = the global remote.
    #[serde(default)]
    pub upload_remote: Option<String>,
    /// Seeding override for torrents in this category; every unset field inherits from
    /// the global settings.
    #[serde(default)]
    pub seeding: Option<crate::SeedingPolicyOverride>,
    /// Post-processing plugin steps for packages in this category, by plugin id and in the
    /// order they run; `None` = the global list. An empty list means "none here", which is
    /// how a category switches a globally enabled step off.
    #[serde(default)]
    pub plugin_steps: Option<Vec<String>>,
}

/// Allowlisted filesystem root available to categories.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct StorageRootConfig {
    pub id: StorageRootId,
    pub name: String,
    pub path: String,
    pub is_default: bool,
    /// Free space that must remain on this root after a download finishes; `None`
    /// inherits `storage_minimum_free_bytes` from the service settings.
    #[serde(default)]
    pub minimum_free_bytes: Option<crate::ByteCount>,
}

/// Prioritized first-match category rule.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct CategoryRule {
    pub id: CategoryRuleId,
    pub name: String,
    pub priority: i32,
    pub source: Option<IngressSource>,
    pub domain: Option<String>,
    pub protocol: Option<String>,
    pub extension: Option<String>,
    pub mime_type: Option<String>,
    pub name_regex: Option<String>,
    pub category_id: CategoryId,
    pub enabled: bool,
}

/// Location responsible for watching a hotfolder.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum HotFolderExecutor {
    Daemon,
    CaptureAgent { agent_id: CaptureAgentId },
}

/// Whether an intake source waits for review or directly enters the queue.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ImportMode {
    Review,
    Enqueue,
}

/// Persisted hotfolder configuration.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct HotFolderConfig {
    pub id: HotFolderId,
    pub name: String,
    pub executor: HotFolderExecutor,
    pub path: String,
    pub recursive: bool,
    pub category_id: Option<CategoryId>,
    pub import_mode: ImportMode,
    pub processed_path: String,
    pub failed_path: String,
    pub enabled: bool,
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
mod candidate_url_tests {
    use super::{candidate_url, split_candidate_url};

    fn stored(address: &str) -> String {
        candidate_url(&address.parse().expect("an address")).into()
    }

    /// **A provider whose key is a link component keeps it -- in the vault** (RD-110-38).
    ///
    /// This test used to state the opposite, as a recorded collision: MEGA encrypts on the
    /// client and puts the file key in the fragment, RD-109-32 drops every fragment because
    /// nothing distinguishes a password from an anchor, and ADR 0011 was accepted on "the key
    /// travels in the link fragment the person already has". Neither could be bent on the way
    /// past, so the address that reached a row was one nothing could decrypt.
    ///
    /// The resolution keeps both promises: the stored address is shortened exactly as it was,
    /// and the fragment comes back out of this call as something to put in the vault under a
    /// reference. The row still never sees it.
    #[test]
    fn a_link_whose_key_is_its_fragment_keeps_it_out_of_the_row_and_in_the_vault() {
        let file = "https://mega.nz/file/yuZ0QJ6J#jFc2HL6rIoDVU9kECBpMEIAbcv2WQcz6le9kS_bb2gc"
            .parse()
            .expect("an address");
        let (stored, secret) = split_candidate_url(&file, true);
        assert_eq!(String::from(stored), "https://mega.nz/file/yuZ0QJ6J");
        assert_eq!(
            secret.as_deref(),
            Some("jFc2HL6rIoDVU9kECBpMEIAbcv2WQcz6le9kS_bb2gc")
        );

        // A folder address, and a file named inside one: the fragment carries the share key
        // and, for the child form, the path to the node as well. Both are kept whole.
        let folder = "https://mega.nz/folder/e4diDZ7T#iJnegBO_m6OXBQp27lHCrg"
            .parse()
            .expect("an address");
        let (stored, secret) = split_candidate_url(&folder, true);
        assert_eq!(String::from(stored), "https://mega.nz/folder/e4diDZ7T");
        assert_eq!(secret.as_deref(), Some("iJnegBO_m6OXBQp27lHCrg"));

        let child = "https://mega.nz/folder/e4diDZ7T#iJnegBO_m6OXBQp27lHCrg/file/KlVgwR4B"
            .parse()
            .expect("an address");
        let (stored, secret) = split_candidate_url(&child, true);
        assert_eq!(String::from(stored), "https://mega.nz/folder/e4diDZ7T");
        assert_eq!(
            secret.as_deref(),
            Some("iJnegBO_m6OXBQp27lHCrg/file/KlVgwR4B")
        );
    }

    /// Without the declaration nothing moves: every other address behaves exactly as it did
    /// under RD-109-32, and no fragment is handed out to be stored anywhere.
    #[test]
    fn a_link_no_provider_declared_still_loses_its_fragment_outright() {
        for address in [
            "https://cloud.exmaple.org/s/QxT7bK2mNp9wZr4#s3cret",
            "https://mega.nz/file/yuZ0QJ6J#jFc2HL6rIoDVU9kECBpMEIAbcv2WQcz6le9kS_bb2gc",
        ] {
            let url = address.parse().expect("an address");
            let (shortened, secret) = split_candidate_url(&url, false);
            assert_eq!(String::from(shortened), stored(address));
            assert_eq!(secret, None, "{address} must hand out no secret");
        }
    }

    /// An empty fragment is nothing to keep, however loudly a provider declares itself: the
    /// vault refuses empty material, and a reference pointing at nothing is worse than none.
    #[test]
    fn an_empty_fragment_is_never_vaulted() {
        let url = "https://mega.nz/file/yuZ0QJ6J#"
            .parse()
            .expect("an address");
        let (shortened, secret) = split_candidate_url(&url, true);
        assert_eq!(String::from(shortened), "https://mega.nz/file/yuZ0QJ6J");
        assert_eq!(secret, None);
    }

    /// The password RD-108-07 vaults for a claimed share is the fragment of the same address;
    /// for an unclaimed one it is dropped instead, and nothing else about the address moves.
    #[test]
    fn a_stored_address_never_carries_a_fragment() {
        assert_eq!(
            stored("https://cloud.exmaple.org/s/QxT7bK2mNp9wZr4#s3cret"),
            "https://cloud.exmaple.org/s/QxT7bK2mNp9wZr4"
        );
        // An empty fragment is a fragment: `…/a#` must not become a second, distinct row.
        assert_eq!(stored("https://example.com/a#"), "https://example.com/a");
        // Query, userinfo, port and path are untouched -- this is not a normalizer.
        assert_eq!(
            stored("https://example.com:8443/a/b?t=1&u=2#frag"),
            "https://example.com:8443/a/b?t=1&u=2"
        );
        // An address with nothing to drop comes back byte for byte.
        for address in [
            "https://example.com/a/b?t=1",
            "ftp://user@files.example.com/pub/x.iso",
            "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567",
        ] {
            assert_eq!(stored(address), address);
        }
    }
}

#[cfg(test)]
mod enqueueable_tests {
    use super::LinkCandidateState;

    #[test]
    fn only_links_being_worked_on_are_refused() {
        for state in [
            LinkCandidateState::Online,
            LinkCandidateState::Duplicate,
            LinkCandidateState::Offline,
            LinkCandidateState::Unsupported,
            LinkCandidateState::Error,
        ] {
            assert!(state.is_enqueueable(), "{state:?} should be enqueueable");
        }
        for state in [
            LinkCandidateState::Resolving,
            LinkCandidateState::Checking,
            LinkCandidateState::Enqueued,
            LinkCandidateState::Unresolvable,
        ] {
            assert!(!state.is_enqueueable(), "{state:?} should be refused");
        }
    }

    /// The defect RD-110-07 exists for: a link whose address answers with a page must never
    /// reach the queue, however it got into the list. Every other unhappy state still may.
    #[test]
    fn a_page_that_is_not_a_file_can_never_be_queued() {
        assert!(!LinkCandidateState::Unresolvable.is_enqueueable());
        for state in [
            LinkCandidateState::Offline,
            LinkCandidateState::Unsupported,
            LinkCandidateState::Error,
        ] {
            assert!(state.is_enqueueable(), "{state:?} must stay queueable");
        }
    }

    /// A check that fails outright leaves the link in `Error`, and that used to be the one
    /// state the enqueue handler refused — stricter than `Offline`, which means the file is
    /// known to be gone. Nothing about a failed check is evidence about the file.
    #[test]
    fn a_failed_check_is_not_worse_than_a_confirmed_offline_file() {
        assert_eq!(
            LinkCandidateState::Error.is_enqueueable(),
            LinkCandidateState::Offline.is_enqueueable()
        );
    }
}
