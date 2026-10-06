//! The component: one rDownloader notification, one Telegram `sendMessage` call.
#![allow(unsafe_code)] // Generated canonical-ABI exports contain the only unsafe code here.

use plugin_guest_notifier::{
    Guest, Notification, delivered,
    http::{self, RequestQuery},
    refuse,
    types::{Failure, FailureKind},
};

use crate::payload;

/// The one code every refusal of this plugin carries.
const REJECTED: &str = "telegram_notifier.rejected";

struct Component;

impl Guest for Component {
    fn deliver(message: Notification) -> Result<(), Failure> {
        if !message.has_secret {
            return Err(refuse(
                REJECTED,
                "This destination has no bot token stored",
                FailureKind::AuthRequired,
            ));
        }
        let Some(chat) = payload::chat_id(&message.destination) else {
            return Err(refuse(
                REJECTED,
                "This destination has no usable chat id",
                FailureKind::Permanent,
            ));
        };
        let response = http::http_request(
            "POST",
            payload::ENDPOINT,
            &[
                RequestQuery {
                    name: "chat_id".to_owned(),
                    value_template: chat.to_owned(),
                },
                RequestQuery {
                    name: "parse_mode".to_owned(),
                    value_template: "HTML".to_owned(),
                },
                RequestQuery {
                    name: "text".to_owned(),
                    value_template: payload::text(&message.title, &message.body, &message.severity),
                },
            ],
            // No headers at all: the host states the length of the empty body itself, and it
            // refuses a `Content-Length` a plugin writes (RD-120-60).
            &[],
            &[],
        )?;
        // 429 carries Telegram's own retry delay; the hub's backoff covers it. A 400 is a wrong
        // chat id and a 401 a wrong token, neither of which time fixes.
        delivered(response.status, "Telegram", REJECTED)
    }
}

plugin_guest_notifier::notifier_plugin!(Component);
