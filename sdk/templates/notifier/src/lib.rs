//! A scaffold notification destination. It compiles, packages and passes conformance as it is.
//!
//! Two things the host guarantees, which shape how this is written:
//!
//! - Retrying is not your decision. Report a failure and say what kind it is; the delivery
//!   hub applies its own backoff and quiet hours, the same ones the built-in webhook uses.
//! - A secret reaches your request without reaching you: put `{{secret:<reference>}}` in a
//!   header or query value and the host substitutes it on the way out.
#![allow(unsafe_code)] // Generated canonical-ABI exports contain the only unsafe code here.

wit_bindgen::generate!({
    path: "wit",
    world: "notifier-plugin",
});

use exports::rdownloader::plugin::notifier::{Guest, Notification};
use rdownloader::plugin::types::Failure;

struct Component;

impl Guest for Component {
    /// Delivers one message.
    ///
    /// `idempotency-key` is stable for one event, so a destination that can deduplicate
    /// should send it along: a delivery that failed while answering is tried again, and
    /// at-least-once is what the hub promises rather than exactly-once.
    fn deliver(message: Notification) -> Result<(), Failure> {
        rdownloader::plugin::host::log(
            "info",
            &format!("would deliver {}: {}", message.event, message.title),
        );
        Ok(())
    }
}

export!(Component);
