//! The media library refresh of Plex, Jellyfin and Emby (RD-1240-12), through the real host path
//! to the wire, against the answers those servers give.
//!
//! Each is a notification destination whose manifest names no host: the server is the person's
//! own, given as the target's address, and the token is in the vault. What is checked is the one
//! request a finished package becomes, that nothing else becomes one, and that the token reaches
//! the server and nothing that is written down -- not the failure, not its message.
//!
//! The wire, its proxy and its certificate: `support/notifier_wire.rs`.

#[path = "support/notifier_wire.rs"]
mod support;

use rd_plugin_host::extension::{Delivery, NotifierPlugin};
use support::{Recorded, Wire, host_over, notifier, wire, wire_playing};

const PLEX: &str = include_str!("../../../plugins/plex-notifier/manifest.toml");
const JELLYFIN: &str = include_str!("../../../plugins/jellyfin-notifier/manifest.toml");
const EMBY: &str = include_str!("../../../plugins/emby-notifier/manifest.toml");

/// The token in the vault. Long and unlike anything else in a request, so finding it in a text
/// can only mean it leaked there.
const TOKEN: &str = "tok3n-xYz-never-in-a-log";

/// The servers' answers, modelled on what they send: Plex starts the scan and answers `200` with
/// no body, Jellyfin and Emby queue it and answer `204`, and a wrong token is a `401` -- Plex's
/// with its small HTML page, Jellyfin's empty, Emby's with a line of text.
const PLEX_STARTED: Recorded = Recorded {
    status: 200,
    reason: "OK",
    content_type: "",
    body: "",
};
const PLEX_UNAUTHORIZED: Recorded = Recorded {
    status: 401,
    reason: "Unauthorized",
    content_type: "text/html",
    body: "<html><head><title>Unauthorized</title></head><body><h1>401 Unauthorized</h1></body></html>",
};
const QUEUED: Recorded = Recorded {
    status: 204,
    reason: "No Content",
    content_type: "",
    body: "",
};
const JELLYFIN_UNAUTHORIZED: Recorded = Recorded {
    status: 401,
    reason: "Unauthorized",
    content_type: "",
    body: "",
};
const EMBY_UNAUTHORIZED: Recorded = Recorded {
    status: 401,
    reason: "Unauthorized",
    content_type: "text/plain",
    body: "Access token is invalid or expired.",
};
const STARTING: Recorded = Recorded {
    status: 503,
    reason: "Service Unavailable",
    content_type: "text/plain",
    body: "Server is starting",
};

fn finished<'a>(destination: &'a str, secret_ref: Option<&'a str>) -> Delivery<'a> {
    Delivery {
        title: "Package finished: Holiday",
        body: "Holiday (completed)",
        event: "package_completed",
        severity: "info",
        idempotency_key: "wire:library",
        destination,
        secret_ref,
        settings: &[],
    }
}

/// The application's host over `wire`, the plugin of `source`, and the vault reference of
/// [`TOKEN`].
async fn plugin(
    directory: &std::path::Path,
    wire: &Wire,
    source: &str,
    package: &str,
) -> (NotifierPlugin, String) {
    let (host, reference) = host_over(directory, wire, TOKEN).await;
    (notifier(source, package, host), reference)
}

/// The failure a refused delivery carries, after checking the token is nowhere in it.
fn refusal(error: &anyhow::Error) -> &rd_core::Failure {
    let written = format!("{error:#} {error:?}");
    assert!(!written.contains(TOKEN), "the token leaked: {written}");
    let failure = error
        .downcast_ref::<rd_core::Failure>()
        .unwrap_or_else(|| panic!("the refusal carries its code: {error}"));
    let params = format!("{:?}", failure.params);
    assert!(!params.contains(TOKEN), "the token leaked: {params}");
    failure
}

#[tokio::test]
async fn plex_refreshes_every_section_with_its_token_in_the_query() {
    let wire = wire_playing(vec![("/library/sections/all/refresh", PLEX_STARTED)]).await;
    let directory = tempfile::tempdir().expect("tempdir");
    let (plex, reference) = plugin(directory.path(), &wire, PLEX, "rd-plugin-plex-notifier").await;

    plex.deliver(finished(
        "https://plex.example.org:32400/",
        Some(&reference),
    ))
    .await
    .expect("Plex starts the scan");

    let arrived = wire.arrived.lock().expect("arrived").clone();
    assert_eq!(arrived.len(), 1, "{arrived:?}");
    let request = &arrived[0];
    assert_eq!(request.tunnel, "plex.example.org:32400");
    assert_eq!(request.method, "GET");
    assert_eq!(request.path(), "/library/sections/all/refresh");
    assert_eq!(
        request.query().get("X-Plex-Token").map(String::as_str),
        Some(TOKEN)
    );
    assert_eq!(request.headers_named("authorization"), 0, "{request:?}");
    assert!(request.body.is_empty());
}

/// A Plex server that admits its own network without sign-in: no token stored, none sent.
#[tokio::test]
async fn plex_without_a_token_sends_none() {
    let wire = wire().await;
    let directory = tempfile::tempdir().expect("tempdir");
    let (plex, _) = plugin(directory.path(), &wire, PLEX, "rd-plugin-plex-notifier").await;

    plex.deliver(finished("https://plex.example.org/", None))
        .await
        .expect("Plex starts the scan");

    let arrived = wire.arrived.lock().expect("arrived").clone();
    assert_eq!(arrived.len(), 1, "{arrived:?}");
    assert!(arrived[0].query().is_empty(), "{arrived:?}");
}

/// A wrong token is a refusal nobody retries, and the token -- which travelled in the address --
/// is in no part of what is recorded about it.
#[tokio::test]
async fn plex_refusing_the_token_is_permanent_and_never_shows_it() {
    let wire = wire_playing(vec![(
        "/denied/library/sections/all/refresh",
        PLEX_UNAUTHORIZED,
    )])
    .await;
    let directory = tempfile::tempdir().expect("tempdir");
    let (plex, reference) = plugin(directory.path(), &wire, PLEX, "rd-plugin-plex-notifier").await;

    let error = plex
        .deliver(finished(
            "https://plex.example.org/denied",
            Some(&reference),
        ))
        .await
        .expect_err("Plex refuses the token");

    let failure = refusal(&error);
    assert_eq!(failure.code.as_deref(), Some("plex_notifier.rejected"));
    assert_eq!(failure.message, "Plex answered 401");
    assert!(!failure.category.is_retryable());
}

#[tokio::test]
async fn jellyfin_and_emby_refresh_with_the_key_in_the_authorization_header() {
    // Jellyfin behind a reverse proxy with a base path of its own, Emby at the server's root.
    for (source, package, destination, tunnel, path) in [
        (
            JELLYFIN,
            "rd-plugin-jellyfin-notifier",
            "https://jellyfin.example.org/jellyfin/",
            "jellyfin.example.org:443",
            "/jellyfin/Library/Refresh",
        ),
        (
            EMBY,
            "rd-plugin-emby-notifier",
            "https://emby.example.org:8920",
            "emby.example.org:8920",
            "/Library/Refresh",
        ),
    ] {
        let wire = wire_playing(vec![(path, QUEUED)]).await;
        let directory = tempfile::tempdir().expect("tempdir");
        let (server, reference) = plugin(directory.path(), &wire, source, package).await;

        server
            .deliver(finished(destination, Some(&reference)))
            .await
            .unwrap_or_else(|error| panic!("{package} queues the scan: {error}"));

        let arrived = wire.arrived.lock().expect("arrived").clone();
        assert_eq!(arrived.len(), 1, "{arrived:?}");
        let request = &arrived[0];
        assert_eq!(request.tunnel, tunnel);
        assert_eq!(request.method, "POST");
        assert_eq!(request.path(), path);
        assert!(request.query().is_empty(), "{request:?}");
        assert_eq!(
            request.header("authorization"),
            Some(format!("MediaBrowser Token=\"{TOKEN}\"").as_str()),
            "{package}"
        );
        assert!(request.body.is_empty());
    }
}

#[tokio::test]
async fn jellyfin_and_emby_refusing_the_key_is_permanent_and_never_shows_it() {
    for (source, package, host, answer, code, message) in [
        (
            JELLYFIN,
            "rd-plugin-jellyfin-notifier",
            "jellyfin.example.org",
            JELLYFIN_UNAUTHORIZED,
            "jellyfin_notifier.rejected",
            "Jellyfin answered 401",
        ),
        (
            EMBY,
            "rd-plugin-emby-notifier",
            "emby.example.org",
            EMBY_UNAUTHORIZED,
            "emby_notifier.rejected",
            "Emby answered 401",
        ),
    ] {
        let wire = wire_playing(vec![("/Library/Refresh", answer)]).await;
        let directory = tempfile::tempdir().expect("tempdir");
        let (server, reference) = plugin(directory.path(), &wire, source, package).await;

        let error = server
            .deliver(finished(&format!("https://{host}"), Some(&reference)))
            .await
            .expect_err("the key is refused");

        let failure = refusal(&error);
        assert_eq!(failure.code.as_deref(), Some(code));
        assert_eq!(failure.message, message);
        assert!(!failure.category.is_retryable());
    }
}

/// A server that is starting answers `503`; that is worth another try.
#[tokio::test]
async fn a_server_that_is_starting_is_tried_again() {
    let wire = wire_playing(vec![("/Library/Refresh", STARTING)]).await;
    let directory = tempfile::tempdir().expect("tempdir");
    let (jellyfin, reference) = plugin(
        directory.path(),
        &wire,
        JELLYFIN,
        "rd-plugin-jellyfin-notifier",
    )
    .await;

    let error = jellyfin
        .deliver(finished("https://jellyfin.example.org", Some(&reference)))
        .await
        .expect_err("the server is starting");

    assert!(refusal(&error).category.is_retryable());
}

/// Refreshing a library is an administrator's request: without a key nothing is sent at all.
#[tokio::test]
async fn jellyfin_and_emby_without_a_key_send_nothing() {
    for (source, package, host) in [
        (
            JELLYFIN,
            "rd-plugin-jellyfin-notifier",
            "jellyfin.example.org",
        ),
        (EMBY, "rd-plugin-emby-notifier", "emby.example.org"),
    ] {
        let wire = wire().await;
        let directory = tempfile::tempdir().expect("tempdir");
        let (server, _) = plugin(directory.path(), &wire, source, package).await;

        let error = server
            .deliver(finished(&format!("https://{host}"), None))
            .await
            .expect_err("no key is stored");

        assert_eq!(refusal(&error).category, rd_core::FailureKind::AuthRequired);
        assert!(
            wire.arrived.lock().expect("arrived").is_empty(),
            "{package}"
        );
    }
}

/// Only a finished package refreshes the library. A rule that sends the target other events too
/// is answered as delivered, and nothing reaches the server.
#[tokio::test]
async fn no_other_event_reaches_the_server() {
    for (source, package, host) in [
        (PLEX, "rd-plugin-plex-notifier", "plex.example.org"),
        (
            JELLYFIN,
            "rd-plugin-jellyfin-notifier",
            "jellyfin.example.org",
        ),
        (EMBY, "rd-plugin-emby-notifier", "emby.example.org"),
    ] {
        let wire = wire().await;
        let directory = tempfile::tempdir().expect("tempdir");
        let (server, reference) = plugin(directory.path(), &wire, source, package).await;
        let destination = format!("https://{host}");

        for event in ["package_failed", "storage_blocked", "update_available"] {
            server
                .deliver(Delivery {
                    event,
                    severity: "error",
                    ..finished(&destination, Some(&reference))
                })
                .await
                .unwrap_or_else(|error| panic!("{package} {event}: {error}"));
        }

        assert!(
            wire.arrived.lock().expect("arrived").is_empty(),
            "{package}"
        );
    }
}

/// The manifests name no service, so a destination that is not the server's address has nowhere
/// to go: refused when the target is saved, with the code the delivery would fail with.
#[tokio::test]
async fn a_destination_that_is_not_an_address_is_refused_when_saved() {
    for (source, package) in [
        (PLEX, "rd-plugin-plex-notifier"),
        (JELLYFIN, "rd-plugin-jellyfin-notifier"),
        (EMBY, "rd-plugin-emby-notifier"),
    ] {
        let wire = wire().await;
        let directory = tempfile::tempdir().expect("tempdir");
        let (server, reference) = plugin(directory.path(), &wire, source, package).await;

        let saved = server
            .check_destination("media-server")
            .expect_err("not an address");
        assert_eq!(saved.code.as_deref(), Some("plugin.destination_invalid"));
        let error = server
            .deliver(finished("media-server", Some(&reference)))
            .await
            .expect_err("refused at delivery too");
        assert_eq!(refusal(&error).code, saved.code, "{package}");
        server
            .check_destination("http://192.168.1.10:8096")
            .unwrap_or_else(|failure| panic!("{package}: {failure}"));
        assert!(
            wire.arrived.lock().expect("arrived").is_empty(),
            "{package}"
        );
    }
}
