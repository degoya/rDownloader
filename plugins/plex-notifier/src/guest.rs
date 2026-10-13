//! The component: one finished package, one Plex library refresh.

use plugin_guest_notifier::{
    Guest, Notification, delivered, destination_settings,
    http::{self, RequestQuery},
    refuse,
    types::{Failure, FailureKind},
};

use crate::request;

/// The one code every refusal of this plugin carries.
const REJECTED: &str = "plex_notifier.rejected";

struct Component;

impl Guest for Component {
    fn deliver(message: Notification) -> Result<(), Failure> {
        if !request::wanted(&message.event) {
            return Ok(());
        }
        let named = destination_settings::setting(request::SECTION_SETTING);
        let section = request::section(named.as_deref())
            .map_err(|why| refuse(REJECTED, why, FailureKind::Permanent))?;
        // Refused by the host before the plugin runs when the target is saved (RD-130-15);
        // checked here as well, so nothing is ever sent to a guessed address.
        let Some(endpoint) = request::endpoint(&message.destination, section) else {
            return Err(refuse(
                REJECTED,
                "This destination is not the address of a Plex server",
                FailureKind::Permanent,
            ));
        };
        let mut query = Vec::new();
        let mapping = destination_settings::setting(request::MAPPING_SETTING).unwrap_or_default();
        if let Some(folder) = request::folder(section, &mapping, message.package_path.as_deref()) {
            query.push(RequestQuery {
                name: request::PATH_QUERY.to_owned(),
                value_template: folder,
            });
        }
        // A server that admits its own network without sign-in needs no token, so the query is
        // only added when one is stored. The host substitutes it on the way out; the plugin
        // never sees it, and neither the message below nor the host's own report of a failed
        // request carries the address it was expanded into.
        if message.has_secret {
            query.push(RequestQuery {
                name: request::TOKEN_QUERY.to_owned(),
                value_template: "{{secret}}".to_owned(),
            });
        }
        let response = http::http_request("GET", &endpoint, &query, &[], &[])?;
        // A 401 is a wrong token and a 404 a wrong address or section, neither of which time
        // fixes; a 5xx is a server that is starting or busy, and is tried again.
        delivered(response.status, "Plex", REJECTED)
    }
}

plugin_guest_notifier::notifier_plugin!(Component);
