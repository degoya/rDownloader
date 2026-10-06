//! The component: one rDownloader notification, one ntfy request.
#![allow(unsafe_code)] // Generated canonical-ABI exports contain the only unsafe code here.

use plugin_guest_notifier::{
    Guest, Notification, delivered, destination_settings,
    http::{self, RequestHeader, RequestQuery},
    types::Failure,
};

use crate::payload;

struct Component;

impl Guest for Component {
    fn deliver(message: Notification) -> Result<(), Failure> {
        // Title, priority and tags as ntfy's query parameters rather than its headers: ntfy
        // reads both spellings (`readParam`: header first, then the lowercase query name),
        // and the host sends no header outside its allowlist (RD-120-60).
        let query = [
            ("title", payload::field_value(&message.title)),
            // The target's own choice where it made one (RD-170-09); the host answers the
            // manifest's defaults otherwise.
            (
                "priority",
                payload::priority(&message.severity, destination_settings::setting),
            ),
            ("tags", payload::field_value(&message.event)),
        ]
        .into_iter()
        .map(|(name, value_template)| RequestQuery {
            name: name.to_owned(),
            value_template,
        })
        .collect::<Vec<_>>();
        let mut headers = Vec::new();
        // A public topic needs no token, so the header is only added when one is stored:
        // sending `Bearer ` with nothing after it would fail for a reason nobody could read
        // off the message. `has-secret` says whether there is one — never what it is.
        if message.has_secret {
            headers.push(RequestHeader {
                name: "Authorization".to_owned(),
                // The host substitutes the one secret this invocation was granted, or refuses
                // the request if it was granted none. The plugin never sees the value.
                value_template: "Bearer {{secret}}".to_owned(),
            });
        }
        let response = http::http_request(
            "POST",
            &payload::endpoint(&message.destination),
            &query,
            &headers,
            payload::body(&message.body).as_bytes(),
        )?;
        // 4xx will not become 5xx by trying again; 5xx and 429 might.
        delivered(response.status, "ntfy", "ntfy_notifier.rejected")
    }
}

plugin_guest_notifier::notifier_plugin!(Component);
