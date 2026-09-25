//! Origin containment for replayed browser requests.
//!
//! An ordinary download follows redirects freely, which is what a public CDN link needs. A
//! *replay* carries credentials the browser sent — a cookie jar, an `Authorization` header,
//! a client certificate, a form body — and those may only ever reach the origins a person
//! approved.
//!
//! This mirrors the containment the plugin host already applies to resolver requests
//! (`rd_plugin_host::native::expand::validate_redirect`); the transfer path never had an
//! equivalent.

use std::{future::Future, sync::Arc};

use url::Url;

use crate::HttpDownloadError;

/// Decides, for one request, whether a redirect hop may be followed (RD-130-24).
pub type RedirectGate = Arc<dyn Fn(&Url) -> bool + Send + Sync>;

tokio::task_local! {
    /// The gate of the request being sent in this task, if its sender set one.
    static REDIRECT_GATE: RedirectGate;
}

/// Sends `request` with every redirect hop it takes put to `gate` *before* it is followed.
///
/// A plugin's HTTP request goes out through a pooled client that is shared with every other
/// request of the same account, so where one request may be redirected to cannot be baked
/// into the client the way a replay's origins are: the plugin host narrows each invocation to
/// its own list of hosts, sometimes to a single one. The client's redirect policy therefore
/// asks the task it runs in. reqwest consults the policy while the response future is polled,
/// which is inside this scope, so a hop the gate refuses is never requested: the policy stops,
/// and the redirect comes back as the response, for the caller to refuse by its own rules.
/// Until RD-130-24 the plugin host checked where a request *ended* — after a `307` had already
/// carried the plugin's body to a host outside its domains.
///
/// Without a gate every policy behaves as it did before: an ordinary download is not affected.
pub async fn with_redirect_gate<F: Future>(gate: RedirectGate, request: F) -> F::Output {
    REDIRECT_GATE.scope(gate, request).await
}

/// Whether the gate of the current task, if there is one, lets a hop to `target` be followed.
pub(crate) fn gate_allows(target: &Url) -> bool {
    REDIRECT_GATE.try_with(|gate| gate(target)).unwrap_or(true)
}

/// The origins one consented replay may talk to.
#[derive(Clone, Debug, Default)]
pub struct ReplayScope {
    /// `scheme://host[:port]` entries, exactly as consented.
    pub approved_origins: Vec<String>,
}

impl ReplayScope {
    /// A scope from the consented origin list; `None` when nothing was consented, which
    /// leaves the client on the ordinary unrestricted policy.
    #[must_use]
    pub fn new(approved_origins: Vec<String>) -> Option<Self> {
        (!approved_origins.is_empty()).then_some(Self { approved_origins })
    }

    /// Stable key so the client pool can share one client per distinct origin set.
    #[must_use]
    pub fn key(&self) -> u64 {
        use std::hash::{Hash, Hasher};

        let mut origins = self.approved_origins.clone();
        origins.sort();
        origins.dedup();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        origins.hash(&mut hasher);
        hasher.finish()
    }
}

/// `scheme://host[:port]`, lowercased, with the scheme's default port dropped.
///
/// Deliberately identical to `rd_core::origin_of`: the string compared here has to be the
/// one consent was recorded against, or containment would be decided on a different value
/// than the one a person saw.
#[must_use]
pub fn origin_of(url: &Url) -> Option<String> {
    rd_core::origin_of(url)
}

/// Whether `target` is one of the approved origins.
#[must_use]
pub fn is_approved(origins: &[String], target: &Url) -> bool {
    origin_of(target).is_some_and(|origin| origins.iter().any(|approved| approved == &origin))
}

/// The failure a stopped redirect produces.
///
/// Without this a refused hop would surface as a bare `HTTP 302`, and the user would have
/// no way to tell a containment refusal from an ordinary redirect loop.
#[must_use]
pub fn not_allowed(target: &Url) -> HttpDownloadError {
    HttpDownloadError::Failure(
        rd_core::Failure::coded(
            rd_core::FailureKind::Permanent,
            "download.redirect_not_allowed",
            "The download was redirected outside the approved origins",
        )
        .with_param("target", rd_core::redact_url(target)),
    )
}

#[cfg(test)]
mod tests {
    use super::{ReplayScope, is_approved, origin_of};
    use url::Url;

    fn url(input: &str) -> Url {
        input.parse().expect("url")
    }

    #[test]
    fn origins_normalise_the_way_consent_recorded_them() {
        assert_eq!(
            origin_of(&url("HTTPS://CDN.Example.NET:443/f")).as_deref(),
            Some("https://cdn.example.net")
        );
        assert_eq!(
            origin_of(&url("https://cdn.example.net.:443/f")).as_deref(),
            Some("https://cdn.example.net")
        );
        assert_eq!(
            origin_of(&url("https://[2001:db8::1]:8443/f")).as_deref(),
            Some("https://[2001:db8::1]:8443")
        );
    }

    #[test]
    fn approval_is_exact_and_never_matches_a_lookalike() {
        let approved = ["https://cdn.example.net".to_owned()];
        assert!(is_approved(
            &approved,
            &url("https://cdn.example.net/f.bin")
        ));
        for hostile in [
            // A scheme downgrade would strip TLS from a credential-carrying request.
            "http://cdn.example.net/f.bin",
            // Neither a suffix nor a prefix of an approved host is that host.
            "https://evil-cdn.example.net.attacker.tld/f",
            "https://cdn.example.net.attacker.tld/f",
            "https://attacker.tld/f",
            // A different port is a different origin.
            "https://cdn.example.net:8443/f",
        ] {
            assert!(!is_approved(&approved, &url(hostile)), "{hostile}");
        }
    }

    #[test]
    fn an_empty_scope_is_no_scope_at_all() {
        // Guards against accidentally creating a scope that approves nothing and would
        // therefore refuse every hop of an ordinary download.
        assert!(ReplayScope::new(Vec::new()).is_none());
        assert!(ReplayScope::new(vec!["https://a.example".to_owned()]).is_some());
    }

    #[test]
    fn the_pool_key_ignores_order_and_duplicates() {
        let one = ReplayScope::new(vec![
            "https://b.example".to_owned(),
            "https://a.example".to_owned(),
        ])
        .expect("scope");
        let two = ReplayScope::new(vec![
            "https://a.example".to_owned(),
            "https://b.example".to_owned(),
            "https://a.example".to_owned(),
        ])
        .expect("scope");
        assert_eq!(one.key(), two.key());

        let other = ReplayScope::new(vec!["https://c.example".to_owned()]).expect("scope");
        assert_ne!(one.key(), other.key());
    }
}
