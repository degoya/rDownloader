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
        .user_agent(concat!("rDownloader/", env!("CARGO_PKG_VERSION")));

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
mod tests {
    use std::sync::Arc;

    use rd_core::AuthProfileId;
    use reqwest::cookie::Jar;
    use secrecy::SecretString;

    use super::{AuthMaterial, ClientContext, ClientKey, ClientPool};

    fn key(profile: Option<AuthProfileId>, revision: i64) -> ClientKey {
        ClientKey {
            proxy_profile_id: None,
            account_id: None,
            cookie_ref: None,
            auth_profile_id: profile,
            auth_revision: revision,
            replay_scope: None,
            tls_revision: 0,
            address_policy: None,
        }
    }

    fn context(key: ClientKey) -> ClientContext {
        ClientContext {
            key,
            proxy: None,
            proxy_credentials: None,
            cookie_jar: Arc::new(Jar::default()),
            custom_ca_pem: Vec::new(),
            auth: None,
            replay_scope: None,
        }
    }

    #[tokio::test]
    async fn editing_a_profile_evicts_its_previous_client() {
        // Without eviction every profile edit would leak a cached Client, and with it a
        // connection pool still holding the old certificate and redirect scope.
        let pool = ClientPool::default();
        let profile = AuthProfileId::new();
        pool.get_or_create(context(key(Some(profile), 1)))
            .await
            .expect("first client");
        pool.get_or_create(context(key(Some(profile), 2)))
            .await
            .expect("second client");

        let clients = pool.clients.read().await;
        assert_eq!(clients.len(), 1);
        assert_eq!(
            clients.keys().next().expect("key").auth_revision,
            2,
            "the stale revision must be dropped"
        );
    }

    /// TR-16: one client per replay scope or address rule, and no more of them than the bound.
    #[tokio::test]
    async fn scoped_clients_are_bounded_and_the_oldest_goes_first() {
        let pool = ClientPool::default();
        let scoped = |scope: u64| ClientKey {
            replay_scope: Some(scope),
            ..key(None, 0)
        };
        pool.get_or_create(context(key(None, 0)))
            .await
            .expect("ordinary client");
        for scope in 0..(super::MAX_SCOPED_CLIENTS as u64 + 5) {
            pool.get_or_create(context(scoped(scope)))
                .await
                .expect("scoped client");
        }

        let clients = pool.clients.read().await;
        assert_eq!(clients.len(), super::MAX_SCOPED_CLIENTS + 1);
        assert!(
            clients.contains_key(&key(None, 0)),
            "an ordinary client stays"
        );
        assert!(
            !clients.contains_key(&scoped(0)),
            "the oldest scoped client went"
        );
        assert!(clients.contains_key(&scoped(super::MAX_SCOPED_CLIENTS as u64 + 4)));
    }

    /// A hoster sets its session cookie while the plugin resolves, and the download wants it
    /// back. The plugin's client carries an address rule and the download's does not, so they
    /// are two clients; they share one jar, and another account's client does not.
    #[tokio::test]
    async fn a_cookie_the_plugin_client_received_is_sent_by_the_download_client() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };

        let server = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = server.local_addr().expect("address").port();
        let seen = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let log = Arc::clone(&seen);
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = server.accept().await {
                let mut buffer = [0_u8; 4096];
                let read = stream.read(&mut buffer).await.unwrap_or(0);
                let request = String::from_utf8_lossy(&buffer[..read]).to_ascii_lowercase();
                let cookie = request
                    .lines()
                    .find_map(|line| line.strip_prefix("cookie:"))
                    .map(|value| value.trim().to_owned())
                    .unwrap_or_default();
                log.lock().expect("log").push(cookie);
                let _ = stream
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nset-cookie: xfss=resolved; Path=/\r\n\
                          content-length: 0\r\nconnection: close\r\n\r\n",
                    )
                    .await;
            }
        });
        let pool = ClientPool::default();
        let account = |id: rd_core::AccountId, policy: Option<crate::AddressPolicy>| ClientKey {
            account_id: Some(id),
            address_policy: policy,
            ..key(None, 0)
        };
        let own = rd_core::AccountId::new();
        let plugin = pool
            .get_or_create(context(account(
                own,
                Some(crate::AddressPolicy::new(false)),
            )))
            .await
            .expect("plugin client");
        let download = pool
            .get_or_create(context(account(own, None)))
            .await
            .expect("download client");
        let stranger = pool
            .get_or_create(context(account(rd_core::AccountId::new(), None)))
            .await
            .expect("another account's client");

        // A literal address never reaches the guarded resolver, so the plugin client may call
        // the loopback listener here; the host judges literals before the request.
        let base = format!("http://127.0.0.1:{port}");
        plugin
            .get(format!("{base}/resolve"))
            .send()
            .await
            .expect("resolve");
        download
            .get(format!("{base}/file"))
            .send()
            .await
            .expect("download");
        stranger
            .get(format!("{base}/file"))
            .send()
            .await
            .expect("stranger");

        let seen = seen.lock().expect("log").clone();
        assert_eq!(seen.len(), 3);
        assert_eq!(seen[0], "", "nothing was set before the resolve");
        assert_eq!(
            seen[1], "xfss=resolved",
            "the download sends the resolve's cookie"
        );
        assert_eq!(seen[2], "", "another account has a jar of its own");
        assert_eq!(pool.clients.read().await.len(), 3);
        assert_eq!(pool.jars.lock().expect("jars").len(), 2);
    }

    #[tokio::test]
    async fn clients_of_different_profiles_coexist() {
        let pool = ClientPool::default();
        pool.get_or_create(context(key(Some(AuthProfileId::new()), 1)))
            .await
            .expect("first");
        pool.get_or_create(context(key(Some(AuthProfileId::new()), 1)))
            .await
            .expect("second");
        assert_eq!(pool.clients.read().await.len(), 2);
    }

    /// RD-130-24: a hop the request's gate refuses is never requested. The redirect comes
    /// back as the response; the same client without a gate follows it, as every ordinary
    /// download always did.
    #[tokio::test]
    async fn a_gated_redirect_is_handed_back_unfollowed() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };

        let target = TcpListener::bind("127.0.0.1:0").await.expect("bind target");
        let target_port = target.local_addr().expect("target address").port();
        let reached = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&reached);
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = target.accept().await {
                counter.fetch_add(1, Ordering::SeqCst);
                let mut buffer = [0_u8; 1024];
                let _ = stream.read(&mut buffer).await;
                let _ = stream
                    .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
                    .await;
            }
        });
        let origin = TcpListener::bind("127.0.0.1:0").await.expect("bind origin");
        let origin_port = origin.local_addr().expect("origin address").port();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = origin.accept().await {
                let mut buffer = [0_u8; 1024];
                let _ = stream.read(&mut buffer).await;
                let answer = format!(
                    "HTTP/1.1 307 Temporary Redirect\r\nlocation: http://127.0.0.1:{target_port}/x\r\n\
                     content-length: 0\r\nconnection: close\r\n\r\n"
                );
                let _ = stream.write_all(answer.as_bytes()).await;
            }
        });

        let client = ClientPool::default()
            .get_or_create(context(key(None, 0)))
            .await
            .expect("client");
        let start = format!("http://127.0.0.1:{origin_port}/start");
        let gate: crate::RedirectGate =
            Arc::new(move |hop: &url::Url| hop.port() != Some(target_port));
        let refused = crate::with_redirect_gate(gate, client.post(&start).body("payload").send())
            .await
            .expect("the redirect itself is the response");
        assert_eq!(refused.status().as_u16(), 307);
        assert_eq!(
            reached.load(Ordering::SeqCst),
            0,
            "the refused hop was requested"
        );

        let followed = client
            .post(&start)
            .body("payload")
            .send()
            .await
            .expect("followed");
        assert_eq!(followed.status().as_u16(), 200);
        assert_eq!(reached.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_client_certificate_must_be_a_pem_bundle() {
        // Identity::from_pem rejects encrypted keys and unknown sections outright, so a
        // bad bundle has to fail here rather than at the first download.
        let context = ClientContext {
            auth: Some(AuthMaterial {
                identity_pem: SecretString::from("-----BEGIN ENCRYPTED PRIVATE KEY-----\nx\n"),
            }),
            ..context(key(Some(AuthProfileId::new()), 1))
        };
        let jar = Arc::clone(&context.cookie_jar);
        assert!(super::build_client(&context, jar).is_err());
    }
}
