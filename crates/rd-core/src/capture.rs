use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use url::Url;
use utoipa::ToSchema;

use crate::{
    CaptureTokenId,
    request_template::{CapturedBody, ReplayBlockReason},
};

/// Scope granting access to the capture intake endpoints only.
pub const CAPTURE_SCOPE: &str = "capture:*";
/// Scope granting a paired capture agent the whole queue's pause and resume, and nothing else
/// (RD-1100-06). Chosen when the agent is paired, never implied: a capture token without it can
/// do exactly what it could before.
pub const CAPTURE_QUEUE_SCOPE: &str = "capture:queue";
/// Scope granting full API access (MCP and future machine clients).
pub const API_SCOPE: &str = "api:*";
/// Scope granting read-only access to queue and status resources.
///
/// Meant for dashboards and monitoring clients: it never unlocks intake, queue control,
/// settings or credentials, and the endpoints it reaches are an explicit allowlist rather
/// than "every GET".
pub const API_READ_SCOPE: &str = "api:read";

/// Scope granting link and file intake: adding work, and nothing about what happens to it.
pub const API_INTAKE_SCOPE: &str = "api:intake";
/// Scope granting control over queued work: start, pause, retry, reorder, delete.
pub const API_QUEUE_SCOPE: &str = "api:queue";
/// Scope granting configuration that holds no credential: categories, rules, schedules.
pub const API_CONFIG_SCOPE: &str = "api:config";
/// Scope granting access to stored credentials and the resources that carry them.
pub const API_SECRETS_SCOPE: &str = "api:secrets";
/// Scope granting service administration: plugins, service switches, updates, backups.
pub const API_ADMIN_SCOPE: &str = "api:admin";
/// Scope granting the metrics exposition and nothing else (RD-110-01).
///
/// A scrape target is pasted into a Prometheus configuration and lives there for years, so
/// it is its own surface: it reaches `/api/v1/metrics` alone, and `api:read` does not reach
/// that route either.
pub const API_METRICS_SCOPE: &str = "api:metrics";

/// One area of the API a token may be granted.
///
/// The areas are drawn along the lines a person would actually delegate. A dashboard reads;
/// a browser extension or a `*arr` instance adds links; an automation controls the queue;
/// a provisioning script writes configuration. Credentials and administration are separate
/// from all of it, because "may configure the download folder" and "may read every stored
/// password" are not the same request even though both are settings.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Scope {
    /// Queue, progress and system status.
    Read,
    /// Adding links, files and containers.
    Intake,
    /// Controlling queued and running work.
    Queue,
    /// Configuration that carries no credential.
    Config,
    /// Stored credentials and the resources built on them.
    Secrets,
    /// Service administration.
    Admin,
    /// The metrics exposition, and only that (RD-110-01). Off the ladder: it confers
    /// nothing and nothing but `api:*` confers it.
    Metrics,
    /// Browser-capture intake. Isolated from every API scope in both directions.
    Capture,
    /// Pausing and resuming the whole queue from the capture agent's tray (RD-1100-06). Off the
    /// ladder like [`Capture`](Self::Capture): it confers nothing, and nothing confers it, not
    /// even `api:*` or `capture:*`.
    CaptureQueue,
}

impl Scope {
    /// Every API scope, in least-to-most-privileged reading order.
    ///
    /// [`Scope::Capture`] is deliberately absent: it is not a point on this ladder, it is a
    /// separate surface, and listing it here would invite a "grant everything" loop to hand
    /// an API token the capture endpoints.
    pub const API: &'static [Self] = &[
        Self::Read,
        Self::Intake,
        Self::Queue,
        Self::Config,
        Self::Secrets,
        Self::Admin,
        // Last because it is not a point on the ladder at all: a scrape target, which sees
        // operational figures and can do nothing with them. Listed here so that `api:*`, a
        // session and the token editor all know it exists.
        Self::Metrics,
    ];

    /// The capture surface's scopes: the intake every agent holds, and the queue control one is
    /// paired with on request. Neither is on [`API`](Self::API)'s ladder.
    pub const CAPTURE: &'static [Self] = &[Self::Capture, Self::CaptureQueue];

    /// The scope string persisted in a token and shown to the user.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Read => API_READ_SCOPE,
            Self::Intake => API_INTAKE_SCOPE,
            Self::Queue => API_QUEUE_SCOPE,
            Self::Config => API_CONFIG_SCOPE,
            Self::Secrets => API_SECRETS_SCOPE,
            Self::Admin => API_ADMIN_SCOPE,
            Self::Metrics => API_METRICS_SCOPE,
            Self::Capture => CAPTURE_SCOPE,
            Self::CaptureQueue => CAPTURE_QUEUE_SCOPE,
        }
    }

    /// Parses a scope string, including the two legacy ones.
    ///
    /// `api:*` has no single [`Scope`]: it is every API scope at once, so it resolves through
    /// [`granted_scopes`] rather than here.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        Self::API
            .iter()
            .chain(Self::CAPTURE)
            .copied()
            .find(|scope| scope.as_str() == value)
    }

    /// The scopes this one confers in addition to itself.
    ///
    /// An explicit edge list, never a prefix or an ordering. Two properties matter and both
    /// are easy to lose to a clever rule:
    ///
    /// * **Nothing implies [`Secrets`](Self::Secrets) or [`Admin`](Self::Admin).** Not even
    ///   `Admin` implies `Secrets`: administering the service and reading every stored
    ///   password are different requests, and a token that needs both says so twice.
    /// * **Everything that acts implies [`Read`](Self::Read)**, because controlling a queue
    ///   you cannot see is not a thing anyone wants, and forcing a second scope for it would
    ///   only teach people to grant `api:*`.
    ///
    /// `Secrets` is the exception on both counts: it confers nothing, so a token minted to
    /// rotate a password does not also become a queue reader. `Metrics` is the same shape
    /// for the same reason: a scrape token must not become a queue reader, and no acting
    /// scope needs the exposition to act.
    #[must_use]
    pub fn implies(self) -> &'static [Self] {
        match self {
            Self::Intake | Self::Queue | Self::Config | Self::Admin => &[Self::Read],
            Self::Read | Self::Secrets | Self::Metrics | Self::Capture | Self::CaptureQueue => &[],
        }
    }

    /// Whether holding `self` satisfies a requirement for `required`.
    #[must_use]
    pub fn satisfies(self, required: Self) -> bool {
        self == required || self.implies().contains(&required)
    }

    /// The scope a live event belongs to, for filtering the event stream.
    ///
    /// Deliberately an exhaustive `match` with no wildcard arm. A new [`EventKind`] is then a
    /// compile error until someone decides who may see it — which is the strongest guarantee
    /// available here and costs nothing. A wildcard would silently classify every future
    /// event as whatever the fallback happened to be, and the fallback that is convenient to
    /// write is "everyone".
    #[must_use]
    pub fn of_event(kind: &crate::EventKind) -> Self {
        use crate::EventKind as Kind;
        match kind {
            // Queue and progress: what a dashboard exists to show.
            Kind::DownloadProgress
            | Kind::DownloadState
            | Kind::PackageState
            | Kind::PostprocessProgress
            | Kind::StorageCapacity
            | Kind::TorrentStats
            // A job running at a provider announces its stage and its progress, and that is
            // the same class of thing as a download's: something a dashboard or a tray shows.
            // Acting on it - choosing files, discarding it at the provider - is a separate
            // call behind its own scope, so seeing it grants nothing.
            | Kind::RemoteJobChanged
            | Kind::PowerChanged
            // Where the queue will pause, by id: the queue's own state, read at `Read` with
            // `get_queue_pause` (RD-1210-02).
            | Kind::QueueStopMark => Self::Read,
            // The LinkGrabber and what is waiting in it.
            Kind::CollectorChanged
            | Kind::CollectorIntake
            | Kind::CaptchaChanged
            // The post-processing steps and upload destinations an installed plugin offers are
            // read at `Queue`, so that is where their invalidation has to arrive. Announced at
            // `Admin` instead, a queue-scoped token read a list it was never told had changed.
            | Kind::PostprocessCatalogChanged => Self::Queue,
            // How the installation is set up.
            Kind::CategoryChanged
            | Kind::HotFolderChanged
            | Kind::StreamChanged
            | Kind::SubscriptionChanged
            | Kind::BandwidthChanged
            | Kind::NotificationChanged
            // An automation is a rule the operator wrote, and a managed tool is which
            // helper binary this installation verified and kept. Both are configuration
            // that carries no credential, and both are written through `Config`-scoped
            // endpoints (`/api/v1/automations*`, `/api/v1/system/tools/*`). Classifying
            // either as `Admin` would hide a change from exactly the tokens allowed to
            // make it, which is the mirror image of the leak this match exists to prevent.
            // Neither payload carries a source URL, a digest or a rule body, so a `Config`
            // subscriber learns which thing changed and nothing about its contents.
            | Kind::AutomationChanged
            | Kind::ManagedToolChanged
            // A site rule is the same class of thing as an automation: a rule the operator
            // wrote, carrying no credential, and its event names only the id.
            | Kind::SiteRuleChanged
            // The provider registry and the notification destinations are built from installed
            // manifests and are read at `Config`; the same argument as
            // `PostprocessCatalogChanged`, one scope along.
            | Kind::PluginCatalogChanged
            | Kind::ReconnectChanged
            | Kind::System => Self::Config,
            // Announce that a credential exists, was added or was taken away.
            Kind::AccountChanged
            | Kind::AuthProfileChanged
            | Kind::RemoteCredentialChanged
            | Kind::ProxyChanged
            | Kind::UsenetChanged
            | Kind::CaptureChanged
            // The trust store's two axes: which signing key is believed, and which package
            // digest is withdrawn. Both are written through `Secrets`-scoped routes and both
            // payloads name a row in a `Secrets`-scoped table, so the event follows the write
            // rather than the subsystem the word "plugin" suggests. As `Admin` it was wrong
            // twice over: the `Secrets` token that made the write never saw its own event,
            // and `Admin` subscribers were handed key ids and digests they may not read.
            | Kind::PluginTrustChanged => Self::Secrets,
            // Installing, enabling, disabling or removing a plugin: the service itself, and
            // the `Admin`-scoped half of `/api/v1/plugins*`.
            Kind::PluginChanged => Self::Admin,
        }
    }
}

/// Expands the scope strings a token holds into the scopes it actually confers.
///
/// This is where `api:*` is resolved: a legacy token expands to every API scope, `Secrets`
/// and `Admin` included. Silently narrowing it would be the worse failure — a token that
/// still exists, still authenticates, and has quietly lost the ability to do the job it was
/// minted for, with a 403 as the only clue.
#[must_use]
pub fn granted_scopes<'a, I>(held: I) -> Vec<Scope>
where
    I: IntoIterator<Item = &'a str>,
{
    let mut granted = Vec::new();
    for value in held {
        if value == API_SCOPE {
            granted.extend_from_slice(Scope::API);
            continue;
        }
        if let Some(scope) = Scope::parse(value) {
            granted.push(scope);
            granted.extend_from_slice(scope.implies());
        }
    }
    granted.sort_unstable();
    granted.dedup();
    granted
}

/// Whether the scope strings a token holds satisfy a required [`Scope`].
#[must_use]
pub fn scopes_grant<'a, I>(held: I, required: Scope) -> bool
where
    I: IntoIterator<Item = &'a str>,
{
    granted_scopes(held).contains(&required)
}

/// Whether a scope held by a token satisfies a required one, as strings.
///
/// Kept for the persisted-string comparisons the token store does. New code should reach for
/// [`scopes_grant`], which understands the whole vocabulary rather than a pair of strings.
#[must_use]
pub fn scope_satisfies(held: &str, required: &str) -> bool {
    if held == required {
        return true;
    }
    match (Scope::parse(held), Scope::parse(required)) {
        (Some(held), Some(required)) => held.satisfies(required),
        // `api:*` is not a single scope; it grants every API scope and nothing capture.
        _ if held == API_SCOPE => Scope::parse(required)
            .is_some_and(|required| required != Scope::Capture && Scope::API.contains(&required)),
        _ => false,
    }
}

/// Whether any of the scopes held by a token satisfies the requirement.
pub fn scopes_satisfy<'a, I>(held: I, required: &str) -> bool
where
    I: IntoIterator<Item = &'a str>,
{
    held.into_iter()
        .any(|scope| scope_satisfies(scope, required))
}

/// Version of the capture intake contract announced by `/api/v1/capture/ping`.
/// Clients that see a lower value (or none) fall back to the text-only payload.
///
/// v2 adds POST replay: a bounded request body plus the server-derived origin, expiry and
/// replayability markers. Every v2 field is `#[serde(default)]`, so a v1 client's payload
/// still deserializes unchanged.
pub const CAPTURE_CONTRACT_VERSION: u32 = 2;

/// The product name the desktop capture agent sends in its `User-Agent`, followed by `/` and its
/// version (`rdownloader-capture/1.9.0`). The service reads the version from it to tell the
/// update view which version each connected agent runs (RD-190-07); an agent from before 1.9
/// sends no `User-Agent` at all.
pub const CAPTURE_AGENT_PRODUCT: &str = "rdownloader-capture";

/// Request headers a capture client may forward with an intercepted download.
/// Everything else is dropped: `referer` and `user-agent` have dedicated fields,
/// transport headers are owned by the download engine, and credentials are
/// deliberately not part of this contract.
///
/// The last three entries arrived with contract v2, because many origins reject a form POST
/// that is missing them. Every entry must survive [`is_allowed_captured_header`]; the test
/// below locks that in, because [`is_credential_header`] matches substrings and would
/// silently drop a future addition such as `x-goog-api-key`.
pub const CAPTURED_HEADER_ALLOWLIST: &[&str] = &[
    "accept",
    "accept-language",
    "content-type",
    "origin",
    "x-requested-with",
];

/// Maximum number of links accepted in one structured capture batch.
pub const MAX_CAPTURE_LINKS: usize = 100;
/// Maximum number of headers kept per captured request.
pub const MAX_CAPTURED_HEADERS: usize = 32;
/// Maximum length of a captured header name.
pub const MAX_CAPTURED_HEADER_NAME: usize = 128;
/// Maximum length of a captured header value and of the free-text request fields.
pub const MAX_CAPTURED_VALUE: usize = 4096;

/// Whether a header name may be stored with a captured request.
#[must_use]
pub fn is_allowed_captured_header(name: &str) -> bool {
    let name = name.trim().to_ascii_lowercase();
    CAPTURED_HEADER_ALLOWLIST.contains(&name.as_str()) && !is_credential_header(&name)
}

/// Backstop for header names that may carry credentials. The allowlist already
/// excludes them; this guards against the allowlist ever growing carelessly.
#[must_use]
pub fn is_credential_header(name: &str) -> bool {
    let name = name.trim().to_ascii_lowercase();
    matches!(
        name.as_str(),
        "cookie" | "set-cookie" | "authorization" | "proxy-authorization" | "www-authenticate"
    ) || ["token", "secret", "session", "auth", "key"]
        .iter()
        .any(|needle| name.contains(needle))
}

/// One allowlisted request header captured with a browser download.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct CapturedHeader {
    pub name: String,
    pub value: String,
}

/// Metadata needed to reproduce the GET of an intercepted browser download.
///
/// Credential-free by contract: cookies, `Authorization` and reusable sessions
/// are modelled separately (auth profiles), never here.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct CapturedRequest {
    /// Final URL after redirects; only set when it differs from the link URL.
    #[schema(value_type = Option<String>, format = "uri")]
    #[serde(default)]
    pub effective_url: Option<Url>,
    /// HTTP method; contract v1 accepts `GET`, v2 also accepts `POST`.
    pub method: String,
    #[serde(default)]
    pub referrer: Option<String>,
    #[serde(default)]
    pub user_agent: Option<String>,
    #[serde(default)]
    pub content_disposition: Option<String>,
    /// Allowlisted headers only, names lowercased.
    #[serde(default)]
    pub headers: Vec<CapturedHeader>,
    /// Metadata of the request body. Field names only; values live in the secret store.
    #[serde(default)]
    pub body: Option<CapturedBody>,
    /// Base64 of the raw body, accepted on intake and never echoed back.
    ///
    /// The only plaintext-bearing field in the contract. Sanitizing moves it into the
    /// secret store and clears it before the request is persisted.
    #[serde(default, skip_serializing)]
    #[schema(write_only)]
    pub body_b64: Option<String>,
    /// Client's declaration that the request carried a file part. Never trusted for
    /// anything except refusing the body.
    #[serde(default)]
    pub has_file_upload: bool,
    /// Deadline read out of the signed URL. Server-derived; a client value is ignored.
    #[serde(default)]
    #[schema(read_only)]
    pub expires_at: Option<DateTime<Utc>>,
    /// Origins this request may be replayed against. Server-derived.
    #[serde(default)]
    #[schema(read_only)]
    pub approved_origins: Vec<String>,
    /// Whether the capture can be reproduced at all. Server-derived.
    #[serde(default = "default_true")]
    #[schema(read_only)]
    pub replayable: bool,
    /// Why it cannot be, when `replayable` is false. Server-derived.
    #[serde(default)]
    #[schema(read_only)]
    pub blocked_reason: Option<ReplayBlockReason>,
}

/// `replayable` defaults to true so a contract v1 payload keeps meaning what it meant.
const fn default_true() -> bool {
    true
}

impl Default for CapturedRequest {
    fn default() -> Self {
        Self {
            effective_url: None,
            method: String::new(),
            referrer: None,
            user_agent: None,
            content_disposition: None,
            headers: Vec::new(),
            body: None,
            body_b64: None,
            has_file_upload: false,
            expires_at: None,
            approved_origins: Vec::new(),
            replayable: true,
            blocked_reason: None,
        }
    }
}

/// Revocable token metadata; the bearer secret is never persisted in plaintext.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct CaptureToken {
    pub id: CaptureTokenId,
    pub label: String,
    pub scopes: Vec<String>,
    pub created_at: DateTime<Utc>,
    /// When the token was last accepted on a request, to the nearest minute.
    ///
    /// `None` means it has not been used since it was issued — which is the one thing worth
    /// knowing about a machine token nobody can remember handing out. Coarse on purpose: the
    /// write is throttled, because the alternative is one database write per request for a
    /// field nobody reads in real time.
    pub last_used_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
    /// From when on the token is refused like a revoked one (RD-1110-07); `None` never
    /// expires. An expired token stays listed until somebody revokes it, so it does not
    /// vanish from the list the moment its client starts failing.
    #[serde(default)]
    pub expires_at: Option<DateTime<Utc>>,
    /// Calls the token may make per minute, REST and MCP together (RD-1200-04); `None` is no
    /// limit. A call above it is refused with `429` and `api.token_rate_limited`.
    #[serde(default)]
    pub calls_per_minute: Option<u32>,
}

#[cfg(test)]
#[path = "capture_tests.rs"]
mod tests;
