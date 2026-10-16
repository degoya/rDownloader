//! The proxy the environment names, for the requests no proxy profile covers (RD-1330-01).
//!
//! A webhook, a Web Push message and the reconnect's IP check send through a service client,
//! which reqwest would hand the proxy `HTTP_PROXY`, `HTTPS_PROXY` or `ALL_PROXY` names — with the
//! target's name, for the proxy to resolve, after the address rule had judged what the name
//! answered here. No profile stands behind such a proxy, so none can say it resolves names
//! itself, and these requests always pin (owner, 2026-10-16): [`EnvironmentRoute::guarded`]
//! reads the variables as reqwest does — the same `hyper-util` matcher, so the lower-case
//! spellings, `NO_PROXY` and the rule that ignores them under CGI hold unchanged — and sends a
//! target with a proxy through the bridge of [`super::with_proxy`], names resolved here. A
//! network where only the proxy can resolve a name then fails with
//! [`super::CODE_NAME_UNRESOLVED`] instead of reaching the name unchecked.
//!
//! The client is built for one target, and a redirect hop takes the target's way: through the
//! same proxy, resolved and checked here, or directly with the guarded resolver. reqwest's own
//! reading of the environment is switched off either way, since it would hand a hop's name on.

use std::sync::Arc;

use anyhow::Result;
use base64::Engine as _;
use hyper_util::client::proxy::matcher::{Intercept, Matcher};
use reqwest::ClientBuilder;
use secrecy::SecretString;
use url::Url;
use zeroize::Zeroizing;

use crate::{AddressPolicy, GuardedResolver, HostLookup, ProxyCredentials, SystemLookup};

/// The proxies the environment names, read as reqwest reads them.
pub struct EnvironmentProxy(Matcher);

impl EnvironmentProxy {
    /// `ALL_PROXY`, `HTTPS_PROXY`, `HTTP_PROXY` and `NO_PROXY`, in upper or lower case, as the
    /// process has them now.
    #[must_use]
    pub fn from_env() -> Self {
        Self(Matcher::from_env())
    }

    /// As if `HTTP_PROXY`, `HTTPS_PROXY` and `NO_PROXY` said `http`, `https` and `no_proxy`; a
    /// test names its proxy this way and leaves the process's variables alone.
    #[must_use]
    pub fn given(http: Option<&str>, https: Option<&str>, no_proxy: Option<&str>) -> Self {
        let mut builder = Matcher::builder();
        if let Some(value) = http {
            builder = builder.http(value);
        }
        if let Some(value) = https {
            builder = builder.https(value);
        }
        if let Some(value) = no_proxy {
            builder = builder.no(value);
        }
        Self(builder.build())
    }

    /// The proxy for `target` and its login, or `None` when no variable names one for the
    /// target's scheme or `NO_PROXY` exempts its host.
    #[must_use]
    pub fn for_target(&self, target: &Url) -> Option<(Url, Option<ProxyCredentials>)> {
        let uri = target.as_str().parse::<http::Uri>().ok()?;
        let intercept = self.0.intercept(&uri)?;
        let endpoint = Url::parse(&intercept.uri().to_string()).ok()?;
        Some((endpoint, credentials(&intercept)))
    }
}

/// The login a proxy variable carried: as the user and password a SOCKS5 proxy takes, or as
/// the `Basic` value the matcher made of them for an HTTP(S) one, decoded again for the bridge.
fn credentials(intercept: &Intercept) -> Option<ProxyCredentials> {
    if let Some((username, password)) = intercept.raw_auth() {
        return Some(ProxyCredentials {
            username: username.to_owned(),
            password: SecretString::from(password.to_owned()),
        });
    }
    let value = intercept
        .basic_auth()?
        .to_str()
        .ok()?
        .strip_prefix("Basic ")?;
    let pair = Zeroizing::new(
        base64::engine::general_purpose::STANDARD
            .decode(value)
            .ok()?,
    );
    let pair = std::str::from_utf8(&pair).ok()?;
    let (username, password) = pair.split_once(':').unwrap_or((pair, ""));
    Some(ProxyCredentials {
        username: username.to_owned(),
        password: SecretString::from(password.to_owned()),
    })
}

/// How a request without a proxy profile reaches its target: one lookup for the check before
/// the request and for the connection, and the proxy the environment names.
pub struct EnvironmentRoute {
    proxies: EnvironmentProxy,
    lookup: Arc<dyn HostLookup>,
}

impl EnvironmentRoute {
    /// The system's resolver and the process's variables.
    #[must_use]
    pub fn system() -> Self {
        Self::new(EnvironmentProxy::from_env(), Arc::new(SystemLookup))
    }

    /// `proxies` and `lookup`; a test makes a name answer as it likes and names its own proxy.
    #[must_use]
    pub fn new(proxies: EnvironmentProxy, lookup: Arc<dyn HostLookup>) -> Self {
        Self { proxies, lookup }
    }

    /// The lookup a caller checks its target with before the request.
    #[must_use]
    pub fn lookup(&self) -> &dyn HostLookup {
        self.lookup.as_ref()
    }

    /// Gives `builder`, a client for requests to `target` under the address rule `guard`, its
    /// way there: through the proxy the environment names for `target`, which is handed the
    /// address checked at the connection and never the name, or with no proxy at all and the
    /// guarded resolver.
    pub fn guarded(
        &self,
        builder: ClientBuilder,
        target: &Url,
        guard: &AddressPolicy,
    ) -> Result<ClientBuilder> {
        match self.proxies.for_target(target) {
            Some((endpoint, credentials)) => super::pinned(
                builder,
                guard,
                &endpoint,
                credentials.as_ref(),
                &[],
                Arc::clone(&self.lookup),
            ),
            None => Ok(builder
                .no_proxy()
                .dns_resolver(GuardedResolver::with_lookup(
                    guard.clone(),
                    Arc::clone(&self.lookup),
                ))),
        }
    }
}
