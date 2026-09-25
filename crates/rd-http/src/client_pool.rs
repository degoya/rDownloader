use std::{collections::HashMap, sync::Arc, time::Duration};

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
}

impl ClientPool {
    /// Returns a matching cached client or creates one.
    pub async fn get_or_create(&self, context: ClientContext) -> Result<Client> {
        if let Some(client) = self.clients.read().await.get(&context.key).cloned() {
            return Ok(client);
        }

        let client = build_client(&context)?;
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
        Ok(clients
            .entry(context.key)
            .or_insert_with(|| client.clone())
            .clone())
    }

    /// Removes one client after a profile or account changed.
    pub async fn invalidate(&self, key: &ClientKey) {
        self.clients.write().await.remove(key);
    }

    /// Drops every cached connection, for example after global TLS changes.
    pub async fn clear(&self) {
        self.clients.write().await.clear();
    }
}

fn build_client(context: &ClientContext) -> Result<Client> {
    let mut builder = Client::builder()
        .no_proxy()
        .cookie_provider(Arc::clone(&context.cookie_jar))
        .connect_timeout(Duration::from_secs(20))
        .pool_idle_timeout(Duration::from_secs(90))
        .redirect(redirect_policy(
            context.auth.as_ref(),
            context.replay_scope.clone(),
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
fn redirect_policy(auth: Option<&AuthMaterial>, replay: Option<Arc<crate::ReplayScope>>) -> Policy {
    // A consented replay is confined to the origins a person approved, which is narrower
    // and more explicit than the same-origin rule a client certificate gets.
    if let Some(scope) = replay {
        return Policy::custom(move |attempt: Attempt| {
            if attempt.previous().len() >= MAX_REDIRECTS {
                return attempt.error(RedirectRefused::TooMany);
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
        return Policy::custom(|attempt: Attempt| {
            if attempt.previous().len() >= MAX_REDIRECTS {
                return attempt.error(RedirectRefused::TooMany);
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
        assert!(super::build_client(&context).is_err());
    }
}
