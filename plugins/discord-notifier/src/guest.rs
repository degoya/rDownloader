//! The component: one rDownloader notification, one Discord webhook call.
#![allow(unsafe_code)] // Generated canonical-ABI exports contain the only unsafe code here.

wit_bindgen::generate!({
    path: "../../crates/rd-plugin-api/wit",
    world: "notifier-plugin",
});

use exports::rdownloader::plugin::notifier::{Guest, Notification};
use rdownloader::plugin::{
    http::{self, RequestHeader},
    types::{Failure, FailureKind},
};

use crate::payload;

struct Component;

impl Guest for Component {
    fn deliver(message: Notification) -> Result<(), Failure> {
        // A Discord webhook is its token: without one there is no address to post to, so
        // this fails immediately rather than sending somewhere incomplete.
        if !message.has_secret {
            return Err(Failure {
                category: FailureKind::AuthRequired,
                message: "This destination has no webhook address stored".to_owned(),
                code: Some("discord_notifier.rejected".to_owned()),
                params: Vec::new(),
            });
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
        if (200..300).contains(&response.status) {
            return Ok(());
        }
        Err(Failure {
            // 429 is Discord's rate limit and 5xx its own trouble; both pass. A 401 or 404
            // means the webhook was deleted, and repeating that forever helps nobody.
            category: if response.status >= 500 || response.status == 429 {
                FailureKind::Transient(None)
            } else {
                FailureKind::Permanent
            },
            message: format!("Discord answered {}", response.status),
            code: Some("discord_notifier.rejected".to_owned()),
            params: Vec::new(),
        })
    }
}

export!(Component);
