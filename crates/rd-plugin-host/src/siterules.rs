//! The ports the site-rule executor needs, wired to what this application already has
//! (RD-110-06).
//!
//! `rd-siterules` owns the rule format, the steps and the three bolts, and deliberately owns
//! no HTTP client, no captcha broker and no clock (RD-110-05). This is the other half: the
//! adapters live here, where the proxy profiles, the custom TLS roots and the captcha broker
//! already are, and the crate above (`rd-plugin-ext`) puts them together with the catalogue.
//!
//! **Two obligations, both load-bearing rather than advisory**, and both are why the fetcher
//! builds its own client instead of taking one from [`rd_http::ClientPool`]:
//!
//! 1. **Resolve once, connect to that.** The executor resolves the host through
//!    [`RuleResolver`], refuses every private, loopback and link-local answer, and hands the
//!    addresses it checked over in `FetchRequest::addresses`. The fetcher connects to exactly
//!    those with `ClientBuilder::resolve_to_addrs` and never resolves the name again — a
//!    record with a time-to-live of zero is otherwise free to answer a routable address to the
//!    check and `127.0.0.1` to the connection a moment later, and the bolt would be theatre.
//! 2. **Redirects are reported, not followed.** The executor follows them itself so that every
//!    hop passes the host bolt and the address bolt again, so the client is built with
//!    `Policy::none()`.
//!
//! Pinned addresses are what keeps these clients out of the shared pool: a client that is
//! bound to one host's checked addresses is of no use to any other request, so pooling it
//! would gain nothing and would grow a cache with one entry per host a rule ever visited.
//! A fetcher therefore belongs to **one run**: its clients, its cookies and its pinning die
//! with it, which is also what keeps a board's session cookie from reaching a download.
//!
//! A configured proxy is honoured, and with one the name is resolved at the proxy rather than
//! here — the pinning cannot bind there. That is the person's own configuration, and the
//! executor's ban on private addresses still refuses a name that resolves into this network.

use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use rd_http::SharedNetworkDefaults;
use rd_plugin_api::{CaptchaChallenge, WidgetChallenge};
use rd_siterules::{
    CaptchaRequest, FetchFailure, FetchRequest, FetchResponse, Fetcher, HostResolver, Method,
};
use reqwest::{Certificate, Client, Proxy, cookie::Jar, redirect::Policy};
use secrecy::ExposeSecret;
use tokio::sync::Mutex;
use url::Url;

/// How long one connection attempt may take. The whole request is capped by the executor's
/// own `request_timeout`, which arrives with every [`FetchRequest`].
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

/// What a rule run borrows from the application: the proxy and TLS configuration, and the
/// secrets a proxy credential sits in. Cheap to clone; one per service, not one per run.
#[derive(Clone)]
pub struct RuleNetwork {
    database: rd_db::Database,
    secrets: rd_secrets::SecretStore,
    network_defaults: SharedNetworkDefaults,
}

impl RuleNetwork {
    #[must_use]
    pub fn new(
        database: rd_db::Database,
        secrets: rd_secrets::SecretStore,
        network_defaults: SharedNetworkDefaults,
    ) -> Self {
        Self {
            database,
            secrets,
            network_defaults,
        }
    }

    /// The fetcher for **one** run: its cookie jar and its pinned clients live exactly as long
    /// as the run does.
    #[must_use]
    pub fn fetcher(&self) -> RuleFetcher {
        RuleFetcher {
            network: self.clone(),
            jar: Arc::new(Jar::default()),
            clients: Mutex::new(HashMap::new()),
        }
    }
}

/// One run's HTTP client: no redirects, and connecting only to the addresses the executor
/// checked.
pub struct RuleFetcher {
    network: RuleNetwork,
    /// One jar for the whole run, so a page reached through a login form or a redirect chain
    /// carries the session the previous request was given. It never outlives the run.
    jar: Arc<Jar>,
    clients: Mutex<HashMap<PinnedHost, Client>>,
}

/// A host together with the addresses this run may reach it at. Two runs of the same rule can
/// legitimately see different addresses, which is precisely why the client is not shared.
type PinnedHost = (String, Vec<IpAddr>);

impl RuleFetcher {
    /// The client for one host, built once per run and kept for the rest of it.
    async fn client(&self, url: &Url, addresses: &[IpAddr]) -> Result<Client, FetchFailure> {
        let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
        let mut pinned = addresses.to_vec();
        pinned.sort_unstable();
        let key = (host.clone(), pinned);
        if let Some(client) = self.clients.lock().await.get(&key) {
            return Ok(client.clone());
        }
        let client = self.build(url, &host, addresses).await?;
        self.clients.lock().await.insert(key, client.clone());
        Ok(client)
    }

    async fn build(
        &self,
        url: &Url,
        host: &str,
        addresses: &[IpAddr],
    ) -> Result<Client, FetchFailure> {
        let defaults = self.network.network_defaults.read().await.clone();
        // No account and no auth profile: a rule reads a public page. A credential would be a
        // decision the rule has no business making, and a profile is a transfer-time thing.
        let config = self
            .network
            .database
            .network_client_config(
                None,
                None,
                defaults.global_proxy_profile_id,
                rd_core::AuthProfileSelection::None,
                url,
            )
            .await
            .map_err(|error| FetchFailure::Other(format!("network configuration: {error}")))?;
        let mut builder = Client::builder()
            .no_proxy()
            .cookie_provider(Arc::clone(&self.jar))
            // Obligation two: the executor follows redirects itself, checking every hop.
            .redirect(Policy::none())
            .connect_timeout(CONNECT_TIMEOUT)
            .user_agent(concat!("rDownloader/", env!("CARGO_PKG_VERSION")));
        // Obligation one. Empty exactly when the host is a literal address, where there was
        // nothing to resolve and nothing to pin.
        if !addresses.is_empty() {
            let port = url.port_or_known_default().unwrap_or(443);
            let pinned: Vec<SocketAddr> = addresses
                .iter()
                .map(|address| SocketAddr::new(*address, port))
                .collect();
            builder = builder.resolve_to_addrs(host, &pinned);
        }
        if let Some(profile) = &config.proxy {
            let mut proxy = Proxy::all(profile.endpoint.as_str())
                .map_err(|error| FetchFailure::Other(format!("proxy: {error}")))?;
            if let (Some(reference), Some(username)) = (&profile.secret_ref, &profile.username) {
                let password =
                    self.network.secrets.get(reference).await.map_err(|error| {
                        FetchFailure::Other(format!("proxy credential: {error}"))
                    })?;
                proxy = proxy.basic_auth(username, password.expose_secret());
            }
            builder = builder.proxy(proxy);
        }
        for pem in &defaults.custom_ca_pem {
            let certificate = Certificate::from_pem(pem)
                .map_err(|error| FetchFailure::Other(format!("custom CA: {error}")))?;
            builder = builder.add_root_certificate(certificate);
        }
        builder
            .build()
            .map_err(|error| FetchFailure::Other(format!("build client: {error}")))
    }
}

#[async_trait]
impl Fetcher for RuleFetcher {
    async fn fetch(&self, request: FetchRequest) -> Result<FetchResponse, FetchFailure> {
        let client = self.client(&request.url, &request.addresses).await?;
        let method = match request.method {
            Method::Get => reqwest::Method::GET,
            Method::Post => reqwest::Method::POST,
        };
        let mut builder = client
            .request(method, request.url.clone())
            .timeout(request.timeout);
        if !request.form.is_empty() {
            // Encoded here rather than through `RequestBuilder::form`, which this build of
            // reqwest does not carry: the executor's contract is one shape, and it is the
            // one every browser sends a form in.
            let body = url::form_urlencoded::Serializer::new(String::new())
                .extend_pairs(request.form.iter())
                .finish();
            builder = builder
                .header(
                    reqwest::header::CONTENT_TYPE,
                    "application/x-www-form-urlencoded",
                )
                .body(body);
        }
        let mut response = builder.send().await.map_err(transport_failure)?;
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .filter_map(|(name, value)| {
                value
                    .to_str()
                    .ok()
                    .map(|value| (name.as_str().to_ascii_lowercase(), value.to_owned()))
            })
            .collect();
        // Read to the ceiling and stop there: the executor checks the length it got back as
        // well, so this is the cheap half of that guard rather than the only one.
        let mut body: Vec<u8> = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(transport_failure)? {
            if body.len() + chunk.len() > request.max_bytes {
                return Err(FetchFailure::TooLarge);
            }
            body.extend_from_slice(&chunk);
        }
        Ok(FetchResponse {
            status,
            headers,
            // Lossy on purpose: a page whose bytes are not UTF-8 still has addresses in it,
            // and a rule that finds none of them refuses honestly a moment later.
            body: String::from_utf8_lossy(&body).into_owned(),
        })
    }
}

/// Why nothing answered, in the terms the executor sorts its refusals by.
fn transport_failure(error: reqwest::Error) -> FetchFailure {
    if error.is_timeout() {
        return FetchFailure::Timeout;
    }
    if error.is_connect() {
        return FetchFailure::Unreachable(error.to_string());
    }
    FetchFailure::Other(error.to_string())
}

/// A name to its addresses, through the system resolver.
///
/// It answers honestly and judges nothing: the ban on private, loopback and link-local
/// addresses is the executor's, applied to what this returns, so a name that resolves into
/// this network is refused there rather than quietly here.
#[derive(Clone, Copy, Debug, Default)]
pub struct RuleResolver;

#[async_trait]
impl HostResolver for RuleResolver {
    async fn resolve(&self, host: &str) -> Result<Vec<IpAddr>, String> {
        // A literal address never reaches this: the executor checks those itself and hands
        // the fetcher an empty address list, so `host` is always a name here.
        let addresses = tokio::net::lookup_host(format!("{host}:0"))
            .await
            .map_err(|error| error.to_string())?;
        Ok(addresses.map(|address| address.ip()).collect())
    }
}

/// The captcha broker, as a rule's `captcha` step needs it.
///
/// Only the widget kinds cross this line. An image captcha has no picture in a rule — the
/// step names a challenge kind and a site key — so a kind this does not know is refused
/// rather than guessed at, and the run ends with `site_rules.captcha_failed`.
pub struct RuleCaptcha {
    solver: Arc<dyn rd_plugin_api::CaptchaSolver>,
}

impl RuleCaptcha {
    #[must_use]
    pub fn new(solver: Arc<dyn rd_plugin_api::CaptchaSolver>) -> Self {
        Self { solver }
    }
}

#[async_trait]
impl rd_siterules::CaptchaSolver for RuleCaptcha {
    async fn solve(&self, request: CaptchaRequest) -> Result<String, String> {
        let site_key = request
            .sitekey
            .ok_or_else(|| format!("a {} challenge needs a site key", request.challenge))?;
        let widget = WidgetChallenge {
            site_key,
            page_url: request.page_url.to_string(),
            invisible: false,
        };
        let challenge = match request.challenge.as_str() {
            "recaptcha-v2" => CaptchaChallenge::RecaptchaV2(widget),
            "hcaptcha" => CaptchaChallenge::HCaptcha(widget),
            "turnstile" => CaptchaChallenge::Turnstile(widget),
            other => return Err(format!("no solver answers a {other} challenge")),
        };
        let limit = self.solver.allowance().await;
        let answer = self
            .solver
            .solve(challenge, limit)
            .await
            .map_err(|failure| failure.code.unwrap_or(failure.message))?;
        answer
            .token()
            .map(str::to_owned)
            .ok_or_else(|| "the answer is a point, which no rule can use".to_owned())
    }
}

#[cfg(test)]
#[path = "siterules_tests.rs"]
mod tests;
