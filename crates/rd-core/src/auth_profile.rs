use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use url::Url;
use utoipa::ToSchema;

use crate::AuthProfileId;

/// Longest credential value accepted for a profile (tokens, passwords).
pub const MAX_AUTH_SECRET: usize = 16 * 1024;
/// Longest cookie payload accepted for a profile, matching the account cookie limit.
pub const MAX_AUTH_COOKIES: usize = 4 * 1024 * 1024;
/// Longest client-certificate PEM bundle accepted for a profile.
pub const MAX_AUTH_CERTIFICATE: usize = 128 * 1024;

/// Credential a profile carries. Exactly one per profile; a client certificate is an
/// independent addition rather than a method of its own.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AuthMethod {
    /// Browser session cookies, stored as Netscape `cookies.txt` or a `Cookie` header.
    Cookies,
    /// RFC 7617 `Authorization: Basic`.
    Basic,
    /// RFC 6750 `Authorization: Bearer`.
    Bearer,
}

/// Where a profile came from. Profiles handed over by a capture client are never usable
/// until a person approves them in the web UI.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AuthOrigin {
    BrowserCapture,
    Manual,
}

/// Why a scope string was rejected.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScopeError {
    /// Not parseable as a host, or carrying userinfo, query or fragment.
    Invalid,
    /// Scheme other than http/https.
    UnsupportedScheme,
    /// A path prefix that does not start with `/`.
    PathInvalid,
}

impl core::fmt::Display for ScopeError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let text = match self {
            Self::Invalid => "scope is not a valid host",
            Self::UnsupportedScheme => "scope must use http or https",
            Self::PathInvalid => "path prefix must start with a slash",
        };
        formatter.write_str(text)
    }
}

impl core::error::Error for ScopeError {}

/// Normalized domain and path a profile applies to.
///
/// The path prefix selects *which* profile matches a URL. It is deliberately **not** a
/// containment boundary: no HTTP mechanism keeps a credential from travelling to another
/// path of the same host on a redirect, and cookies do not treat paths as a security
/// boundary at all. Host containment is real; path containment is not.
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize, ToSchema)]
pub struct AuthScope {
    /// Lowercased, IDNA-encoded host without a trailing dot. `www.` is **not** stripped:
    /// a session for `www.example.com` is not a session for `example.com`.
    pub host: String,
    pub include_subdomains: bool,
    /// Always starts with `/` when present; a bare `/` is stored as `None`.
    pub path_prefix: Option<String>,
}

impl AuthScope {
    /// Normalizes user input such as `example.com`, `https://example.com/media/` or
    /// `HTTPS://Example.COM/media/`.
    ///
    /// Normalization goes through `Url::parse`, which supplies IDNA/punycode encoding,
    /// lowercasing and default-port removal. Doing it any other way risks the stored scope
    /// disagreeing with the host string seen at request time.
    pub fn parse(input: &str, include_subdomains: bool) -> Result<Self, ScopeError> {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Err(ScopeError::Invalid);
        }
        let candidate = if trimmed.contains("://") {
            trimmed.to_owned()
        } else {
            format!("https://{trimmed}")
        };
        let url = Url::parse(&candidate).map_err(|_| ScopeError::Invalid)?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(ScopeError::UnsupportedScheme);
        }
        // Credentials or a query in a scope are always a mistake, and silently dropping
        // them would store something other than what the user typed.
        if !url.username().is_empty() || url.password().is_some() || url.query().is_some() {
            return Err(ScopeError::Invalid);
        }
        let host = url.host_str().ok_or(ScopeError::Invalid)?;
        let host = host.trim_end_matches('.').to_ascii_lowercase();
        if host.is_empty() {
            return Err(ScopeError::Invalid);
        }
        Ok(Self {
            host,
            include_subdomains,
            path_prefix: normalize_path_prefix(url.path()),
        })
    }

    /// Whether this scope covers `url`.
    #[must_use]
    pub fn matches_url(&self, url: &Url) -> bool {
        let Some(host) = url.host_str() else {
            return false;
        };
        if !self.matches_host(host) {
            return false;
        }
        match &self.path_prefix {
            Some(prefix) => path_within(url.path(), prefix),
            None => true,
        }
    }

    /// Whether this scope covers `host`, honouring `include_subdomains`.
    #[must_use]
    pub fn matches_host(&self, host: &str) -> bool {
        let host = host.trim_end_matches('.').to_ascii_lowercase();
        if host == self.host {
            return true;
        }
        // The leading dot is what keeps `evil-example.com` and `example.com.evil.tld`
        // from matching a scope of `example.com`.
        self.include_subdomains && host.ends_with(&format!(".{}", self.host))
    }

    /// Ranking key for picking the most specific of several matching profiles: more host
    /// labels first, then the longer path prefix.
    #[must_use]
    pub fn specificity(&self) -> (usize, usize) {
        (
            self.host.split('.').count(),
            self.path_prefix.as_ref().map_or(0, String::len),
        )
    }

    /// Canonical `https://host/path` form, used as the target of the profile test action.
    #[must_use]
    pub fn probe_url(&self) -> Option<Url> {
        let path = self.path_prefix.as_deref().unwrap_or("/");
        Url::parse(&format!("https://{}{path}", self.host)).ok()
    }
}

/// Strips a bare `/` to `None` and guarantees a leading slash otherwise.
fn normalize_path_prefix(path: &str) -> Option<String> {
    let trimmed = path.trim();
    if trimmed.is_empty() || trimmed == "/" {
        return None;
    }
    let with_slash = if trimmed.starts_with('/') {
        trimmed.to_owned()
    } else {
        format!("/{trimmed}")
    };
    Some(with_slash)
}

/// Whether `path` lies inside `prefix`, on segment boundaries only, so `/media` never
/// covers `/mediafoo`.
fn path_within(path: &str, prefix: &str) -> bool {
    let prefix = prefix.trim_end_matches('/');
    if prefix.is_empty() {
        return true;
    }
    let Some(rest) = path.strip_prefix(prefix) else {
        return false;
    };
    rest.is_empty() || rest.starts_with('/')
}

/// Which profile a single job uses.
///
/// Three states, because "let the scope decide" and "deliberately send nothing" are
/// different intents and an `Option` can only express two.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case", tag = "mode", content = "id")]
pub enum AuthProfileSelection {
    /// Apply the most specific enabled profile whose scope matches the URL.
    #[default]
    Auto,
    /// Send no profile at all, even if one would match.
    None,
    /// Use exactly this profile; the job fails if it is missing, disabled or expired.
    Pinned(AuthProfileId),
}

impl AuthProfileSelection {
    /// Rebuilds the selection from its two stored columns.
    #[must_use]
    pub const fn from_columns(id: Option<AuthProfileId>, pinned: bool) -> Self {
        match (id, pinned) {
            (Some(id), _) => Self::Pinned(id),
            (None, true) => Self::None,
            (None, false) => Self::Auto,
        }
    }

    /// Splits the selection into the two stored columns.
    #[must_use]
    pub const fn to_columns(self) -> (Option<AuthProfileId>, bool) {
        match self {
            Self::Auto => (None, false),
            Self::None => (None, true),
            Self::Pinned(id) => (Some(id), false),
        }
    }
}

/// A reusable per-domain session or authentication profile.
///
/// Secret values live in the secret store; this struct only ever carries opaque
/// `vault://` references, and those are never serialized.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct AuthProfile {
    pub id: AuthProfileId,
    pub name: String,
    #[serde(flatten)]
    pub scope: AuthScope,
    pub method: AuthMethod,
    pub origin: AuthOrigin,
    pub enabled: bool,
    pub expires_at: Option<DateTime<Utc>>,
    /// Username for `AuthMethod::Basic`; `None` for every other method.
    pub username: Option<String>,
    #[serde(skip_serializing)]
    #[schema(ignore)]
    pub secret_ref: Option<String>,
    #[serde(skip_serializing)]
    #[schema(ignore)]
    pub certificate_ref: Option<String>,
    pub has_secret: bool,
    pub has_client_certificate: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl AuthProfile {
    /// Whether the profile's expiry has passed.
    #[must_use]
    pub fn is_expired(&self, now: DateTime<Utc>) -> bool {
        self.expires_at.is_some_and(|expiry| expiry <= now)
    }

    /// Whether the profile may be applied to a request right now.
    #[must_use]
    pub fn is_usable(&self, now: DateTime<Utc>) -> bool {
        self.enabled && !self.is_expired(now)
    }

    /// Revision the client pool keys on: a rotated secret, an edited scope and a disabled
    /// profile must all yield a fresh HTTP client.
    #[must_use]
    pub fn revision(&self) -> i64 {
        self.updated_at.timestamp_millis()
    }
}

#[cfg(test)]
mod tests {
    use super::{AuthProfileSelection, AuthScope, ScopeError};
    use crate::AuthProfileId;

    fn scope(input: &str, subdomains: bool) -> AuthScope {
        AuthScope::parse(input, subdomains).expect("scope")
    }

    fn url(input: &str) -> url::Url {
        input.parse().expect("url")
    }

    #[test]
    fn bare_host_and_full_url_normalize_alike() {
        assert_eq!(scope("Example.COM", false).host, "example.com");
        assert_eq!(scope("https://example.com/", false).host, "example.com");
        // A trailing dot is the same host to every resolver but a different string.
        assert_eq!(scope("example.com.", false).host, "example.com");
    }

    #[test]
    fn unicode_hosts_are_stored_as_punycode() {
        // The jar and reqwest both see the encoded host at request time, so the stored
        // scope has to agree with it.
        let scope = scope("https://\u{e9}xample.fr/", false);
        assert_eq!(scope.host, "xn--xample-9ua.fr");
        assert!(scope.matches_url(&url("https://xn--xample-9ua.fr/file")));
        assert!(scope.matches_url(&url("https://\u{e9}xample.fr/file")));
    }

    #[test]
    fn www_is_not_stripped() {
        // Cookies for www.example.com are not cookies for example.com.
        let scope = scope("www.example.com", false);
        assert_eq!(scope.host, "www.example.com");
        assert!(!scope.matches_url(&url("https://example.com/")));
    }

    #[test]
    fn subdomains_only_match_when_enabled() {
        assert!(!scope("example.com", false).matches_url(&url("https://cdn.example.com/f")));
        assert!(scope("example.com", true).matches_url(&url("https://cdn.example.com/f")));
    }

    #[test]
    fn lookalike_hosts_never_match() {
        let wide = scope("example.com", true);
        for host in [
            "https://evil-example.com/f",
            "https://example.com.evil.tld/f",
            "https://notexample.com/f",
        ] {
            assert!(!wide.matches_url(&url(host)), "{host}");
        }
    }

    #[test]
    fn port_and_scheme_do_not_affect_host_matching() {
        let scope = scope("example.com", false);
        assert!(scope.matches_url(&url("https://example.com:8443/f")));
        assert!(scope.matches_url(&url("http://example.com/f")));
    }

    #[test]
    fn path_prefix_matches_on_segment_boundaries() {
        let scope = scope("https://example.com/media", false);
        assert_eq!(scope.path_prefix.as_deref(), Some("/media"));
        assert!(scope.matches_url(&url("https://example.com/media")));
        assert!(scope.matches_url(&url("https://example.com/media/clip.mp4")));
        // The bug this locks in: a prefix compare without a boundary check would match.
        assert!(!scope.matches_url(&url("https://example.com/mediafoo")));
        assert!(!scope.matches_url(&url("https://example.com/other")));
    }

    #[test]
    fn root_path_is_stored_as_no_prefix() {
        assert_eq!(scope("https://example.com/", false).path_prefix, None);
        assert_eq!(scope("example.com", false).path_prefix, None);
    }

    #[test]
    fn scopes_with_credentials_or_query_are_rejected() {
        assert_eq!(
            AuthScope::parse("https://user:pw@example.com/", false),
            Err(ScopeError::Invalid)
        );
        assert_eq!(
            AuthScope::parse("https://example.com/?token=1", false),
            Err(ScopeError::Invalid)
        );
        assert_eq!(AuthScope::parse("   ", false), Err(ScopeError::Invalid));
    }

    #[test]
    fn non_http_schemes_are_rejected() {
        assert_eq!(
            AuthScope::parse("ftp://example.com/", false),
            Err(ScopeError::UnsupportedScheme)
        );
    }

    #[test]
    fn specificity_prefers_more_labels_then_longer_paths() {
        let mut scopes = [
            scope("example.com", true),
            scope("https://cdn.example.com/media/hd", false),
            scope("https://cdn.example.com/media", false),
        ];
        scopes.sort_by_key(AuthScope::specificity);
        assert_eq!(scopes[0].host, "example.com");
        assert_eq!(scopes[1].path_prefix.as_deref(), Some("/media"));
        assert_eq!(scopes[2].path_prefix.as_deref(), Some("/media/hd"));
    }

    #[test]
    fn probe_url_rebuilds_the_scope() {
        assert_eq!(
            scope("https://example.com/media", false)
                .probe_url()
                .map(String::from),
            Some("https://example.com/media".to_owned())
        );
    }

    #[test]
    fn selection_round_trips_through_its_columns() {
        let id = AuthProfileId::new();
        for selection in [
            AuthProfileSelection::Auto,
            AuthProfileSelection::None,
            AuthProfileSelection::Pinned(id),
        ] {
            let (stored_id, pinned) = selection.to_columns();
            assert_eq!(
                AuthProfileSelection::from_columns(stored_id, pinned),
                selection
            );
        }
    }
}
