//! The component: one finished package, one Plex library refresh.
#![allow(unsafe_code)] // Generated canonical-ABI exports contain the only unsafe code here.

use plugin_guest_notifier::{
    Guest, Notification, delivered,
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
        // Refused by the host before the plugin runs when the target is saved (RD-130-15);
        // checked here as well, so nothing is ever sent to a guessed address.
        let Some(endpoint) = request::endpoint(&message.destination) else {
            return Err(refuse(
                REJECTED,
                "This destination is not the address of a Plex server",
                FailureKind::Permanent,
            ));
        };
        // A server that admits its own network without sign-in needs no token, so the query is
        // only added when one is stored. The host substitutes it on the way out; the plugin
        // never sees it, and neither the message below nor the host's own report of a failed
        // request carries the address it was expanded into.
        let mut query = Vec::new();
        if message.has_secret {
            query.push(RequestQuery {
                name: request::TOKEN_QUERY.to_owned(),
                value_template: "{{secret}}".to_owned(),
            });
        }
        let response = http::http_request("GET", &endpoint, &query, &[], &[])?;
        // A 401 is a wrong token and a 404 a wrong address, neither of which time fixes; a 5xx
        // is a server that is starting or busy, and is tried again.
        delivered(response.status, "Plex", REJECTED)
    }
}

plugin_guest_notifier::notifier_plugin!(Component);
