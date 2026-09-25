//! The component: one rDownloader notification, one ntfy request.
#![allow(unsafe_code)] // Generated canonical-ABI exports contain the only unsafe code here.

wit_bindgen::generate!({
    path: "../../crates/rd-plugin-api/wit",
    world: "notifier-plugin",
});

use exports::rdownloader::plugin::notifier::{Guest, Notification};
use rdownloader::plugin::{
    http::{self, RequestHeader, RequestQuery},
    types::{Failure, FailureKind},
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
            ("priority", payload::priority(&message.severity).to_owned()),
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
        if (200..300).contains(&response.status) {
            return Ok(());
        }
        Err(Failure {
            // 4xx will not become 5xx by trying again; 5xx and 429 might.
            category: if response.status >= 500 || response.status == 429 {
                FailureKind::Transient(None)
            } else {
                FailureKind::Permanent
            },
            message: format!("ntfy answered {}", response.status),
            code: Some("ntfy_notifier.rejected".to_owned()),
            params: Vec::new(),
        })
    }
}

export!(Component);
