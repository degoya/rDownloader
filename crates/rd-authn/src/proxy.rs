//! The contract with whatever sits in front of the service.
//!
//! Four things have to agree once a reverse proxy is involved, and they are usually configured
//! in four different places: which hops may speak for a client, what the outside world calls
//! this service, what path it is mounted under, and whether cookies may travel unencrypted.
//! Disagreement between them is not a visible error — it is a login that silently fails, a
//! cookie a browser quietly drops, or a client address that is really the proxy's.
//!
//! So they are one setting here. The external URL carries the scheme, the host and the base
//! path together, which removes the commonest of those disagreements: an origin that says
//! `https` while the base path was configured for a deployment that used `http`, or a base
//! path set in one place and not the other.

use std::net::IpAddr;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::cidr::Cidr;

/// When the session cookie carries the `Secure` attribute.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CookieSecurity {
    /// Secure when the external URL is `https`. The right answer for almost every deployment,
    /// and it needs no second decision from the operator.
    #[default]
    Auto,
    /// Always secure. For a proxy that terminates TLS but is described by an `http` URL.
    Always,
    /// Never secure.
    ///
    /// Exists because a plain-HTTP LAN deployment is a real thing people run, and a `Secure`
    /// cookie there is simply dropped by the browser — the login appears to succeed and the
    /// next request is unauthenticated, with nothing to explain why.
    Never,
}

/// Why a proxy configuration was refused.
#[derive(Debug, thiserror::Error, Eq, PartialEq)]
pub enum ProxyConfigError {
    /// A trusted range did not parse.
    #[error("trusted proxy `{value}` is not an address or CIDR range")]
    TrustedRange { value: String },
    /// The external URL is not a URL.
    #[error("the external URL `{value}` could not be read as a URL")]
    ExternalUrl { value: String },
    /// The external URL uses a scheme browsers do not speak.
    #[error("the external URL must use http or https, not `{scheme}`")]
    ExternalScheme { scheme: String },
    /// The external URL carries something an origin cannot.
    #[error("the external URL must not contain {part}")]
    ExternalExtras { part: &'static str },
}

/// The resolved proxy contract.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ProxyConfig {
    trusted: Vec<Cidr>,
    /// Scheme and authority, without a trailing slash: `https://rd.example.com`.
    origin: Option<String>,
    /// Mount point, with a leading and no trailing slash, or empty for the root.
    base_path: String,
    cookie_security: CookieSecurity,
}

impl ProxyConfig {
    /// Reads a configuration, refusing anything it cannot act on.
    ///
    /// Refusing rather than repairing: a base path the operator did not write, silently
    /// derived from something malformed, is worse than being told the value is wrong.
    pub fn parse(
        trusted_proxies: &[String],
        external_url: Option<&str>,
        cookie_security: CookieSecurity,
    ) -> Result<Self, ProxyConfigError> {
        let mut trusted = Vec::new();
        for value in trusted_proxies {
            let value = value.trim();
            if value.is_empty() {
                continue;
            }
            trusted.push(
                Cidr::parse(value).map_err(|_| ProxyConfigError::TrustedRange {
                    value: value.to_owned(),
                })?,
            );
        }

        let (origin, base_path) = match external_url.map(str::trim).filter(|v| !v.is_empty()) {
            None => (None, String::new()),
            Some(value) => {
                let url = url::Url::parse(value).map_err(|_| ProxyConfigError::ExternalUrl {
                    value: value.to_owned(),
                })?;
                match url.scheme() {
                    "http" | "https" => {}
                    scheme => {
                        return Err(ProxyConfigError::ExternalScheme {
                            scheme: scheme.to_owned(),
                        });
                    }
                }
                if url.query().is_some() {
                    return Err(ProxyConfigError::ExternalExtras { part: "a query" });
                }
                if url.fragment().is_some() {
                    return Err(ProxyConfigError::ExternalExtras { part: "a fragment" });
                }
                if !url.username().is_empty() || url.password().is_some() {
                    return Err(ProxyConfigError::ExternalExtras {
                        part: "credentials",
                    });
                }
                if url.host_str().is_none() {
                    return Err(ProxyConfigError::ExternalExtras { part: "no host" });
                }
                let origin = match url.port() {
                    Some(port) => format!(
                        "{}://{}:{port}",
                        url.scheme(),
                        url.host_str().unwrap_or_default()
                    ),
                    None => format!("{}://{}", url.scheme(), url.host_str().unwrap_or_default()),
                };
                (Some(origin), normalise_base_path(url.path()))
            }
        };

        Ok(Self {
            trusted,
            origin,
            base_path,
            cookie_security,
        })
    }

    /// The ranges whose forwarded headers are believed.
    #[must_use]
    pub fn trusted(&self) -> &[Cidr] {
        &self.trusted
    }

    /// Whether `address` is one of the configured proxies.
    #[must_use]
    pub fn trusts(&self, address: IpAddr) -> bool {
        crate::client_ip::is_trusted(address, &self.trusted)
    }

    /// The origin browsers see, if the operator declared one.
    ///
    /// This is what a WebAuthn ceremony has to be bound to and what a strict CORS or CSRF
    /// check compares against; both are wrong if they use the address the service happens to
    /// be bound to instead.
    #[must_use]
    pub fn origin(&self) -> Option<&str> {
        self.origin.as_deref()
    }

    /// The host part of the origin, which is what WebAuthn calls the relying party id.
    #[must_use]
    pub fn relying_party_id(&self) -> Option<String> {
        let origin = self.origin.as_deref()?;
        let url = url::Url::parse(origin).ok()?;
        url.host_str().map(str::to_owned)
    }

    /// The mount point: `/downloads`, or empty at the root.
    #[must_use]
    pub fn base_path(&self) -> &str {
        &self.base_path
    }

    /// Whether the session cookie should be marked `Secure`.
    #[must_use]
    pub fn cookie_is_secure(&self) -> bool {
        match self.cookie_security {
            CookieSecurity::Always => true,
            CookieSecurity::Never => false,
            CookieSecurity::Auto => self
                .origin
                .as_deref()
                .is_some_and(|origin| origin.starts_with("https://")),
        }
    }

    /// Warnings worth telling an operator about, in the order they matter.
    ///
    /// Returned rather than logged so both `doctor` and the settings page can show the same
    /// text. Every entry describes a configuration that will *work* and then behave in a way
    /// nobody would connect back to this setting.
    #[must_use]
    pub fn warnings(&self) -> Vec<&'static str> {
        let mut warnings = Vec::new();
        if !self.trusted.is_empty() && self.origin.is_none() {
            warnings.push(
                "Trusted proxies are configured but no external URL is set, so cookies cannot \
                 be marked Secure automatically and sign-in links will point at the internal \
                 address.",
            );
        }
        if self.origin.is_some() && self.trusted.is_empty() {
            warnings.push(
                "An external URL is set but no trusted proxies are, so every request will look \
                 as though it came from the proxy: rate limiting and the session list will show \
                 one address for everybody.",
            );
        }
        if self.cookie_security == CookieSecurity::Never
            && self
                .origin
                .as_deref()
                .is_some_and(|origin| origin.starts_with("https://"))
        {
            warnings.push(
                "The external URL is https but the session cookie is configured never to be \
                 Secure, which sends it over plain HTTP if anything ever reaches the service \
                 that way.",
            );
        }
        if self.cookie_security == CookieSecurity::Always
            && self
                .origin
                .as_deref()
                .is_some_and(|origin| origin.starts_with("http://"))
        {
            warnings.push(
                "The session cookie is forced Secure while the external URL is plain http. A \
                 browser will drop the cookie, so signing in will appear to succeed and the \
                 next request will not be authenticated.",
            );
        }
        warnings
    }
}

/// `/downloads/` and `downloads` both become `/downloads`; `/` becomes empty.
fn normalise_base_path(path: &str) -> String {
    let trimmed = path.trim().trim_matches('/');
    if trimmed.is_empty() {
        String::new()
    } else {
        format!("/{trimmed}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(trusted: &[&str], url: Option<&str>, security: CookieSecurity) -> ProxyConfig {
        let trusted: Vec<String> = trusted.iter().map(|value| (*value).to_owned()).collect();
        ProxyConfig::parse(&trusted, url, security).expect("configuration")
    }

    /// The default has to be the safe one: nothing trusted, nothing assumed.
    #[test]
    fn the_empty_configuration_trusts_nothing() {
        let config = ProxyConfig::default();
        assert!(config.trusted().is_empty());
        assert_eq!(config.origin(), None);
        assert_eq!(config.base_path(), "");
        assert!(!config.cookie_is_secure());
    }

    #[test]
    fn an_external_url_yields_an_origin_and_a_base_path() {
        let config = config(
            &[],
            Some("https://rd.example.com/downloads/"),
            CookieSecurity::Auto,
        );
        assert_eq!(config.origin(), Some("https://rd.example.com"));
        assert_eq!(config.base_path(), "/downloads");
        assert_eq!(config.relying_party_id().as_deref(), Some("rd.example.com"));
        assert!(config.cookie_is_secure());
    }

    #[test]
    fn a_non_default_port_stays_part_of_the_origin() {
        let config = config(
            &[],
            Some("https://rd.example.com:8443"),
            CookieSecurity::Auto,
        );
        assert_eq!(config.origin(), Some("https://rd.example.com:8443"));
        // The relying party id is the host alone: WebAuthn ignores the port.
        assert_eq!(config.relying_party_id().as_deref(), Some("rd.example.com"));
    }

    #[test]
    fn a_root_mount_has_an_empty_base_path() {
        for url in ["https://rd.example.com", "https://rd.example.com/"] {
            assert_eq!(config(&[], Some(url), CookieSecurity::Auto).base_path(), "");
        }
    }

    /// Auto is the whole point of the default: no second decision, and it follows the scheme.
    #[test]
    fn the_cookie_follows_the_scheme_by_default() {
        assert!(
            config(&[], Some("https://rd.example.com"), CookieSecurity::Auto).cookie_is_secure()
        );
        assert!(
            !config(&[], Some("http://rd.example.com"), CookieSecurity::Auto).cookie_is_secure()
        );
        assert!(!config(&[], None, CookieSecurity::Auto).cookie_is_secure());
    }

    /// A plain-HTTP LAN deployment is real, and a Secure cookie there is silently dropped.
    #[test]
    fn the_cookie_setting_can_be_forced_either_way() {
        assert!(config(&[], Some("http://rd.lan"), CookieSecurity::Always).cookie_is_secure());
        assert!(!config(&[], Some("https://rd.lan"), CookieSecurity::Never).cookie_is_secure());
    }

    #[test]
    fn a_malformed_range_is_refused_rather_than_ignored() {
        let trusted = vec!["10.0.0.0/8".to_owned(), "nonsense".to_owned()];
        assert!(matches!(
            ProxyConfig::parse(&trusted, None, CookieSecurity::Auto),
            Err(ProxyConfigError::TrustedRange { .. })
        ));
    }

    /// Blank entries come from a textarea with a stray newline; they are not an error.
    #[test]
    fn blank_range_entries_are_skipped() {
        let trusted = vec!["10.0.0.0/8".to_owned(), "  ".to_owned(), String::new()];
        let config = ProxyConfig::parse(&trusted, None, CookieSecurity::Auto).expect("config");
        assert_eq!(config.trusted().len(), 1);
    }

    #[test]
    fn an_external_url_must_be_something_a_browser_can_reach() {
        for value in ["not a url", "ftp://rd.example.com", "/just/a/path"] {
            assert!(
                ProxyConfig::parse(&[], Some(value), CookieSecurity::Auto).is_err(),
                "{value} was accepted"
            );
        }
    }

    /// An origin carrying a query, a fragment or credentials is a copy-paste from a browser
    /// bar, not a deployment address; taking the host out of it silently would hide the slip.
    #[test]
    fn an_external_url_may_not_carry_extras() {
        for value in [
            "https://rd.example.com/?tab=1",
            "https://rd.example.com/#top",
            "https://user:pass@rd.example.com/",
        ] {
            assert!(
                matches!(
                    ProxyConfig::parse(&[], Some(value), CookieSecurity::Auto),
                    Err(ProxyConfigError::ExternalExtras { .. })
                ),
                "{value} was accepted"
            );
        }
    }

    #[test]
    fn trust_is_answered_from_the_configured_ranges() {
        let config = config(&["10.0.0.0/8"], None, CookieSecurity::Auto);
        assert!(config.trusts("10.1.2.3".parse().expect("address")));
        assert!(!config.trusts("203.0.113.9".parse().expect("address")));
    }

    /// The half-configured cases are the ones that work and then behave inexplicably.
    #[test]
    fn a_half_configured_deployment_is_warned_about() {
        let only_proxies = config(&["10.0.0.0/8"], None, CookieSecurity::Auto);
        assert!(!only_proxies.warnings().is_empty());

        let only_url = config(&[], Some("https://rd.example.com"), CookieSecurity::Auto);
        assert!(!only_url.warnings().is_empty());

        let complete = config(
            &["10.0.0.0/8"],
            Some("https://rd.example.com"),
            CookieSecurity::Auto,
        );
        assert!(complete.warnings().is_empty(), "{:?}", complete.warnings());
    }

    /// A forced-Secure cookie on a plain-http deployment is the one that looks like a broken
    /// login and is really a configuration mistake.
    #[test]
    fn forcing_a_secure_cookie_over_plain_http_is_warned_about() {
        let config = config(
            &["10.0.0.0/8"],
            Some("http://rd.lan"),
            CookieSecurity::Always,
        );
        assert!(
            config
                .warnings()
                .iter()
                .any(|warning| warning.contains("drop the cookie"))
        );
    }
}
