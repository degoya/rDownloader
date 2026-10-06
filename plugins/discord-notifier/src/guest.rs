//! The component: one rDownloader notification, one Discord webhook call.
#![allow(unsafe_code)] // Generated canonical-ABI exports contain the only unsafe code here.

use plugin_guest_notifier::{
    Guest, Notification, delivered,
    http::{self, RequestHeader},
    refuse,
    types::{Failure, FailureKind},
};

use crate::payload;

/// The one code every refusal of this plugin carries.
const REJECTED: &str = "discord_notifier.rejected";

struct Component;

impl Guest for Component {
    fn deliver(message: Notification) -> Result<(), Failure> {
        // A Discord webhook is its token: without one there is no address to post to, so
        // this fails immediately rather than sending somewhere incomplete.
        if !message.has_secret {
            return Err(refuse(
                REJECTED,
                "This destination has no webhook address stored",
                FailureKind::AuthRequired,
            ));
        }
        let response = http::http_request(
            "POST",
            payload::ENDPOINT,
            &[],
            &[RequestHeader {
                name: "Content-Type".to_owned(),
                value_template: "application/json".to_owned(),
            }],
            payload::body(
                &message.title,
                &message.body,
                &message.event,
                &message.severity,
            )
            .as_bytes(),
        )?;
        // 429 is Discord's rate limit and 5xx its own trouble; both pass. A 401 or 404 means
        // the webhook was deleted, and repeating that forever helps nobody.
        delivered(response.status, "Discord", REJECTED)
    }
}

plugin_guest_notifier::notifier_plugin!(Component);
