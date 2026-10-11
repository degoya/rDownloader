//! The component: one finished package, one Emby library refresh.
#![allow(unsafe_code)] // Generated canonical-ABI exports contain the only unsafe code here.

use plugin_guest_notifier::{
    Guest, Notification, delivered, destination_settings,
    http::{self, RequestHeader},
    refuse,
    types::{Failure, FailureKind},
};

use crate::request;

/// The one code every refusal of this plugin carries.
const REJECTED: &str = "emby_notifier.rejected";

struct Component;

impl Guest for Component {
    fn deliver(message: Notification) -> Result<(), Failure> {
        if !request::wanted(&message.event) {
            return Ok(());
        }
        // Refreshing a library is an administrator's request: without a key Emby answers
        // 401 every time, so this fails before anything is sent.
        if !message.has_secret {
            return Err(refuse(
                REJECTED,
                "This destination has no API key stored",
                FailureKind::AuthRequired,
            ));
        }
        let mapping = destination_settings::setting(request::MAPPING_SETTING).unwrap_or_default();
        let folder = request::folder(&mapping, message.package_path.as_deref());
        // Refused by the host before the plugin runs when the target is saved (RD-130-15);
        // checked here as well, so nothing is ever sent to a guessed address.
        let Some(endpoint) = request::endpoint(&message.destination, folder.is_some()) else {
            return Err(refuse(
                REJECTED,
                "This destination is not the address of an Emby server",
                FailureKind::Permanent,
            ));
        };
        let mut headers = vec![RequestHeader {
            name: "Authorization".to_owned(),
            value_template: request::AUTHORIZATION.to_owned(),
        }];
        let body = folder
            .as_deref()
            .map(request::updated_body)
            .unwrap_or_default();
        if !body.is_empty() {
            headers.push(RequestHeader {
                name: "Content-Type".to_owned(),
                value_template: "application/json".to_owned(),
            });
        }
        let response = http::http_request("POST", &endpoint, &[], &headers, body.as_bytes())?;
        // A 401 is a wrong key and a 403 one without administrator rights, neither of which time
        // fixes; a 5xx is a server that is starting or busy, and is tried again.
        delivered(response.status, "Emby", REJECTED)
    }
}

plugin_guest_notifier::notifier_plugin!(Component);
