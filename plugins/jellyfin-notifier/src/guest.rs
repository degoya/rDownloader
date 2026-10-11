//! The component: one finished package, one Jellyfin library refresh.
#![allow(unsafe_code)] // Generated canonical-ABI exports contain the only unsafe code here.

use plugin_guest_notifier::{
    Guest, Notification, delivered,
    http::{self, RequestHeader},
    refuse,
    types::{Failure, FailureKind},
};

use crate::request;

/// The one code every refusal of this plugin carries.
const REJECTED: &str = "jellyfin_notifier.rejected";

struct Component;

impl Guest for Component {
    fn deliver(message: Notification) -> Result<(), Failure> {
        if !request::wanted(&message.event) {
            return Ok(());
        }
        // Refreshing a library is an administrator's request: without a key Jellyfin answers
        // 401 every time, so this fails before anything is sent.
        if !message.has_secret {
            return Err(refuse(
                REJECTED,
                "This destination has no API key stored",
                FailureKind::AuthRequired,
            ));
        }
        // Refused by the host before the plugin runs when the target is saved (RD-130-15);
        // checked here as well, so nothing is ever sent to a guessed address.
        let Some(endpoint) = request::endpoint(&message.destination) else {
            return Err(refuse(
                REJECTED,
                "This destination is not the address of a Jellyfin server",
                FailureKind::Permanent,
            ));
        };
        let response = http::http_request(
            "POST",
            &endpoint,
            &[],
            &[RequestHeader {
                name: "Authorization".to_owned(),
                value_template: request::AUTHORIZATION.to_owned(),
            }],
            &[],
        )?;
        // A 401 is a wrong key and a 403 one without administrator rights, neither of which time
        // fixes; a 5xx is a server that is starting or busy, and is tried again.
        delivered(response.status, "Jellyfin", REJECTED)
    }
}

plugin_guest_notifier::notifier_plugin!(Component);
