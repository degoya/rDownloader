//! The component: one rDownloader notification, one Telegram `sendMessage` call.
#![allow(unsafe_code)] // Generated canonical-ABI exports contain the only unsafe code here.

wit_bindgen::generate!({
    path: "../../crates/rd-plugin-api/wit",
    world: "notifier-plugin",
});

use exports::rdownloader::plugin::notifier::{Guest, Notification};
use rdownloader::plugin::{
    http::{self, RequestQuery},
    types::{Failure, FailureKind},
};

use crate::payload;

struct Component;

fn refuse(message: &str, category: FailureKind) -> Failure {
    Failure {
        category,
        message: message.to_owned(),
        code: Some("telegram_notifier.rejected".to_owned()),
        params: Vec::new(),
    }
}

impl Guest for Component {
    fn deliver(message: Notification) -> Result<(), Failure> {
        if !message.has_secret {
            return Err(refuse(
                "This destination has no bot token stored",
                FailureKind::AuthRequired,
            ));
        }
        let Some(chat) = payload::chat_id(&message.destination) else {
            return Err(refuse(
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
        if (200..300).contains(&response.status) {
            return Ok(());
        }
        Err(refuse(
            &format!("Telegram answered {}", response.status),
            // 429 carries Telegram's own retry delay; the hub's backoff covers it. A 400 is
            // a wrong chat id and a 401 a wrong token, neither of which time fixes.
            if response.status >= 500 || response.status == 429 {
                FailureKind::Transient(None)
            } else {
                FailureKind::Permanent
            },
        ))
    }
}

export!(Component);
