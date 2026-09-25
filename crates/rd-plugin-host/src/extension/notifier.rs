//! Notification destinations (RD-090-15): one delivery attempt, no retry policy of its own.

use std::{
    net::{Ipv4Addr, Ipv6Addr},
    sync::Arc,
};

use anyhow::Result;
use rd_core::{Failure, FailureKind};
use rd_plugin_api::ResolverHost;
use url::{Host, Url};

use super::{ExtensionRuntime, bindings::notifier};
use crate::{PluginManifest, runtime::PluginStoreState};

/// A compiled notification destination.
pub struct NotifierPlugin {
    runtime: ExtensionRuntime,
    pre: notifier::NotifierPluginPre<PluginStoreState>,
}

impl NotifierPlugin {
    pub fn new(
        manifest: PluginManifest,
        component_bytes: &[u8],
        host: Option<Arc<dyn ResolverHost>>,
    ) -> Result<Self> {
        let (runtime, pre) = ExtensionRuntime::build(manifest, component_bytes, host)?;
        Ok(Self {
            runtime,
            pre: notifier::NotifierPluginPre::new(pre)?,
        })
    }

    #[must_use]
    pub fn manifest(&self) -> &PluginManifest {
        self.runtime.manifest()
    }

    /// Whether a destination would be refused at delivery, asked without delivering: the same
    /// decision and the same code, so a target can be refused when it is saved (RD-130-15)
    /// instead of failing on its first event.
    pub fn check_destination(&self, destination: &str) -> Result<(), Failure> {
        destination_reach(self.manifest().capabilities.domains(), destination).map(|_| ())
    }

    /// Delivers one notification. Whether to try again is the hub's decision, not the
    /// plugin's, so a failure comes back as a message rather than a retry.
    ///
    /// A failure the plugin reported arrives as an [`rd_core::Failure`] inside the error, so
    /// the hub can read its category (RD-120-62): a revoked token is `permanent` and must not
    /// be tried six times. So does a destination the host refuses before the plugin runs
    /// (RD-130-15), as `permanent`. Anything else — a trap, an instantiation error — carries
    /// none.
    pub async fn deliver(&self, message: Delivery<'_>) -> Result<()> {
        // Decided before anything runs, so a destination the host refuses never instantiates
        // a plugin and never has its token expanded.
        let reach = destination_reach(
            self.runtime.manifest().capabilities.domains(),
            message.destination,
        )
        .map_err(anyhow::Error::new)?;
        let mut store =
            self.runtime
                .store_reaching(None, message.secret_ref.map(str::to_owned), reach)?;
        let instance = self.pre.instantiate_async(&mut store).await?;
        // Everything but the destination is text the plugin will copy into its request, and
        // much of it was written by somebody else — a release name from a feed. The host would
        // expand a vault marker inside it like one the plugin wrote, so its braces are made
        // inert here, before any plugin sees it (RD-120-65). The destination is the person's
        // own configuration and stays as it was entered.
        let foreign = |text: &str| crate::foreign_text::inert(text).into_owned();
        let message = notifier::exports::rdownloader::plugin::notifier::Notification {
            title: foreign(message.title),
            body: foreign(message.body),
            event: foreign(message.event),
            severity: foreign(message.severity),
            idempotency_key: foreign(message.idempotency_key),
            destination: message.destination.to_owned(),
            has_secret: message.secret_ref.is_some(),
        };
        instance
            .rdownloader_plugin_notifier()
            .call_deliver(&mut store, &message)
            .await?
            .map_err(|failure| anyhow::Error::new(crate::component::from_wit_failure(failure)))
    }
}

/// One message on its way to one destination.
///
/// A struct rather than six positional arguments, because five of them are strings and a
/// caller that swaps two would produce a delivery that looks right and says the wrong thing.
#[derive(Clone, Copy, Debug)]
pub struct Delivery<'a> {
    pub title: &'a str,
    pub body: &'a str,
    pub event: &'a str,
    pub severity: &'a str,
    pub idempotency_key: &'a str,
    /// Where this destination sends, as configured. Never a secret.
    pub destination: &'a str,
    /// The vault reference the plugin's `{{secret}}` expands to, if the destination has one.
    pub secret_ref: Option<&'a str>,
}

/// The domains one delivery may reach (RD-130-15).
///
/// A manifest that names its services keeps them: Telegram's destination is a chat id and says
/// nothing about where it lives. A manifest that also declares `*` reads it the way a storage
/// destination does — "wherever the destination points" — and the grant that actually applies
/// is narrower than either: the host of a destination written as an address, or, for anything
/// else (a bare ntfy topic), the services the manifest named, without the `*`. So a token set
/// for one destination reaches that one host and no other, and the manifest's `*` is never
/// handed to the sandbox, which would read it as "anywhere".
///
/// `https` everywhere; `http` only to an address inside the person's own network, because the
/// token would otherwise cross the internet readable (owner's decision, 2026-09-25).
pub(crate) fn destination_reach(
    domains: &[String],
    destination: &str,
) -> Result<Vec<String>, Failure> {
    if !domains.iter().any(|domain| domain == "*") {
        return Ok(domains.to_vec());
    }
    let destination = destination.trim();
    let written_as_address = ["https://", "http://"].iter().any(|scheme| {
        destination
            .get(..scheme.len())
            .is_some_and(|start| start.eq_ignore_ascii_case(scheme))
    });
    if !written_as_address {
        return Ok(domains
            .iter()
            .filter(|domain| *domain != "*")
            .cloned()
            .collect());
    }
    let invalid = || {
        Failure::coded(
            FailureKind::Permanent,
            "plugin.destination_invalid",
            "The notification destination is not a usable web address",
        )
    };
    let url = Url::parse(destination).map_err(|_| invalid())?;
    let (Some(host), Some(name)) = (url.host(), url.host_str()) else {
        return Err(invalid());
    };
    if url.scheme() == "http" && !inside_own_network(&host) {
        return Err(Failure::coded(
            FailureKind::Permanent,
            "plugin.destination_not_encrypted",
            "A notification destination outside the local network needs https",
        ));
    }
    Ok(vec![name.to_ascii_lowercase()])
}

/// Whether an address can only be inside the person's own network: the private IPv4 ranges
/// (RFC 1918), loopback, link-local, IPv6 unique-local, the names `localhost`, `*.lan` and
/// `*.local`, and a single-label name such as `ntfy` — a Docker or LAN service name, since a
/// name on the public internet always has a dot. A name is taken at its word — what it
/// resolves to is not asked, since the question is what the person meant to configure, not
/// what a resolver answers today.
fn inside_own_network(host: &Host<&str>) -> bool {
    fn v4(address: Ipv4Addr) -> bool {
        address.is_private() || address.is_loopback() || address.is_link_local()
    }
    fn v6(address: Ipv6Addr) -> bool {
        address.is_loopback()
            || address.is_unicast_link_local()
            || address.is_unique_local()
            || address.to_ipv4_mapped().is_some_and(v4)
    }
    match host {
        Host::Ipv4(address) => v4(*address),
        Host::Ipv6(address) => v6(*address),
        Host::Domain(name) => {
            let name = name.trim_end_matches('.').to_ascii_lowercase();
            !name.contains('.') || name.ends_with(".lan") || name.ends_with(".local")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::destination_reach;

    fn ntfy() -> Vec<String> {
        vec!["ntfy.sh".to_owned(), "*".to_owned()]
    }

    fn reach(destination: &str) -> Vec<String> {
        destination_reach(&ntfy(), destination)
            .unwrap_or_else(|failure| panic!("{destination}: {failure}"))
    }

    fn refusal(destination: &str) -> String {
        destination_reach(&ntfy(), destination)
            .expect_err(destination)
            .code
            .unwrap_or_default()
    }

    #[test]
    fn a_bare_topic_reaches_the_named_service_and_never_the_catch_all() {
        assert_eq!(reach("downloads"), ["ntfy.sh"]);
        assert_eq!(reach("/downloads"), ["ntfy.sh"]);
        // Something that looks like a scheme but is not a web address is a topic like any
        // other, and gets the named service rather than the wildcard.
        assert_eq!(reach("ftp://files.example.org/x"), ["ntfy.sh"]);
    }

    #[test]
    fn an_address_reaches_its_own_host_and_nothing_else() {
        assert_eq!(
            reach("https://ntfy.example.org/alerts"),
            ["ntfy.example.org"]
        );
        assert_eq!(
            reach(" HTTPS://Ntfy.Example.ORG:8443/alerts "),
            ["ntfy.example.org"]
        );
        // Even the public service, written out, is narrowed to itself.
        assert_eq!(reach("https://ntfy.sh/downloads"), ["ntfy.sh"]);
    }

    #[test]
    fn plain_http_is_for_the_own_network_only() {
        for inside in [
            "http://192.168.1.20/alerts",
            "http://10.0.0.5:8080/alerts",
            "http://172.16.4.1/alerts",
            "http://127.0.0.1/alerts",
            "http://169.254.10.10/alerts",
            "http://[::1]/alerts",
            "http://[fd12:3456::1]/alerts",
            "http://[fe80::1]/alerts",
            "http://localhost:2586/alerts",
            // A single label is a service name in a Docker network or on the LAN.
            "http://ntfy:2586/alerts",
            "http://ntfy.lan/alerts",
            "http://pi.local/alerts",
        ] {
            assert_eq!(reach(inside).len(), 1, "{inside}");
        }
        for outside in [
            "http://ntfy.sh/alerts",
            "http://ntfy.example.org/alerts",
            "http://172.32.0.1/alerts",
            "http://8.8.8.8/alerts",
            "http://[2001:db8::1]/alerts",
            // A suffix that merely ends in the letters is not the suffix.
            "http://example.planlan/alerts",
        ] {
            assert_eq!(
                refusal(outside),
                "plugin.destination_not_encrypted",
                "{outside}"
            );
        }
    }

    #[test]
    fn an_address_without_a_host_is_refused() {
        assert_eq!(refusal("https://"), "plugin.destination_invalid");
    }

    #[test]
    fn a_manifest_without_the_catch_all_keeps_its_list_whatever_the_destination() {
        let telegram = vec!["api.telegram.org".to_owned()];
        assert_eq!(
            destination_reach(&telegram, "https://elsewhere.example/").expect("unchanged"),
            telegram
        );
        assert_eq!(
            destination_reach(&telegram, "-100123456").expect("unchanged"),
            telegram
        );
    }
}
