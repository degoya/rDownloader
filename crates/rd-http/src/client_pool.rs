use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
};

use anyhow::{Context, Result};
use rd_core::{AccountId, AuthProfileId, ProxyProfile, ProxyProfileId};
use reqwest::{
    Certificate, Client, Identity, Proxy,
    cookie::Jar,
    redirect::{Attempt, Policy},
};
use secrecy::{ExposeSecret, SecretString};
use tokio::sync::RwLock;
use url::Url;

/// Redirect budget shared by the default and the scope-bounded policy.
const MAX_REDIRECTS: usize = 10;

/// Clients built for one consented replay or one stranger's mirror that the pool keeps at
/// most; the oldest goes first. Each such key is new with every replay scope and every
/// address rule, so without a bound they stayed until process exit (audit 1.9.1, TR-16). A
/// transfer that still holds an evicted client keeps it; only the pool forgets it.
const MAX_SCOPED_CLIENTS: usize = 32;

/// Global fallback proxy and TLS roots shared by resolver and transfer clients.
#[derive(Clone, Debug, Default)]
pub struct NetworkDefaults {
    pub global_proxy_profile_id: Option<ProxyProfileId>,
    pub custom_ca_pem: Vec<Vec<u8>>,
    pub tls_revision: u64,
}

pub type SharedNetworkDefaults = Arc<RwLock<NetworkDefaults>>;

/// Stable cache key for a reqwest client.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ClientKey {
    pub proxy_profile_id: Option<ProxyProfileId>,
    pub account_id: Option<AccountId>,
    /// Secret reference of the imported cookies; replacing them yields a new client.
    pub cookie_ref: Option<String>,
    /// Set only when a profile contributes client-wide material (cookies or a client
    /// certificate). `Basic`/`Bearer` travel as per-request headers and never fragment
    /// the pool.
    pub auth_profile_id: Option<AuthProfileId>,
    /// The profile's `updated_at` in milliseconds. Keying on the revision rather than the
    /// secret reference also catches scope edits, which change the baked-in redirect
    /// policy, and keeps opaque references out of the pool's `Debug` output.
    pub auth_revision: i64,
    /// Hash of the approved origin set of a consented replay.
    ///
    /// The redirect policy is baked into a client, so two transfers with different approved
    /// origins must not share one. `None` for every ordinary download, which keeps the pool
    /// exactly as fragmented as it was before replay existed.
    pub replay_scope: Option<u64>,
    pub tls_revision: u64,
    /// The address rule of a request made on a stranger's word — a Metalink's mirror
    /// (RD-150-03). Such a client resolves names through [`crate::GuardedResolver`] and
    /// follows no redirect to an address the rule refuses. `None` for every ordinary download.
    pub address_policy: Option<crate::AddressPolicy>,
}

/// Decrypted proxy credentials held only while a client is built.
pub struct ProxyCredentials {
    pub username: String,
    pub password: SecretString,
}

/// Client-wide material contributed by an auth profile.
pub struct AuthMaterial {
    /// Private key and certificate chain as one PEM bundle.
    pub identity_pem: SecretString,
}

/// Inputs required to construct an isolated HTTP client.
pub struct ClientContext {
    pub key: ClientKey,
    pub proxy: Option<ProxyProfile>,
    pub proxy_credentials: Option<ProxyCredentials>,
    pub cookie_jar: Arc<Jar>,
    pub custom_ca_pem: Vec<Vec<u8>>,
    /// Client certificate plus the scope it is confined to.
    pub auth: Option<AuthMaterial>,
    /// Origins a consented replay may follow redirects into.
    pub replay_scope: Option<Arc<crate::ReplayScope>>,
}

/// Reuses clients without allowing proxy, cookies, credentials or TLS roots to bleed
/// between accounts and profiles.
#[derive(Clone, Default)]
pub struct ClientPool {
    clients: Arc<RwLock<HashMap<ClientKey, Client>>>,
    /// The keys of [`MAX_SCOPED_CLIENTS`] in the order they were added. Only touched while
    /// `clients` is held for writing.
    scoped: Arc<Mutex<VecDeque<ClientKey>>>,
    /// The cookie jar each client is built with, keyed by [`ClientKey::jar_key`].
    ///
    /// A plugin's requests carry an address rule and a download's do not, so the two get
    /// different clients — but a hoster sets its session cookies while the plugin resolves and
    /// wants them back on the download, which until the rule existed was the same client. The
    /// jar is therefore shared by every client that differs only in its address rule. Only
    /// touched while `clients` is held for writing, or before a client is built.
    jars: Arc<Mutex<HashMap<ClientKey, Arc<Jar>>>>,
}

impl ClientKey {
    /// Whether the client belongs to one replay or one stranger's address rule rather than to
    /// an account, a proxy or a profile that comes back.
    fn is_scoped(&self) -> bool {
        self.replay_scope.is_some() || self.address_policy.is_some()
    }

    /// The key whose cookie jar this client shares: itself without the address rule. The rule
    /// decides which addresses a request may reach, never which session it belongs to.
    fn jar_key(&self) -> Self {
        Self {
            address_policy: None,
            ..self.clone()
        }
    }
}

impl ClientPool {
    /// Returns a matching cached client or creates one.
    pub async fn get_or_create(&self, context: ClientContext) -> Result<Client> {
        if let Some(client) = self.clients.read().await.get(&context.key).cloned() {
            return Ok(client);
        }

        // The jar a client of the same session already has, or this context's own, which then
        // becomes that session's jar. Cookies imported into the context's jar (the vault's,
        // a profile's) are the same for every key with the same `jar_key`, since the key names
        // where they came from.
        let jar = Arc::clone(
            self.jars
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .entry(context.key.jar_key())
                .or_insert_with(|| Arc::clone(&context.cookie_jar)),
        );
        let client = build_client(&context, jar)?;
        let mut clients = self.clients.write().await;
        // Drop the account's previous client (stale cookies) instead of keeping it around.
        clients.retain(|key, _| {
            key.account_id != context.key.account_id
                || key.proxy_profile_id != context.key.proxy_profile_id
                || key.cookie_ref == context.key.cookie_ref
        });
        // Same for a profile that was edited: the old client still carries the previous
        // certificate and redirect scope, and would otherwise live until process exit.
        if let Some(profile_id) = context.key.auth_profile_id {
            clients.retain(|key, _| {
                key.auth_profile_id != Some(profile_id)
                    || key.auth_revision == context.key.auth_revision
            });
        }
        if context.key.is_scoped() {
            let mut scoped = self.scoped.lock().unwrap_or_else(PoisonError::into_inner);
            scoped.retain(|key| clients.contains_key(key));
            if !clients.contains_key(&context.key) {
                scoped.push_back(context.key.clone());
            }
            while scoped.len() > MAX_SCOPED_CLIENTS {
                if let Some(oldest) = scoped.pop_front() {
                    clients.remove(&oldest);
                }
            }
        }
        let client = clients
            .entry(context.key)
            .or_insert_with(|| client.clone())
            .clone();
        // A jar outlives its last client by nothing: a changed cookie reference or an evicted
        // scope starts a session of its own, as a new client always did.
        self.jars
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|jar_key, _| clients.keys().any(|key| key.jar_key() == *jar_key));
        Ok(client)
    }

    /// Drops every cached connection, for example after global TLS changes.
    pub async fn clear(&self) {
        let mut clients = self.clients.write().await;
        clients.clear();
        self.jars
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
        self.scoped
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
    }
}

fn build_client(context: &ClientContext, cookie_jar: Arc<Jar>) -> Result<Client> {
    let mut builder = Client::builder()
        .no_proxy()
        .cookie_provider(cookie_jar)
        .connect_timeout(Duration::from_secs(20))
        .pool_idle_timeout(Duration::from_secs(90))
        .redirect(redirect_policy(
            context.auth.as_ref(),
            context.replay_scope.clone(),
            context.key.address_policy.clone(),
        ))
        .user_agent(rd_core::user_agent!());

    if let Some(profile) = &context.proxy {
        let mut proxy = Proxy::all(profile.endpoint.as_str()).context("create proxy")?;
        if let Some(credentials) = &context.proxy_credentials {
            proxy = proxy.basic_auth(&credentials.username, credentials.password.expose_secret());
        }
        builder = builder.proxy(proxy);
    }

    for pem in &context.custom_ca_pem {
        let certificate = Certificate::from_pem(pem).context("parse custom CA certificate")?;
        builder = builder.add_root_certificate(certificate);
    }

    if let Some(auth) = &context.auth {
        let identity = Identity::from_pem(auth.identity_pem.expose_secret().as_bytes())
            .context("parse client certificate")?;
        builder = builder.identity(identity);
    }

    // Through a proxy the resolver would only ever see the proxy's own name — which may well
    // sit on the person's network — while the proxy resolves the target. The target was still
    // judged before the request (`check_target`), and every redirect hop still is.
    if let Some(policy) = &context.key.address_policy
        && context.proxy.is_none()
    {
        builder = builder.dns_resolver(crate::GuardedResolver::system(policy.clone()));
    }

    builder.build().context("build HTTP client")
}

/// Redirects are only confined when a client certificate is attached.
///
/// Cookies and `Authorization` contain themselves: the jar refuses to emit for a foreign
/// host, and reqwest strips `Authorization` on every host/port/scheme change. A client
/// certificate has no such protection — it is a property of the client and is offered
/// during the handshake with whatever host a redirect points at, before any of this code
/// runs again. Confining every profile instead would break ordinary downloads, because
/// hosters routinely hand the payload off to a foreign CDN.
///
/// The bar is the request's own origin rather than the profile scope: a scope may cover
/// subdomains, and presenting a client certificate to a sibling host is exactly the leak
/// this prevents. It is also the definition of "cross-origin" reqwest itself uses when it
/// strips credential headers.
///
/// A client with an address rule refuses, on top of all that, a hop to a scheme other than
/// HTTP(S) or to a literal address the rule does not permit: a literal never reaches the
/// resolver that judges names.
fn redirect_policy(
    auth: Option<&AuthMaterial>,
    replay: Option<Arc<crate::ReplayScope>>,
    guard: Option<crate::AddressPolicy>,
) -> Policy {
    // A consented replay is confined to the origins a person approved, which is narrower
    // and more explicit than the same-origin rule a client certificate gets.
    if let Some(scope) = replay {
        return Policy::custom(move |attempt: Attempt| {
            if attempt.previous().len() >= MAX_REDIRECTS {
                return attempt.error(RedirectRefused::TooMany);
            }
            if let Some(refused) = hop_refusal(guard.as_ref(), attempt.url()) {
                return attempt.error(refused);
            }
            if !crate::redirect::gate_allows(attempt.url()) {
                return attempt.stop();
            }
            if crate::redirect::is_approved(&scope.approved_origins, attempt.url()) {
                return attempt.follow();
            }
            tracing::warn!(
                target = %rd_core::Redacted(attempt.url()),
                "refused a replay redirect outside the approved origins"
            );
            // `stop()` rather than `error()`: the engine's post-condition turns the
            // resulting 3xx into `download.redirect_not_allowed`, which is a permanent,
            // translatable failure instead of a retried transport error.
            attempt.stop()
        });
    }
    // Every policy asks the request's redirect gate first, if its sender set one
    // (`with_redirect_gate`, RD-130-24): a plugin request may be narrowed to hosts no client
    // can know about. Stopping hands the redirect back as the response, unfollowed; the
    // sender refuses it by its own rules. Without a gate this is `Policy::limited`.
    if auth.is_none() {
        return Policy::custom(move |attempt: Attempt| {
            if attempt.previous().len() >= MAX_REDIRECTS {
                return attempt.error(RedirectRefused::TooMany);
            }
            if let Some(refused) = hop_refusal(guard.as_ref(), attempt.url()) {
                return attempt.error(refused);
            }
            if !crate::redirect::gate_allows(attempt.url()) {
                return attempt.stop();
            }
            attempt.follow()
        });
    }
    Policy::custom(move |attempt: Attempt| {
        // A custom policy inherits no loop or limit handling, so the budget is explicit.
        if attempt.previous().len() >= MAX_REDIRECTS {
            return attempt.error(RedirectRefused::TooMany);
        }
        if let Some(refused) = hop_refusal(guard.as_ref(), attempt.url()) {
            return attempt.error(refused);
        }
        if !crate::redirect::gate_allows(attempt.url()) {
            return attempt.stop();
        }
        let origin = attempt.previous().first().map(origin_of);
        if origin == Some(origin_of(attempt.url())) {
            attempt.follow()
        } else {
            // An error, not `stop()`: stopping would surface the 30x as a successful
            // response and the engine would report a puzzling bad status instead.
            attempt.error(RedirectRefused::OutOfScope)
        }
    })
}

/// Why a guarded client may not follow a hop; `None` without a guard. An error rather than
/// `stop()`, so the refusal travels in the error chain and the engine reports it by its code.
fn hop_refusal(
    guard: Option<&crate::AddressPolicy>,
    target: &Url,
) -> Option<crate::AddressRefused> {
    guard.and_then(|policy| policy.hop_refusal(target))
}

/// Scheme, host and effective port — the triple reqwest treats as one origin.
fn origin_of(url: &Url) -> (String, Option<String>, Option<u16>) {
    (
        url.scheme().to_owned(),
        url.host_str().map(str::to_ascii_lowercase),
        url.port_or_known_default(),
    )
}

/// Why a redirect was not followed.
#[derive(Debug)]
enum RedirectRefused {
    OutOfScope,
    TooMany,
}

impl std::fmt::Display for RedirectRefused {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            Self::OutOfScope => {
                "redirect leaves the request origin while a client certificate is attached"
            }
            Self::TooMany => "too many redirects",
        };
        formatter.write_str(text)
    }
}

impl std::error::Error for RedirectRefused {}

#[cfg(test)]
#[path = "client_pool_tests.rs"]
mod tests;
