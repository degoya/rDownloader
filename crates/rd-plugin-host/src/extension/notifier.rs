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
use crate::{PluginManifest, SettingManifest, runtime::PluginStoreState};

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
        destination_reach(
            self.manifest().capabilities.domains(),
            destination,
            &crate::own_endpoints::current(),
        )
        .map(|_| ())
    }

    /// Whether a target's settings are ones this destination offers, asked when the target
    /// is saved (RD-170-09): every name declared in the manifest, every value one of its
    /// choices. An empty value leaves the setting at its default.
    pub fn check_settings(&self, settings: &[(String, String)]) -> Result<(), Failure> {
        check_settings(self.declared_settings(), settings)
    }

    fn declared_settings(&self) -> &[SettingManifest] {
        self.manifest()
            .extension
            .as_ref()
            .map(|extension| extension.settings.as_slice())
            .unwrap_or_default()
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
            &crate::own_endpoints::current(),
        )
        .map_err(anyhow::Error::new)?;
        let mut store =
            self.runtime
                .store_reaching(None, message.secret_ref.map(str::to_owned), reach)?;
        store.data_mut().own_network = reaches_supplied_address(
            self.runtime.manifest().capabilities.domains(),
            message.destination,
        );
        store.data_mut().destination_settings =
            resolve_settings(self.declared_settings(), message.settings);
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
    /// The target's settings as stored (RD-170-09), name and value. Resolved against the
    /// manifest before the plugin runs, so a name it does not declare never reaches it.
    pub settings: &'a [(String, String)],
}

/// `destination-settings`: the settings of the target this delivery goes to, and nothing else.
impl notifier::rdownloader::plugin::destination_settings::Host for PluginStoreState {
    async fn setting(&mut self, name: String) -> Option<String> {
        self.destination_settings
            .iter()
            .find(|(declared, _)| *declared == name)
            .map(|(_, value)| value.clone())
    }
}

/// A target's `config.settings`, read as the name-to-text object it is stored as.
///
/// Refused rather than read leniently when something else is there, because the only way to
/// get there is a request that did not come from the target editor; at delivery the caller
/// falls back to the defaults instead.
pub fn settings_from_config(config: &serde_json::Value) -> Result<Vec<(String, String)>, Failure> {
    let Some(value) = config.get("settings") else {
        return Ok(Vec::new());
    };
    if value.is_null() {
        return Ok(Vec::new());
    }
    let invalid = || {
        Failure::coded(
            FailureKind::Permanent,
            "plugin.setting_invalid",
            "A setting of this notification target has a value its destination does not offer",
        )
    };
    value
        .as_object()
        .ok_or_else(invalid)?
        .iter()
        .map(|(name, value)| {
            value
                .as_str()
                .map(|text| (name.clone(), text.to_owned()))
                .ok_or_else(invalid)
        })
        .collect()
}

/// Whether every chosen setting is one the manifest declares, with a value it offers.
fn check_settings(
    declared: &[SettingManifest],
    chosen: &[(String, String)],
) -> Result<(), Failure> {
    for (name, value) in chosen {
        let Some(setting) = declared.iter().find(|setting| setting.name == *name) else {
            return Err(Failure::coded(
                FailureKind::Permanent,
                "plugin.setting_unknown",
                "This notification destination has no setting of that name",
            ));
        };
        if !value.is_empty() && !setting.choices.contains(value) {
            return Err(Failure::coded(
                FailureKind::Permanent,
                "plugin.setting_invalid",
                "A setting of this notification target has a value its destination does not offer",
            ));
        }
    }
    Ok(())
}

/// What the plugin is told for each setting its manifest declares: the stored value when it is
/// one of the choices, else the default. A value the manifest stopped offering after the target
/// was saved -- a newer version of the plugin -- falls back rather than failing the delivery.
fn resolve_settings(
    declared: &[SettingManifest],
    chosen: &[(String, String)],
) -> Vec<(String, String)> {
    declared
        .iter()
        .filter_map(|setting| {
            let stored = chosen
                .iter()
                .find(|(name, value)| *name == setting.name && setting.choices.contains(value))
                .map(|(_, value)| value.clone());
            stored
                .or_else(|| setting.default.clone())
                .map(|value| (setting.name.clone(), value))
        })
        .collect()
}

/// The domains one delivery may reach (RD-130-15).
///
/// A manifest that names its services keeps them: Telegram's destination is a chat id and says
/// nothing about where it lives. A manifest that also declares `*` reads it the way a storage
/// destination does — "wherever the destination points" — and the grant that actually applies
/// is narrower than either: the host of a destination written as an address, or, for anything
/// else (a bare ntfy topic), the services the manifest named, without the `*`. So a token set
/// for one destination reaches that one host and no other, and the manifest's `*` is never
/// handed to the sandbox, which would read it as "anywhere". A manifest whose only domain is
/// `*` names no service to fall back on, so a destination that is not an address is refused.
///
/// `https` everywhere; `http` only to an address inside the person's own network, because the
/// token would otherwise cross the internet readable (owner's decision, 2026-09-25).
pub(crate) fn destination_reach(
    domains: &[String],
    destination: &str,
    own: &crate::OwnEndpoints,
) -> Result<Vec<String>, Failure> {
    if !domains.iter().any(|domain| domain == "*") {
        return Ok(domains.to_vec());
    }
    let destination = destination.trim();
    let invalid = || {
        Failure::coded(
            FailureKind::Permanent,
            "plugin.destination_invalid",
            "The notification destination is not a usable web address",
        )
    };
    if !written_as_address(destination) {
        let named: Vec<String> = domains
            .iter()
            .filter(|domain| *domain != "*")
            .cloned()
            .collect();
        // A manifest that names no service of its own -- a media server's library refresh,
        // RD-1240-12 -- has nowhere to send anything but an address, so anything else is
        // refused when the target is saved rather than failing on its first event.
        if named.is_empty() {
            return Err(invalid());
        }
        return Ok(named);
    }
    let url = Url::parse(destination).map_err(|_| invalid())?;
    let (Some(host), Some(name)) = (url.host(), url.host_str()) else {
        return Err(invalid());
    };
    // The request would be refused anyway (RA-HOST-01); refused here, the target is never
    // saved as one that cannot be delivered to. The rule is the request's own, for an address
    // the person entered: loopback is fine, the service's own ports and link-local are not.
    if refused_on_this_machine(&host, &url, own) {
        return Err(own.refused(&url));
    }
    if url.scheme() == "http" && !inside_own_network(&host) {
        return Err(Failure::coded(
            FailureKind::Permanent,
            "plugin.destination_not_encrypted",
            "A notification destination outside the local network needs https",
        ));
    }
    Ok(vec![name.to_ascii_lowercase()])
}

/// Whether a destination is a web address rather than a bare topic or channel name.
fn written_as_address(destination: &str) -> bool {
    let destination = destination.trim();
    ["https://", "http://"].iter().any(|scheme| {
        destination
            .get(..scheme.len())
            .is_some_and(|start| start.eq_ignore_ascii_case(scheme))
    })
}

/// Whether [`destination_reach`] narrowed the manifest's `*` to the address the person
/// entered — the one case a delivery may reach their own network (RA-HOST-01). A bare topic
/// goes to the services the manifest named, which are public.
pub(crate) fn reaches_supplied_address(domains: &[String], destination: &str) -> bool {
    domains.iter().any(|domain| domain == "*") && written_as_address(destination)
}

/// Whether the request rule for an entered address refuses `url` before any lookup: a literal
/// address it does not permit, or `localhost` on one of the service's own ports. A name that
/// resolves there is caught when the request is made.
fn refused_on_this_machine(host: &Host<&str>, url: &Url, own: &crate::OwnEndpoints) -> bool {
    let policy = own.policy(true, url);
    match host {
        Host::Ipv4(address) => !policy.permits((*address).into()),
        Host::Ipv6(address) => !policy.permits((*address).into()),
        Host::Domain(name) => {
            let name = name.trim_end_matches('.').to_ascii_lowercase();
            (name == "localhost" || name.ends_with(".localhost"))
                && own.is_own_port(url.port_or_known_default())
        }
    }
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
#[path = "notifier_tests.rs"]
mod tests;
