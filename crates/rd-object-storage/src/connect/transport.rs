//! The HTTP client every store sends through, which follows no redirect (RD-1200-06).
//!
//! `object_store` builds its reqwest client with reqwest's default redirect policy, ten hops,
//! and sets none of its own. A hop to a literal address never asks the resolver the endpoint
//! is held to ([`crate::endpoint::GuardedDns`]), so an endpoint that passed the address rule
//! could hand a request on to a cloud's metadata service or to one of rDownloader's own
//! listeners. No object storage API redirects a correct request, so this client follows none:
//! the 3xx comes back to `object_store` as the answer, which reports it as a refused request,
//! and `error::classify` names it `object_storage.redirect_refused`.
//!
//! Everything else is what `object_store`'s own client takes from [`ClientOptions`] as
//! [`super::client_options`] sets it: the user agent, the custom CA, the timeouts, the guarded
//! resolver, plain HTTP only where allowed, no compression. The options a builder changes for
//! one client of its own — plain HTTP and a one-second connect timeout for a cloud's metadata
//! service — are read from the options it hands over.
//!
//! The proxy is this client's own ([`StoreProxy`], RD-1330-01): behind it an endpoint held to
//! the address rule is pinned like every other guarded request ([`rd_http::with_proxy`]) — the
//! proxy is handed the address checked when the connection is made, never the name — unless
//! the proxy's profile says it resolves names itself. `object_store`'s `with_proxy_url` could
//! only hand reqwest the proxy, which would hand it the name.
//!
//! Without a proxy profile an entered endpoint takes the proxy the environment names
//! (`HTTP_PROXY`, `HTTPS_PROXY`, `NO_PROXY`) the same way, always pinned and with no setting of
//! its own, as a webhook does ([`EnvironmentWay`], owner 2026-10-16): no profile stands behind
//! such a proxy to say it resolves names itself.

use std::{fmt, net::SocketAddr, sync::Arc, time::Duration};

use object_store::client::{
    ClientConfigKey, ClientOptions, DnsResolver, HttpClient, HttpConnector,
};
use url::Url;

/// The connect timeout `object_store` gives a client for a cloud's metadata service.
const METADATA_CONNECT_TIMEOUT: Duration = Duration::from_secs(1);

/// The proxy a store's clients go through: the global proxy profile, its login read from the
/// vault. Never logged with its login.
pub(crate) struct StoreProxy {
    pub(crate) endpoint: Url,
    pub(crate) credentials: Option<rd_http::ProxyCredentials>,
    /// The profile's "the proxy resolves names itself" (`resolves_names`).
    pub(crate) resolution: rd_http::NameResolution,
    /// Resolves the targets and the proxy's own name behind a pinned proxy: the system's, or a
    /// test's own.
    pub(crate) lookup: Arc<dyn rd_http::HostLookup>,
}

impl fmt::Debug for StoreProxy {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StoreProxy")
            .field("endpoint", &self.endpoint.origin().ascii_serialization())
            .field("resolution", &self.resolution)
            .finish_non_exhaustive()
    }
}

/// The way to an entered endpoint when no proxy profile is set: through the proxy the
/// environment names for it, handed the checked address, or directly under the guard.
pub(crate) struct EnvironmentWay {
    /// The endpoint as the store's requests address it, the bucket in the host for a virtual
    /// host, so `NO_PROXY` is matched against the host that is reached.
    pub(crate) target: Url,
    /// The system's resolver and the process's variables, or a test's own.
    pub(crate) route: rd_http::EnvironmentRoute,
}

impl fmt::Debug for EnvironmentWay {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EnvironmentWay")
            .field("target", &self.target.origin().ascii_serialization())
            .finish_non_exhaustive()
    }
}

/// Builds every client of a store without a redirect policy.
#[derive(Debug)]
pub(crate) struct NoRedirects {
    /// The custom CA material every transport trusts beside the system roots, as PEM bundles.
    pub(crate) custom_ca_pem: Vec<Vec<u8>>,
    /// The profile's connect and read timeout.
    pub(crate) timeout: Duration,
    pub(crate) proxy: Option<Arc<StoreProxy>>,
    /// Without [`Self::proxy`], for an endpoint held to [`Self::guard`]; `None` leaves the
    /// environment's proxy to reqwest, as for a provider's own service.
    pub(crate) environment: Option<Arc<EnvironmentWay>>,
    /// The address rule the profile's endpoint keeps to (RD-1190-18); `None` for a provider's
    /// own service, which has no rule to keep behind a proxy either.
    pub(crate) guard: Option<rd_http::AddressPolicy>,
}

impl HttpConnector for NoRedirects {
    fn connect(&self, options: &ClientOptions) -> object_store::Result<HttpClient> {
        let client = self
            .client(options)
            .map_err(|error| object_store::Error::Generic {
                store: "HTTP client",
                source: error.into(),
            })?;
        Ok(HttpClient::new(client))
    }
}

impl NoRedirects {
    fn client(&self, options: &ClientOptions) -> anyhow::Result<reqwest::Client> {
        let value = |key: ClientConfigKey| options.get_config_value(&key);
        // The bucket's own user agent, from the options, replaces the service's.
        let mut builder = rd_http::client_builder(rd_http::ClientPurpose::Service)
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(value(ClientConfigKey::UserAgent).unwrap_or_default())
            .connect_timeout(self.connect_timeout(options))
            .read_timeout(self.timeout)
            .http1_only()
            // As `object_store` does: compression would falsify the `Content-Length` an
            // object's size is read from.
            .no_gzip()
            .no_brotli()
            .no_zstd()
            .no_deflate()
            .https_only(value(ClientConfigKey::AllowHttp).as_deref() != Some("true"));
        for pem in &self.custom_ca_pem {
            for certificate in reqwest::Certificate::from_pem_bundle(pem)? {
                builder = builder.add_root_certificate(certificate);
            }
        }
        let pinned = match (&self.proxy, &self.environment, &self.guard) {
            (Some(proxy), _, _) => {
                builder = rd_http::with_proxy_lookup(
                    builder,
                    &proxy.endpoint,
                    proxy.credentials.as_ref(),
                    self.guard.as_ref(),
                    proxy.resolution,
                    &self.custom_ca_pem,
                    Arc::clone(&proxy.lookup),
                )?;
                self.guard.is_some() && proxy.resolution == rd_http::NameResolution::Here
            }
            (None, Some(way), Some(guard)) => {
                builder = way.route.guarded(builder, &way.target, guard)?;
                true
            }
            _ => false,
        };
        // Behind a pinned proxy the bridge's guarded resolver is the one, and without one the
        // route's own guarded resolver; this would replace it.
        if let Some(resolver) = options.dns_resolver()
            && !pinned
        {
            builder = builder.dns_resolver(Resolver(Arc::clone(resolver)));
        }
        Ok(builder.build()?)
    }

    /// The metadata service's one second where the builder asked for it, the profile's
    /// timeout everywhere else. The options print a duration only as text, so the metadata
    /// value is compared in the same form.
    fn connect_timeout(&self, options: &ClientOptions) -> Duration {
        let metadata = ClientOptions::new()
            .with_connect_timeout(METADATA_CONNECT_TIMEOUT)
            .get_config_value(&ClientConfigKey::ConnectTimeout);
        if options.get_config_value(&ClientConfigKey::ConnectTimeout) == metadata {
            METADATA_CONNECT_TIMEOUT
        } else {
            self.timeout
        }
    }
}

/// `object_store`'s resolver as reqwest's: the guarded one an entered endpoint connects through.
struct Resolver(Arc<dyn DnsResolver>);

impl reqwest::dns::Resolve for Resolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let pending = self.0.resolve(name.as_str());
        Box::pin(async move {
            // Port 0: the connector sets the URL's port on every address it is handed.
            let addresses: reqwest::dns::Addrs = Box::new(
                pending
                    .await?
                    .into_iter()
                    .map(|address| SocketAddr::new(address, 0)),
            );
            Ok(addresses)
        })
    }
}

#[cfg(test)]
#[path = "transport_proxy_tests.rs"]
mod proxy_tests;

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use object_store::client::ClientOptions;

    use super::NoRedirects;

    fn connector() -> NoRedirects {
        NoRedirects {
            custom_ca_pem: Vec::new(),
            timeout: Duration::from_secs(20),
            proxy: None,
            environment: None,
            guard: None,
        }
    }

    /// A metadata service off the cloud does not answer at all; a second is what
    /// `object_store` waits for it, not the profile's timeout.
    #[test]
    fn the_metadata_client_keeps_its_short_connect_timeout() {
        let profile = ClientOptions::new().with_connect_timeout(Duration::from_secs(20));
        assert_eq!(
            connector().connect_timeout(&profile),
            Duration::from_secs(20)
        );
        let metadata = profile
            .with_allow_http(true)
            .with_connect_timeout(Duration::from_secs(1));
        assert_eq!(
            connector().connect_timeout(&metadata),
            Duration::from_secs(1)
        );
    }
}
