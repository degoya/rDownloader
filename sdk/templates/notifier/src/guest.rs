//! The component: one rDownloader notification, one request to the service.
#![allow(unsafe_code)] // Generated canonical-ABI exports contain the only unsafe code here.

wit_bindgen::generate!({
    path: "wit",
    world: "notifier-plugin",
});

use exports::rdownloader::plugin::notifier::{Guest, Notification};
use rdownloader::plugin::{
    http::{self, RequestHeader},
    types::{Failure, FailureKind},
};

use crate::Delivery;

struct Component;

impl Guest for Component {
    /// Delivers one message. Retrying is the host's decision, driven by the failure kind.
    fn deliver(message: Notification) -> Result<(), Failure> {
        let Some(url) = crate::endpoint(&message.destination) else {
            return Err(refuse(
                FailureKind::Permanent,
                "bad_channel",
                "the configured channel is not a plain name",
            ));
        };
        let mut headers = vec![RequestHeader {
            name: "Content-Type".to_owned(),
            value_template: "application/json".to_owned(),
        }];
        // A public channel needs no token, so the header is only added when one is stored:
        // `Bearer ` with nothing after it would fail for a reason nobody could read off the
        // message. `has-secret` says whether there is one — never what it is.
        if message.has_secret {
            headers.push(RequestHeader {
                name: "Authorization".to_owned(),
                value_template: "Bearer {{secret}}".to_owned(),
            });
        }
        let body = crate::body(
            &message.title,
            &message.body,
            &message.severity,
            &message.idempotency_key,
        );
        // A failure of the request itself — no connection — comes back through `?` as the
        // host's own transient failure, which the hub retries.
        let response = http::http_request("POST", &url, &[], &headers, body.as_bytes())?;
        match crate::delivery(response.status) {
            Delivery::Delivered => Ok(()),
            Delivery::Retry => Err(refuse(
                FailureKind::Transient(None),
                "unavailable",
                "the service did not take the message",
            )),
            Delivery::Unauthorized => Err(refuse(
                FailureKind::AuthRequired,
                "unauthorized",
                "the service refused the stored token",
            )),
            Delivery::Refused => Err(refuse(
                FailureKind::Permanent,
                "rejected",
                "the service rejected the message",
            )),
        }
    }
}

/// A failure carrying a stable translation code and nothing the service wrote.
fn refuse(category: FailureKind, code: &str, message: &str) -> Failure {
    Failure {
        category,
        message: message.to_owned(),
        code: Some(format!("{{PLUGIN_SLUG}}.{code}")),
        params: Vec::new(),
    }
}

export!(Component);
