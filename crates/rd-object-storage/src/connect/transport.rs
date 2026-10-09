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
//! [`super::client_options`] sets it: the user agent, the proxy, the custom CA, the timeouts,
//! the guarded resolver, plain HTTP only where allowed, no compression. The options a builder
//! changes for one client of its own — plain HTTP and a one-second connect timeout for a
//! cloud's metadata service — are read from the options it hands over.

use std::{net::SocketAddr, sync::Arc, time::Duration};

use object_store::client::{
    ClientConfigKey, ClientOptions, DnsResolver, HttpClient, HttpConnector,
};

/// The connect timeout `object_store` gives a client for a cloud's metadata service.
const METADATA_CONNECT_TIMEOUT: Duration = Duration::from_secs(1);

/// Builds every client of a store without a redirect policy.
#[derive(Debug)]
pub(crate) struct NoRedirects {
    /// The custom CA material every transport trusts beside the system roots, as PEM bundles.
    pub(crate) custom_ca_pem: Vec<Vec<u8>>,
    /// The profile's connect and read timeout.
    pub(crate) timeout: Duration,
}

impl HttpConnector for NoRedirects {
    fn connect(&self, options: &ClientOptions) -> object_store::Result<HttpClient> {
        let client = self
            .client(options)
            .map_err(|error| object_store::Error::Generic {
                store: "HTTP client",
                source: Box::new(error),
            })?;
        Ok(HttpClient::new(client))
    }
}

impl NoRedirects {
    fn client(&self, options: &ClientOptions) -> reqwest::Result<reqwest::Client> {
        let value = |key: ClientConfigKey| options.get_config_value(&key);
        let mut builder = reqwest::Client::builder()
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
        if let Some(proxy) = value(ClientConfigKey::ProxyUrl) {
            builder = builder.proxy(reqwest::Proxy::all(proxy)?);
        }
        for pem in &self.custom_ca_pem {
            for certificate in reqwest::Certificate::from_pem_bundle(pem)? {
                builder = builder.add_root_certificate(certificate);
            }
        }
        if let Some(resolver) = options.dns_resolver() {
            builder = builder.dns_resolver(Resolver(Arc::clone(resolver)));
        }
        builder.build()
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
mod tests {
    use std::time::Duration;

    use object_store::client::ClientOptions;

    use super::NoRedirects;

    fn connector() -> NoRedirects {
        NoRedirects {
            custom_ca_pem: Vec::new(),
            timeout: Duration::from_secs(20),
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
