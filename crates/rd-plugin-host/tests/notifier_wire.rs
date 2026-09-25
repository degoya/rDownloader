//! The Telegram and ntfy notifiers, through the real host path to the wire (RD-120-60), and
//! ntfy on a server of one's own (RD-130-15).
//!
//! `rd-plugin-ext`'s `notifier_contract` checks what a destination promises; it never lets one
//! send, so a header the host refuses was invisible there, and both destinations failed every
//! delivery from the day they shipped ("Resolver HTTP header is not allowed"). Here the
//! bundled component runs against the application's own host — `ResolverService::host`, the
//! one the notification hub hands it — with its secret expansion, header allowlist and HTTP
//! client, and the request is read where it lands.
//!
//! How the real service names are reached without leaving the machine: `support/notifier_wire.rs`.

#[path = "support/notifier_wire.rs"]
mod support;

use rd_plugin_host::extension::Delivery;
use support::{REDIRECT_TO, host_over, notifier, wire};

const TELEGRAM: &str = include_str!("../../../plugins/telegram-notifier/manifest.toml");
const NTFY: &str = include_str!("../../../plugins/ntfy-notifier/manifest.toml");

#[tokio::test]
async fn telegram_reaches_send_message_with_its_query_and_no_header_of_its_own() {
    let wire = wire().await;
    let directory = tempfile::tempdir().expect("tempdir");
    let (host, reference) = host_over(directory.path(), &wire, "123456:bot-token").await;
    let plugin = notifier(TELEGRAM, "rd-plugin-telegram-notifier", host);

    plugin
        .deliver(Delivery {
            title: "Package finished",
            body: "example.iso",
            event: "package_completed",
            severity: "info",
            idempotency_key: "wire:telegram",
            destination: "-100123456",
            secret_ref: Some(&reference),
        })
        .await
        .expect("the delivery reaches Telegram");

    let arrived = wire.arrived.lock().expect("arrived").clone();
    assert_eq!(arrived.len(), 1, "{arrived:?}");
    let request = &arrived[0];
    assert_eq!(request.tunnel, "api.telegram.org:443");
    assert_eq!(request.method, "POST");
    // The token reaches the path from the vault; the plugin wrote only the marker.
    assert_eq!(request.path(), "/bot123456:bot-token/sendMessage");
    let query = request.query();
    assert_eq!(query.get("chat_id").map(String::as_str), Some("-100123456"));
    assert_eq!(query.get("parse_mode").map(String::as_str), Some("HTML"));
    let text = query.get("text").expect("text");
    assert!(text.contains("Package finished"), "{text}");
    assert!(text.contains("example.iso"), "{text}");
    // The empty body's length is the host's to state, and it states it exactly once: hyper
    // alone would send a bodiless POST with no `Content-Length` at all.
    assert!(request.body.is_empty());
    assert_eq!(request.headers_named("content-length"), 1, "{request:?}");
    assert_eq!(request.header("content-length"), Some("0"), "{request:?}");
}

#[tokio::test]
async fn ntfy_carries_title_priority_and_tags_in_its_query() {
    let wire = wire().await;
    let directory = tempfile::tempdir().expect("tempdir");
    let (host, reference) = host_over(directory.path(), &wire, "tk_access").await;
    let plugin = notifier(NTFY, "rd-plugin-ntfy-notifier", host);

    plugin
        .deliver(Delivery {
            title: "Caf\u{e9} finished",
            body: "example.iso is complete",
            event: "package_completed",
            severity: "error",
            idempotency_key: "wire:ntfy",
            destination: "downloads",
            secret_ref: Some(&reference),
        })
        .await
        .expect("the delivery reaches ntfy");

    let arrived = wire.arrived.lock().expect("arrived").clone();
    assert_eq!(arrived.len(), 1, "{arrived:?}");
    let request = &arrived[0];
    assert_eq!(request.tunnel, "ntfy.sh:443");
    assert_eq!(request.method, "POST");
    assert_eq!(request.path(), "/downloads");
    let query = request.query();
    // Percent-encoded on the way, so the accent survives the trip ntfy's header would not.
    assert_eq!(
        query.get("title").map(String::as_str),
        Some("Caf\u{e9} finished")
    );
    assert_eq!(query.get("priority").map(String::as_str), Some("4"));
    assert_eq!(
        query.get("tags").map(String::as_str),
        Some("package_completed")
    );
    assert_eq!(request.body, b"example.iso is complete");
    assert_eq!(request.header("authorization"), Some("Bearer tk_access"));
    for header in ["title", "priority", "tags"] {
        assert!(request.header(header).is_none(), "{header}: {request:?}");
    }
}

/// RD-130-15: a destination written as an address reaches that server, with the token, and the
/// token goes nowhere else.
#[tokio::test]
async fn ntfy_reaches_a_self_hosted_server_with_the_token() {
    let wire = wire().await;
    let directory = tempfile::tempdir().expect("tempdir");
    let (host, reference) = host_over(directory.path(), &wire, "tk_access").await;
    let plugin = notifier(NTFY, "rd-plugin-ntfy-notifier", host);

    plugin
        .deliver(Delivery {
            title: "Package finished",
            body: "example.iso is complete",
            event: "package_completed",
            severity: "info",
            idempotency_key: "wire:ntfy-self-hosted",
            destination: "https://ntfy.example.org/alerts",
            secret_ref: Some(&reference),
        })
        .await
        .expect("the delivery reaches the self-hosted server");

    let arrived = wire.arrived.lock().expect("arrived").clone();
    assert_eq!(arrived.len(), 1, "{arrived:?}");
    let request = &arrived[0];
    assert_eq!(request.tunnel, "ntfy.example.org:443");
    assert_eq!(request.method, "POST");
    assert_eq!(request.path(), "/alerts");
    assert_eq!(request.header("authorization"), Some("Bearer tk_access"));
}

/// The self-hosted server answers with a redirect to `ntfy.sh`, and nothing arrives there — not
/// the token, not the body, not a connection (RD-130-24).
///
/// Until RD-130-24 the plugin HTTP path let the client follow the redirect and refused it only
/// once it had ended outside the plugin's domains: the `307` carried the message to `ntfy.sh`
/// first. Every hop is now put to the invocation's list before it is followed, and a refused one
/// comes back as `plugin.redirect_outside_domains`, the code a redirect that ended outside
/// always had.
#[tokio::test]
async fn ntfy_on_a_self_hosted_server_is_not_followed_to_ntfy_sh() {
    let wire = wire().await;
    let directory = tempfile::tempdir().expect("tempdir");
    let (host, reference) = host_over(directory.path(), &wire, "tk_access").await;
    let plugin = notifier(NTFY, "rd-plugin-ntfy-notifier", host);
    let destination = format!("https://ntfy.example.org{REDIRECT_TO}ntfy.sh/stolen");

    let error = plugin
        .deliver(Delivery {
            title: "Package finished",
            body: "example.iso is complete",
            event: "package_completed",
            severity: "info",
            idempotency_key: "wire:ntfy-redirect",
            destination: &destination,
            secret_ref: Some(&reference),
        })
        .await
        .expect_err("a redirect off the configured server is not followed");

    let failure = error
        .downcast_ref::<rd_core::Failure>()
        .unwrap_or_else(|| panic!("the refusal carries its code: {error}"));
    assert_eq!(
        failure.code.as_deref(),
        Some("plugin.redirect_outside_domains")
    );
    let arrived = wire.arrived.lock().expect("arrived").clone();
    assert_eq!(arrived.len(), 1, "{arrived:?}");
    assert_eq!(arrived[0].tunnel, "ntfy.example.org:443");
}

/// A redirect between two hosts the plugin may reach is still followed (RD-130-24): the gate
/// narrows, it does not switch redirects off. The manifest here names both hosts and no `*`,
/// so the delivery's list is exactly those two.
///
/// Without a token: a request that carries one is additionally held to the provider registry
/// on a change of host (`validate_redirect`), which no notification service is in.
#[tokio::test]
async fn a_redirect_between_two_reachable_hosts_is_followed() {
    let wire = wire().await;
    let directory = tempfile::tempdir().expect("tempdir");
    let (host, _) = host_over(directory.path(), &wire, "tk_access").await;
    let two_hosts = NTFY.replace(
        r#"domains = ["ntfy.sh", "*"]"#,
        r#"domains = ["ntfy.example.org", "push.example.org"]"#,
    );
    assert_ne!(two_hosts, NTFY, "the manifest's domain line moved");
    let plugin = notifier(&two_hosts, "rd-plugin-ntfy-notifier", host);
    let destination = format!("https://ntfy.example.org{REDIRECT_TO}push.example.org/alerts");

    plugin
        .deliver(Delivery {
            title: "Package finished",
            body: "example.iso is complete",
            event: "package_completed",
            severity: "info",
            idempotency_key: "wire:ntfy-redirect-within",
            destination: &destination,
            secret_ref: None,
        })
        .await
        .expect("a redirect inside the plugin's domains is followed");

    let arrived = wire.arrived.lock().expect("arrived").clone();
    assert_eq!(arrived.len(), 2, "{arrived:?}");
    assert_eq!(arrived[0].tunnel, "ntfy.example.org:443");
    let followed = &arrived[1];
    assert_eq!(followed.tunnel, "push.example.org:443");
    assert_eq!(followed.method, "POST");
    assert_eq!(followed.path(), "/alerts");
    assert_eq!(followed.body, b"example.iso is complete");
}

/// `http://` only inside the own network (owner's decision, 2026-09-25): to a public server
/// the token would travel readable, so the host refuses before the plugin runs, and nothing
/// is sent at all.
#[tokio::test]
async fn ntfy_refuses_plain_http_to_a_public_server_before_sending_anything() {
    let wire = wire().await;
    let directory = tempfile::tempdir().expect("tempdir");
    let (host, reference) = host_over(directory.path(), &wire, "tk_access").await;
    let plugin = notifier(NTFY, "rd-plugin-ntfy-notifier", host);

    let error = plugin
        .deliver(Delivery {
            title: "Package finished",
            body: "example.iso is complete",
            event: "package_completed",
            severity: "info",
            idempotency_key: "wire:ntfy-plain-http",
            destination: "http://ntfy.example.org/alerts",
            secret_ref: Some(&reference),
        })
        .await
        .expect_err("plain http to a public server is refused");

    let failure = error
        .downcast_ref::<rd_core::Failure>()
        .unwrap_or_else(|| panic!("the refusal carries its code: {error}"));
    assert_eq!(
        failure.code.as_deref(),
        Some("plugin.destination_not_encrypted")
    );
    assert!(!failure.category.is_retryable());
    let arrived = wire.arrived.lock().expect("arrived").clone();
    assert!(arrived.is_empty(), "{arrived:?}");
}
