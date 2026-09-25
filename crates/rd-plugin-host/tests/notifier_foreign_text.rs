//! Foreign text never becomes a secret on the wire (RD-120-65).
//!
//! A notification carries text somebody else wrote: a package name, a file name, the title of
//! a feed item. The host expands vault markers in every template of a plugin's request, and the
//! three bundled notifiers put that text into exactly those templates — Telegram's `text` query,
//! ntfy's `title` query and body, Discord's JSON body. So a release named `{{secret}}` was sent
//! with the destination's token in its place, into the chat, the topic or the channel.
//!
//! Each case here sends such text through the real host to the wire and reads what arrived: the
//! text with every `{` shown as `❴` (U+2774, `src/foreign_text.rs`), still delivered,
//! and the token nowhere but where the plugin itself put it.

#[path = "support/notifier_wire.rs"]
mod support;

use rd_plugin_host::extension::Delivery;
use support::{Arrived, host_over, notifier, wire};

const TELEGRAM: &str = include_str!("../../../plugins/telegram-notifier/manifest.toml");
const NTFY: &str = include_str!("../../../plugins/ntfy-notifier/manifest.toml");
const DISCORD: &str = include_str!("../../../plugins/discord-notifier/manifest.toml");

const TOKEN: &str = "123456:bot-token";

/// What foreign text looks like once the host has made it inert.
fn shown(text: &str) -> String {
    text.replace('{', "\u{2774}")
}

/// Everything a hostile name could try: the granted marker, a named one for the reference
/// the destination really uses, the Basic pair, the user name and the client id.
fn hostile(reference: &str) -> String {
    format!(
        "{{{{secret}}}} {{{{secret:{reference}}}}} {{{{basic:{reference}}}}} {{{{username}}}} \
         {{{{client_id}}}}"
    )
}

/// The token appears nowhere in what arrived except the one place the plugin put it.
fn token_only_in(request: &Arrived, allowed: &str) {
    let path = request.path();
    let elsewhere = format!(
        "{} {:?} {}",
        request.target.replacen(path, "", 1),
        request.headers,
        String::from_utf8_lossy(&request.body)
    );
    let elsewhere = match allowed {
        "path" => elsewhere,
        header => elsewhere.replace(&format!("(\"{header}\", \"Bearer {TOKEN}\")"), ""),
    };
    assert!(
        !elsewhere.contains(TOKEN) && !elsewhere.contains("123456%3Abot-token"),
        "the token left its place: {request:?}"
    );
}

fn only(arrived: &support::Wire) -> Arrived {
    let arrived = arrived.arrived.lock().expect("arrived").clone();
    assert_eq!(arrived.len(), 1, "{arrived:?}");
    arrived.into_iter().next().expect("one request")
}

#[tokio::test]
async fn telegram_sends_a_hostile_title_made_inert_and_keeps_the_token_in_its_path() {
    let wire = wire().await;
    let directory = tempfile::tempdir().expect("tempdir");
    let (host, reference) = host_over(directory.path(), &wire, TOKEN).await;
    // The case the owner's setup meets first: the granted marker alone, in a title.
    let text = "Release {{secret}}";
    notifier(TELEGRAM, "rd-plugin-telegram-notifier", host)
        .deliver(Delivery {
            title: text,
            body: "example.iso",
            event: "package_completed",
            severity: "info",
            idempotency_key: "foreign:telegram",
            destination: "-100123456",
            secret_ref: Some(&reference),
        })
        .await
        .expect("a hostile name is still delivered");

    let request = only(&wire);
    assert_eq!(request.path(), format!("/bot{TOKEN}/sendMessage"));
    let sent = request.query().remove("text").expect("text");
    assert!(sent.contains(&shown(text)), "{sent}");
    token_only_in(&request, "path");
}

#[tokio::test]
async fn ntfy_sends_a_hostile_title_and_body_made_inert() {
    let wire = wire().await;
    let directory = tempfile::tempdir().expect("tempdir");
    let (host, reference) = host_over(directory.path(), &wire, TOKEN).await;
    let title = "Release {{secret}}";
    let text = format!("{{{{secret:{reference}}}}} {{{{basic:{reference}}}}}");
    notifier(NTFY, "rd-plugin-ntfy-notifier", host)
        .deliver(Delivery {
            title,
            body: &text,
            event: &hostile(&reference),
            severity: "info",
            idempotency_key: "foreign:ntfy",
            destination: "downloads",
            secret_ref: Some(&reference),
        })
        .await
        .expect("a hostile name is still delivered");

    let request = only(&wire);
    let query = request.query();
    assert_eq!(query.get("title"), Some(&shown(title)));
    assert_eq!(query.get("tags"), Some(&shown(&hostile(&reference))));
    assert_eq!(request.body, shown(&text).as_bytes());
    // The one place the plugin put the token.
    assert_eq!(
        request.header("authorization"),
        Some(format!("Bearer {TOKEN}").as_str())
    );
    token_only_in(&request, "authorization");
}

#[tokio::test]
async fn discord_sends_a_hostile_title_made_inert_and_keeps_the_token_in_its_path() {
    let wire = wire().await;
    let directory = tempfile::tempdir().expect("tempdir");
    let (host, reference) = host_over(directory.path(), &wire, TOKEN).await;
    let text = hostile(&reference);
    notifier(DISCORD, "rd-plugin-discord-notifier", host)
        .deliver(Delivery {
            title: &text,
            body: &text,
            event: &text,
            severity: "info",
            idempotency_key: "foreign:discord",
            destination: "",
            secret_ref: Some(&reference),
        })
        .await
        .expect("a hostile name is still delivered");

    let request = only(&wire);
    assert_eq!(request.path(), format!("/api/webhooks/{TOKEN}"));
    let body: serde_json::Value = serde_json::from_slice(&request.body).expect("json body");
    let embed = &body["embeds"][0];
    // Discord's markdown escape turns `_` into `\_`; nothing else of the text changes.
    let shown = shown(&text).replace('_', "\\_");
    assert_eq!(embed["title"].as_str(), Some(shown.as_str()));
    assert_eq!(embed["description"].as_str(), Some(shown.as_str()));
    assert_eq!(embed["footer"]["text"].as_str(), Some(shown.as_str()));
    token_only_in(&request, "path");
}

/// The granted marker alone, which a destination without an account does expand: into ntfy's
/// title query and its raw body.
#[tokio::test]
async fn ntfy_sends_the_granted_marker_made_inert() {
    let wire = wire().await;
    let directory = tempfile::tempdir().expect("tempdir");
    let (host, reference) = host_over(directory.path(), &wire, TOKEN).await;
    let text = "Release {{secret}}";
    notifier(NTFY, "rd-plugin-ntfy-notifier", host)
        .deliver(Delivery {
            title: text,
            body: text,
            event: "package_completed",
            severity: "info",
            idempotency_key: "foreign:ntfy-granted",
            destination: "downloads",
            secret_ref: Some(&reference),
        })
        .await
        .expect("delivered");

    let request = only(&wire);
    assert_eq!(request.query().get("title"), Some(&shown(text)));
    assert_eq!(request.body, shown(text).as_bytes());
    token_only_in(&request, "authorization");
}

/// The granted marker alone, into Discord's JSON body.
#[tokio::test]
async fn discord_sends_the_granted_marker_made_inert() {
    let wire = wire().await;
    let directory = tempfile::tempdir().expect("tempdir");
    let (host, reference) = host_over(directory.path(), &wire, TOKEN).await;
    let text = "Release {{secret}}";
    notifier(DISCORD, "rd-plugin-discord-notifier", host)
        .deliver(Delivery {
            title: text,
            body: text,
            event: "package_completed",
            severity: "info",
            idempotency_key: "foreign:discord-granted",
            destination: "",
            secret_ref: Some(&reference),
        })
        .await
        .expect("delivered");

    let request = only(&wire);
    let body: serde_json::Value = serde_json::from_slice(&request.body).expect("json body");
    assert_eq!(
        body["embeds"][0]["title"].as_str(),
        Some(shown(text).as_str())
    );
    assert_eq!(
        body["embeds"][0]["description"].as_str(),
        Some(shown(text).as_str())
    );
    token_only_in(&request, "path");
}
