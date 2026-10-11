//! The path-precise library refresh of Plex, Jellyfin and Emby (RD-1250-01), through the real
//! host path to the wire, against answers recorded from real servers.
//!
//! A finished package's notification names its folder; the target maps it onto the server's
//! own path. Plex scans that folder in the section the target names
//! (`/library/sections/<id>/refresh?path=`), Jellyfin and Emby are told it was created
//! (`POST /Library/Media/Updated`, answered `204`). A folder the mapping does not hold, or none
//! at all, refreshes the whole library as before; the token reaches the server and nothing
//! written down.
//!
//! The answers below were recorded on 2026-10-11 from Jellyfin 10.11.0
//! (`jellyfin/jellyfin:10.11.0`), Emby 4.9.1.80 (`emby/embyserver:4.9.1.80`) and Plex Media
//! Server 1.41.6.9685 (`plexinc/pms-docker:1.41.6.9685-d301f511a`), each a fresh container with
//! one movie library. Jellyfin and Emby took `Authorization: MediaBrowser Token="<key>"` on both
//! routes and found a folder reported through `/Library/Media/Updated` with their realtime
//! monitor switched off (both rescan the folder above the reported one); Plex found the folder
//! named by `path` in section 1 and nothing else, while `sections/all/refresh?path=` scanned
//! everything -- which is why a path needs a section.
//!
//! The wire, its proxy and its certificate: `support/notifier_wire.rs`.

#[path = "support/notifier_wire.rs"]
mod support;

use rd_plugin_host::extension::{Delivery, NotifierPlugin};
use support::{Recorded, Wire, host_over, notifier, wire_playing};

const PLEX: &str = include_str!("../../../plugins/plex-notifier/manifest.toml");
const JELLYFIN: &str = include_str!("../../../plugins/jellyfin-notifier/manifest.toml");
const EMBY: &str = include_str!("../../../plugins/emby-notifier/manifest.toml");

/// The token in the vault, unlike anything else in a request.
const TOKEN: &str = "tok3n-pAth-never-in-a-log";

/// The service's folder of the finished package, and the mapping onto the server's.
const FOLDER: &str = "/srv/downloads/Holiday 2026";
const MAPPING: &str = "/srv/downloads=/data/media\n/srv/downloads/series=/data/tv";
const MAPPED: &str = "/data/media/Holiday 2026";

/// Plex starts the scan and answers `200`, `text/html` without a body.
const PLEX_STARTED: Recorded = Recorded {
    status: 200,
    reason: "OK",
    content_type: "text/html",
    body: "",
};
/// Plex for a section it does not have.
const PLEX_NO_SECTION: Recorded = Recorded {
    status: 404,
    reason: "Not Found",
    content_type: "text/html",
    body: "<html><head><title>Not Found</title></head><body><h1>404 Not Found</h1></body></html>",
};
/// Jellyfin and Emby answer the update and the refresh with `204` and no body.
const NO_CONTENT: Recorded = Recorded {
    status: 204,
    reason: "No Content",
    content_type: "",
    body: "",
};
/// A key Jellyfin does not know: `401` with `Content-Length: 0`.
const JELLYFIN_UNAUTHORIZED: Recorded = Recorded {
    status: 401,
    reason: "Unauthorized",
    content_type: "",
    body: "",
};
/// A key Emby does not know: `401` and one line of text.
const EMBY_UNAUTHORIZED: Recorded = Recorded {
    status: 401,
    reason: "Unauthorized",
    content_type: "text/plain",
    body: "Access token is invalid or expired.",
};

fn settings(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
        .collect()
}

fn finished<'a>(
    destination: &'a str,
    secret_ref: &'a str,
    settings: &'a [(String, String)],
    package_path: Option<&'a str>,
) -> Delivery<'a> {
    Delivery {
        title: "Package finished: Holiday 2026",
        body: "Holiday 2026 (completed)",
        event: "package_completed",
        severity: "info",
        idempotency_key: "wire:paths",
        destination,
        secret_ref: Some(secret_ref),
        settings,
        package_path,
    }
}

async fn plugin(
    directory: &std::path::Path,
    wire: &Wire,
    source: &str,
    package: &str,
) -> (NotifierPlugin, String) {
    let (host, reference) = host_over(directory, wire, TOKEN).await;
    (notifier(source, package, host), reference)
}

fn arrived(wire: &Wire) -> Vec<support::Arrived> {
    wire.arrived.lock().expect("arrived").clone()
}

#[tokio::test]
async fn plex_scans_the_mapped_folder_in_the_named_section() {
    let wire = wire_playing(vec![("/library/sections/3/refresh", PLEX_STARTED)]).await;
    let directory = tempfile::tempdir().expect("tempdir");
    let (plex, reference) = plugin(directory.path(), &wire, PLEX, "rd-plugin-plex-notifier").await;
    let chosen = settings(&[("section", "3"), ("path_mapping", MAPPING)]);
    plex.check_settings(&chosen).expect("free text is saved");

    plex.deliver(finished(
        "https://plex.example.org:32400",
        &reference,
        &chosen,
        Some(FOLDER),
    ))
    .await
    .expect("Plex starts the scan");

    let arrived = arrived(&wire);
    assert_eq!(arrived.len(), 1, "{arrived:?}");
    let request = &arrived[0];
    assert_eq!(request.method, "GET");
    assert_eq!(request.path(), "/library/sections/3/refresh");
    let query = request.query();
    assert_eq!(query.get("path").map(String::as_str), Some(MAPPED));
    assert_eq!(query.get("X-Plex-Token").map(String::as_str), Some(TOKEN));
}

/// Without a section Plex cannot scan one folder; with a section but a folder the mapping does
/// not hold, that section is scanned whole; without a folder told, the same.
#[tokio::test]
async fn plex_without_a_section_or_a_mapped_folder_refreshes_as_before() {
    for (section, folder, path) in [
        ("", Some(FOLDER), "/library/sections/all/refresh"),
        ("3", Some("/home/me/Holiday"), "/library/sections/3/refresh"),
        ("3", None, "/library/sections/3/refresh"),
    ] {
        let wire = wire_playing(vec![(path, PLEX_STARTED)]).await;
        let directory = tempfile::tempdir().expect("tempdir");
        let (plex, reference) =
            plugin(directory.path(), &wire, PLEX, "rd-plugin-plex-notifier").await;
        let chosen = settings(&[("section", section), ("path_mapping", MAPPING)]);

        plex.deliver(finished(
            "https://plex.example.org",
            &reference,
            &chosen,
            folder,
        ))
        .await
        .unwrap_or_else(|error| panic!("{section:?} {folder:?}: {error}"));

        let arrived = arrived(&wire);
        assert_eq!(arrived.len(), 1, "{arrived:?}");
        assert_eq!(arrived[0].path(), path);
        assert!(!arrived[0].query().contains_key("path"), "{arrived:?}");
    }
}

/// A section is a number: anything else is refused before a request, under the plugin's code.
#[tokio::test]
async fn plex_refuses_a_section_that_is_no_number() {
    let wire = wire_playing(Vec::new()).await;
    let directory = tempfile::tempdir().expect("tempdir");
    let (plex, reference) = plugin(directory.path(), &wire, PLEX, "rd-plugin-plex-notifier").await;
    let chosen = settings(&[("section", "Movies")]);

    let error = plex
        .deliver(finished(
            "https://plex.example.org",
            &reference,
            &chosen,
            Some(FOLDER),
        ))
        .await
        .expect_err("no section of that name");

    let failure = error.downcast_ref::<rd_core::Failure>().expect("coded");
    assert_eq!(failure.code.as_deref(), Some("plex_notifier.rejected"));
    assert!(!failure.category.is_retryable());
    assert!(arrived(&wire).is_empty());
}

/// A section the server does not have is a `404` nobody retries.
#[tokio::test]
async fn plex_without_that_section_is_permanent() {
    let wire = wire_playing(vec![("/library/sections/99/refresh", PLEX_NO_SECTION)]).await;
    let directory = tempfile::tempdir().expect("tempdir");
    let (plex, reference) = plugin(directory.path(), &wire, PLEX, "rd-plugin-plex-notifier").await;
    let chosen = settings(&[("section", "99"), ("path_mapping", MAPPING)]);

    let error = plex
        .deliver(finished(
            "https://plex.example.org",
            &reference,
            &chosen,
            Some(FOLDER),
        ))
        .await
        .expect_err("no such section");

    let written = format!("{error:#} {error:?}");
    assert!(!written.contains(TOKEN), "the token leaked: {written}");
    let failure = error.downcast_ref::<rd_core::Failure>().expect("coded");
    assert_eq!(failure.message, "Plex answered 404");
    assert!(!failure.category.is_retryable());
}

#[tokio::test]
async fn jellyfin_and_emby_are_told_the_mapped_folder_was_created() {
    for (source, package, host) in [
        (
            JELLYFIN,
            "rd-plugin-jellyfin-notifier",
            "jellyfin.example.org",
        ),
        (EMBY, "rd-plugin-emby-notifier", "emby.example.org"),
    ] {
        let wire = wire_playing(vec![("/Library/Media/Updated", NO_CONTENT)]).await;
        let directory = tempfile::tempdir().expect("tempdir");
        let (server, reference) = plugin(directory.path(), &wire, source, package).await;
        let chosen = settings(&[("path_mapping", MAPPING)]);

        server
            .deliver(finished(
                &format!("https://{host}"),
                &reference,
                &chosen,
                Some(FOLDER),
            ))
            .await
            .unwrap_or_else(|error| panic!("{package} takes the update: {error}"));

        let arrived = arrived(&wire);
        assert_eq!(arrived.len(), 1, "{arrived:?}");
        let request = &arrived[0];
        assert_eq!(request.method, "POST");
        assert_eq!(request.path(), "/Library/Media/Updated");
        assert!(request.query().is_empty(), "{request:?}");
        assert_eq!(
            request.header("authorization"),
            Some(format!("MediaBrowser Token=\"{TOKEN}\"").as_str()),
            "{package}"
        );
        assert_eq!(request.header("content-type"), Some("application/json"));
        let body: serde_json::Value = serde_json::from_slice(&request.body).expect("json");
        assert_eq!(
            body,
            serde_json::json!({"Updates": [{"Path": MAPPED, "UpdateType": "Created"}]}),
            "{package}"
        );
    }
}

/// No folder the mapping holds -- none told, one outside it, no mapping, or one whose brace the
/// host made inert -- and every library is refreshed, with an empty body.
#[tokio::test]
async fn jellyfin_and_emby_without_a_mapped_folder_refresh_every_library() {
    for (source, package, host) in [
        (
            JELLYFIN,
            "rd-plugin-jellyfin-notifier",
            "jellyfin.example.org",
        ),
        (EMBY, "rd-plugin-emby-notifier", "emby.example.org"),
    ] {
        for (mapping, folder) in [
            (MAPPING, None),
            (MAPPING, Some("/home/me/Holiday")),
            ("", Some(FOLDER)),
            (MAPPING, Some("/srv/downloads/Holiday {2026}")),
        ] {
            let wire = wire_playing(vec![("/Library/Refresh", NO_CONTENT)]).await;
            let directory = tempfile::tempdir().expect("tempdir");
            let (server, reference) = plugin(directory.path(), &wire, source, package).await;
            let chosen = settings(&[("path_mapping", mapping)]);

            server
                .deliver(finished(
                    &format!("https://{host}"),
                    &reference,
                    &chosen,
                    folder,
                ))
                .await
                .unwrap_or_else(|error| panic!("{package} {folder:?}: {error}"));

            let arrived = arrived(&wire);
            assert_eq!(arrived.len(), 1, "{arrived:?}");
            assert_eq!(
                arrived[0].path(),
                "/Library/Refresh",
                "{package} {folder:?}"
            );
            assert!(arrived[0].body.is_empty(), "{package} {folder:?}");
        }
    }
}

/// A refused update is permanent, and the key -- in its header -- is in no part of the failure.
#[tokio::test]
async fn a_refused_update_never_shows_the_key() {
    for (source, package, host, answer) in [
        (
            JELLYFIN,
            "rd-plugin-jellyfin-notifier",
            "jellyfin.example.org",
            JELLYFIN_UNAUTHORIZED,
        ),
        (
            EMBY,
            "rd-plugin-emby-notifier",
            "emby.example.org",
            EMBY_UNAUTHORIZED,
        ),
    ] {
        let wire = wire_playing(vec![("/Library/Media/Updated", answer)]).await;
        let directory = tempfile::tempdir().expect("tempdir");
        let (server, reference) = plugin(directory.path(), &wire, source, package).await;
        let chosen = settings(&[("path_mapping", MAPPING)]);

        let error = server
            .deliver(finished(
                &format!("https://{host}"),
                &reference,
                &chosen,
                Some(FOLDER),
            ))
            .await
            .expect_err("the key is refused");

        let written = format!("{error:#} {error:?}");
        assert!(!written.contains(TOKEN), "the token leaked: {written}");
        let failure = error.downcast_ref::<rd_core::Failure>().expect("coded");
        assert!(!format!("{:?}", failure.params).contains(TOKEN));
        assert!(!failure.category.is_retryable(), "{package}");
    }
}
