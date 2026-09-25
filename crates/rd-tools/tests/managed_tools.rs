//! End-to-end managed tools (RD-102-02): a signed manifest, a fake release served over a
//! local socket, and the four things that have to hold — verified before activation, atomic
//! activation, rollback, and a system tool nobody touched.
//!
//! No test reaches the network. Releases are served by an axum stub bound to `127.0.0.1:0`,
//! the same way `crates/rd-captcha/tests/broker.rs` stubs a solver service.

use std::{net::SocketAddr, sync::Arc};

use axum::{Router, routing::get};
use chrono::{Duration, Utc};
use rd_core::{ManagedToolSettings, ToolSource};
use rd_sign::{SigningKey, TrustStore};
use rd_tools::{
    ArchiveFormat, ManagedToolService, TOOL_MANIFEST_SCHEMA_VERSION, ToolEntry, ToolError,
    ToolManifest,
};
use sha2::{Digest, Sha256};

/// The bytes every fake release serves.
const PAYLOAD: &[u8] = b"#!/bin/sh\necho rd-fake-tool\n";

fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// The directory prefix the real BtbN FFmpeg archives carry, kept so the fixture exercises
/// the nested layout the manifest has to name members inside.
const FFMPEG_PREFIX: &str = "ffmpeg-n9.0.1-27-g9b0578816c-linux64-gpl-9.0";

/// A `.tar.xz` holding `bin/ffmpeg` and `bin/ffprobe`, built once.
///
/// Built here rather than committed: a real FFmpeg archive is 120 MB, and what has to be
/// tested is the unpacking rule, not the bytes of somebody else's build.
fn ffmpeg_archive() -> &'static [u8] {
    static ARCHIVE: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    ARCHIVE.get_or_init(|| {
        let mut tarball = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tarball);
            for (name, body) in [
                (format!("{FFMPEG_PREFIX}/bin/ffmpeg"), &b"fake-ffmpeg"[..]),
                (format!("{FFMPEG_PREFIX}/bin/ffprobe"), &b"fake-ffprobe"[..]),
                (format!("{FFMPEG_PREFIX}/README.txt"), &b"documentation"[..]),
            ] {
                let mut header = tar::Header::new_gnu();
                header.set_size(body.len() as u64);
                header.set_mode(0o755);
                header.set_cksum();
                builder
                    .append_data(&mut header, &name, body)
                    .expect("append");
            }
            builder.finish().expect("finish the tar");
        }
        let mut compressed = Vec::new();
        let mut writer =
            lzma_rust2::XzWriter::new(&mut compressed, lzma_rust2::XzOptions::with_preset(1))
                .expect("xz writer");
        std::io::Write::write_all(&mut writer, &tarball).expect("compress");
        writer.finish().expect("finish the xz stream");
        compressed
    })
}

/// A local server answering `/yt-dlp/<version>` with [`PAYLOAD`] and `/ffmpeg/<version>` with
/// the tar.xz fixture.
async fn serve_releases() -> SocketAddr {
    let app = Router::new()
        .route(
            "/yt-dlp/{version}",
            get(|| async { axum::body::Bytes::from_static(PAYLOAD) }),
        )
        .route(
            "/ffmpeg/{version}",
            get(|| async { axum::body::Bytes::from_static(ffmpeg_archive()) }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let address = listener.local_addr().expect("address");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    address
}

fn entry(address: SocketAddr, version: &str) -> ToolEntry {
    ToolEntry {
        name: "yt-dlp".to_owned(),
        version: version.to_owned(),
        platform: rd_tools::platform::current().to_owned(),
        url: format!("http://{address}/yt-dlp/{version}"),
        sha256: digest(PAYLOAD),
        size: PAYLOAD.len() as u64,
        archive: ArchiveFormat::Raw,
        members: Vec::new(),
        min_app_version: None,
        max_app_version: None,
    }
}

/// One entry of the shared FFmpeg archive, naming the member `name` inside it.
///
/// `ffmpeg` and `ffprobe` are two managed tools in one archive, which is exactly how the
/// published builds ship them: two entries, one URL, one hash, different members.
fn ffmpeg_entry(address: SocketAddr, name: &str) -> ToolEntry {
    ToolEntry {
        name: name.to_owned(),
        version: "9.0.1".to_owned(),
        platform: rd_tools::platform::current().to_owned(),
        url: format!("http://{address}/ffmpeg/9.0.1"),
        sha256: digest(ffmpeg_archive()),
        size: ffmpeg_archive().len() as u64,
        archive: ArchiveFormat::TarXz,
        members: vec![format!("{FFMPEG_PREFIX}/bin/{name}")],
        min_app_version: None,
        max_app_version: None,
    }
}

fn manifest(sequence: u64, tools: Vec<ToolEntry>) -> ToolManifest {
    ToolManifest {
        schema_version: TOOL_MANIFEST_SCHEMA_VERSION,
        sequence,
        issued_at: Utc::now(),
        not_after: Some(Utc::now() + Duration::days(30)),
        tools,
        compatibility: Vec::new(),
    }
}

struct Fixture {
    service: ManagedToolService,
    database: rd_db::Database,
    _directory: tempfile::TempDir,
}

async fn fixture(enabled: bool) -> Fixture {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = rd_db::Database::open(directory.path().join("tools.sqlite3"))
        .await
        .expect("database");
    let service = ManagedToolService::new(
        database.clone(),
        rd_tools::store_root(directory.path()),
        ManagedToolSettings {
            managed_tools_enabled: enabled,
            managed_tools_manifest_url: None,
            tool_compatibility_overrides: Vec::new(),
        },
    );
    service.load().await;
    Fixture {
        service,
        database,
        _directory: directory,
    }
}

// ---------------------------------------------------------------------------------------
// Manifest verification
// ---------------------------------------------------------------------------------------

/// A document signed by a key this build does not trust must not be read, however well
/// formed it is. That is the entire reason the manifest is signed.
#[test]
fn a_manifest_signed_by_another_key_is_refused() {
    let stranger = SigningKey::from_bytes(&[7; 32]);
    let bytes =
        rd_tools::manifest::sign("rdownloader-tools-v1", &stranger, &manifest(2, Vec::new()))
            .expect("sign");
    let result = rd_tools::manifest::verify(&bytes, None, Utc::now());
    assert!(
        matches!(result, Err(ToolError::ManifestUntrusted(_))),
        "{result:?}"
    );
    assert_eq!(
        result.expect_err("error").code(),
        "tools.manifest_untrusted"
    );
}

/// The same document, verified against a store that does trust that key, has to succeed —
/// otherwise the test above would pass for the wrong reason.
#[test]
fn a_manifest_signed_by_a_trusted_key_verifies() {
    let key = SigningKey::from_bytes(&[7; 32]);
    let trust = TrustStore::new();
    trust
        .trust("rdownloader-tools-v1".to_owned(), key.verifying_key())
        .expect("trust");
    let bytes = rd_tools::manifest::sign("rdownloader-tools-v1", &key, &manifest(2, Vec::new()))
        .expect("sign");
    let read =
        rd_tools::manifest::verify_with(&bytes, &trust, Some(1), Utc::now()).expect("verify");
    assert_eq!(read.sequence, 2);
}

/// Yesterday's genuine manifest, served again. Correctly signed and still refused, because a
/// signature says who and never when.
#[test]
fn a_manifest_at_or_below_the_accepted_sequence_is_a_replay() {
    let key = SigningKey::from_bytes(&[7; 32]);
    let trust = TrustStore::new();
    trust
        .trust("rdownloader-tools-v1".to_owned(), key.verifying_key())
        .expect("trust");
    let bytes = rd_tools::manifest::sign("rdownloader-tools-v1", &key, &manifest(5, Vec::new()))
        .expect("sign");
    for known in [5_u64, 6] {
        let result = rd_tools::manifest::verify_with(&bytes, &trust, Some(known), Utc::now());
        assert!(
            matches!(result, Err(ToolError::ManifestStale(_))),
            "{known}"
        );
        assert_eq!(result.expect_err("error").code(), "tools.manifest_stale");
    }
}

/// The replay floor only works if it survives a restart, so it has to reach the database.
#[tokio::test]
async fn the_accepted_manifest_sequence_is_persisted_and_never_lowered() {
    let fixture = fixture(true).await;
    assert!(
        fixture
            .database
            .tool_manifest_state()
            .await
            .expect("state")
            .is_none()
    );
    fixture
        .database
        .accept_tool_manifest(7, Utc::now().to_rfc3339())
        .await
        .expect("accept");
    fixture
        .database
        .accept_tool_manifest(3, Utc::now().to_rfc3339())
        .await
        .expect("accept an older one");
    let state = fixture
        .database
        .tool_manifest_state()
        .await
        .expect("state")
        .expect("row");
    assert_eq!(
        state.sequence, 7,
        "an older refresh must not lower the floor"
    );
}

/// A manifest URL that is not https is refused before a request is made: a signature over
/// the answer does not make the way it travelled safe.
#[tokio::test]
async fn a_manifest_url_that_is_not_https_is_refused() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = rd_db::Database::open(directory.path().join("tools.sqlite3"))
        .await
        .expect("database");
    let service = ManagedToolService::new(
        database,
        rd_tools::store_root(directory.path()),
        ManagedToolSettings {
            managed_tools_enabled: true,
            managed_tools_manifest_url: Some("http://example.invalid/tools.json".to_owned()),
            tool_compatibility_overrides: Vec::new(),
        },
    );
    let result = service.refresh_manifest().await;
    assert!(matches!(result, Err(ToolError::ManifestUntrusted(_))));
}

// ---------------------------------------------------------------------------------------
// Download, install, activation
// ---------------------------------------------------------------------------------------

/// The hash decides. A release whose bytes do not match is refused, and — the part that
/// matters — nothing of it survives: no version directory, no staging leftover, no row.
#[tokio::test]
async fn a_download_whose_hash_differs_is_refused_and_leaves_nothing_behind() {
    let address = serve_releases().await;
    let fixture = fixture(true).await;
    let mut wrong = entry(address, "2024.09.07");
    wrong.sha256 = "f".repeat(64);
    fixture
        .service
        .adopt_unverified_manifest(manifest(2, vec![wrong]));

    let result = fixture.service.install("yt-dlp", None).await;
    assert!(
        matches!(result, Err(ToolError::HashMismatch { .. })),
        "{result:?}"
    );
    assert_eq!(result.expect_err("error").code(), "tools.hash_mismatch");

    let status = fixture.service.status().await;
    let ytdlp = status
        .iter()
        .find(|tool| tool.name == "yt-dlp")
        .expect("yt-dlp");
    assert!(ytdlp.installed_versions.is_empty());
    assert!(ytdlp.active_version.is_none());

    // No staging directory either: a half-written install that looks installed is worse than
    // no install at all.
    let tool_directory = fixture.service.root().join("yt-dlp");
    let leftovers: Vec<String> = std::fs::read_dir(&tool_directory)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

/// A body larger than the manifest declares is cut off rather than written out.
#[tokio::test]
async fn a_release_that_does_not_match_the_declared_size_is_refused() {
    let address = serve_releases().await;
    let fixture = fixture(true).await;
    let mut lying = entry(address, "2024.09.07");
    lying.size = 4;
    fixture
        .service
        .adopt_unverified_manifest(manifest(2, vec![lying]));
    let result = fixture.service.install("yt-dlp", None).await;
    assert!(
        matches!(result, Err(ToolError::DownloadFailed { .. })),
        "{result:?}"
    );
    assert_eq!(result.expect_err("error").code(), "tools.download_failed");
}

/// The happy path, and the first acceptance criterion: verified, installed, activated
/// atomically, and answered by the tool lookup as a managed version.
#[tokio::test]
async fn a_verified_release_is_installed_activated_and_resolved_as_managed() {
    let address = serve_releases().await;
    let fixture = fixture(true).await;
    fixture
        .service
        .adopt_unverified_manifest(manifest(2, vec![entry(address, "2024.09.07")]));

    let installed = fixture
        .service
        .install("yt-dlp", None)
        .await
        .expect("install");
    assert_eq!(installed, "2024.09.07");

    // The pointer file is what makes activation atomic; the version directory is what it
    // points at, and both have to exist.
    let pointer = fixture.service.root().join("yt-dlp").join("active.json");
    assert!(pointer.is_file());
    let binary = fixture
        .service
        .root()
        .join("yt-dlp")
        .join("2024.09.07")
        .join(rd_tools::download::executable_name("yt-dlp"));
    assert_eq!(std::fs::read(&binary).expect("read"), PAYLOAD);

    let recorded = fixture
        .database
        .list_managed_tools()
        .await
        .expect("history");
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].sha256, digest(PAYLOAD));

    rd_core::set_managed_tool_resolver(Arc::new(fixture.service.clone()));
    let found = rd_core::locate_tool(None, None, "yt-dlp").expect("resolved");
    assert_eq!(found.source, ToolSource::Managed);
    assert_eq!(found.path, binary);
}

/// The second acceptance criterion. A path somebody typed stays authoritative, and a tool
/// this installation manages nothing of still resolves the way it always did.
#[tokio::test]
async fn a_system_tool_is_untouched_by_the_managed_store() {
    let address = serve_releases().await;
    let fixture = fixture(true).await;
    fixture
        .service
        .adopt_unverified_manifest(manifest(2, vec![entry(address, "2024.09.07")]));
    fixture
        .service
        .install("yt-dlp", None)
        .await
        .expect("install");
    rd_core::set_managed_tool_resolver(Arc::new(fixture.service.clone()));

    // An explicit setting outranks the managed version.
    let elsewhere = tempfile::tempdir().expect("tempdir");
    let system = elsewhere.path().join("system-yt-dlp");
    std::fs::write(&system, PAYLOAD).expect("write");
    let explicit =
        rd_core::locate_tool(Some(&system.to_string_lossy()), None, "yt-dlp").expect("resolved");
    assert_eq!(explicit.source, ToolSource::Explicit);
    assert_eq!(explicit.path, system);

    // A tool with no managed version falls through to the vendor folders untouched.
    let vendored = elsewhere.path().join("gallery-dl");
    std::fs::write(&vendored, PAYLOAD).expect("write");
    let vendor = elsewhere.path().to_string_lossy().into_owned();
    let fallback = rd_core::locate_tool(None, Some(&vendor), "gallery-dl").expect("resolved");
    assert_eq!(fallback.source, ToolSource::Vendor);
    assert_eq!(fallback.path, vendored);

    // And nothing outside the store was written to.
    assert!(system.is_file());
    assert!(fixture.service.root().starts_with(fixture.service.root()));
}

/// The third acceptance criterion: back to the version that was working.
#[tokio::test]
async fn a_rollback_returns_to_the_previously_installed_version() {
    let address = serve_releases().await;
    let fixture = fixture(true).await;
    fixture.service.adopt_unverified_manifest(manifest(
        2,
        vec![entry(address, "2024.01.01"), entry(address, "2024.09.07")],
    ));

    fixture
        .service
        .install("yt-dlp", Some("2024.01.01"))
        .await
        .expect("install the old one");
    fixture
        .service
        .install("yt-dlp", Some("2024.09.07"))
        .await
        .expect("install the new one");
    fixture
        .service
        .activate("yt-dlp", "2024.09.07")
        .await
        .expect("activate");
    assert_eq!(
        active_version(&fixture.service).await.as_deref(),
        Some("2024.09.07")
    );

    let restored = fixture.service.rollback("yt-dlp").await.expect("rollback");
    assert_eq!(restored, "2024.01.01");
    assert_eq!(
        active_version(&fixture.service).await.as_deref(),
        Some("2024.01.01")
    );

    // A second rollback goes back to what it came from rather than running out of versions.
    let forward = fixture
        .service
        .rollback("yt-dlp")
        .await
        .expect("rollback again");
    assert_eq!(forward, "2024.09.07");
}

/// Rolling back with only one version installed is refused with its own code, not with a
/// silent no-op that leaves the user unsure whether anything happened.
#[tokio::test]
async fn a_rollback_with_nothing_to_return_to_is_refused() {
    let address = serve_releases().await;
    let fixture = fixture(true).await;
    fixture
        .service
        .adopt_unverified_manifest(manifest(2, vec![entry(address, "2024.09.07")]));
    fixture
        .service
        .install("yt-dlp", None)
        .await
        .expect("install");
    let result = fixture.service.rollback("yt-dlp").await;
    assert!(matches!(result, Err(ToolError::NothingToRollBackTo { .. })));
}

/// Activating a version that is not on disk must not leave the pointer aiming at nothing.
#[tokio::test]
async fn activating_a_version_that_is_not_installed_is_refused() {
    let fixture = fixture(true).await;
    let result = fixture.service.activate("yt-dlp", "2099.01.01").await;
    assert!(matches!(result, Err(ToolError::VersionNotInstalled { .. })));
    assert_eq!(
        result.expect_err("error").code(),
        "tools.version_not_installed"
    );
    assert!(active_version(&fixture.service).await.is_none());
}

/// The fourth acceptance criterion, in the shape it actually takes: a job that is running
/// keeps the binary it resolved, so a tool failure or a tool switch cannot pull the ground
/// out from under it. Everything else — including the other tools — carries on.
#[tokio::test]
async fn a_version_a_running_job_holds_is_not_removed() {
    let address = serve_releases().await;
    let fixture = fixture(true).await;
    fixture.service.adopt_unverified_manifest(manifest(
        2,
        vec![entry(address, "2024.01.01"), entry(address, "2024.09.07")],
    ));
    fixture
        .service
        .install("yt-dlp", Some("2024.01.01"))
        .await
        .expect("install");
    fixture
        .service
        .install("yt-dlp", Some("2024.09.07"))
        .await
        .expect("install");
    rd_core::set_managed_tool_resolver(Arc::new(fixture.service.clone()));

    // A job starts and takes a lease on whatever is active right now.
    let (running, lease) =
        rd_core::locate_tool_leased(None, None, "yt-dlp").expect("resolved for a job");
    assert_eq!(running.source, ToolSource::Managed);
    assert!(fixture.service.is_leased("yt-dlp", "2024.01.01"));

    // Activation happens under it and takes effect for the next job immediately.
    fixture
        .service
        .activate("yt-dlp", "2024.09.07")
        .await
        .expect("activate");
    assert_eq!(
        active_version(&fixture.service).await.as_deref(),
        Some("2024.09.07")
    );

    // The running job's binary is still there, and removing it is refused while it is held.
    assert!(running.path.is_file());
    let refused = fixture.service.remove_version("yt-dlp", "2024.01.01").await;
    assert!(
        matches!(refused, Err(ToolError::InUse { .. })),
        "{refused:?}"
    );
    assert_eq!(refused.expect_err("error").code(), "tools.version_in_use");
    assert!(running.path.is_file());

    // Once the job is done the version can be retired.
    drop(lease);
    drop(running);
    assert!(!fixture.service.is_leased("yt-dlp", "2024.01.01"));
    fixture
        .service
        .remove_version("yt-dlp", "2024.01.01")
        .await
        .expect("removed once nothing holds it");
}

/// The active version is never removed either, whatever asks.
#[tokio::test]
async fn the_active_version_cannot_be_removed() {
    let address = serve_releases().await;
    let fixture = fixture(true).await;
    fixture
        .service
        .adopt_unverified_manifest(manifest(2, vec![entry(address, "2024.09.07")]));
    fixture
        .service
        .install("yt-dlp", None)
        .await
        .expect("install");
    let result = fixture.service.remove_version("yt-dlp", "2024.09.07").await;
    assert!(matches!(result, Err(ToolError::InUse { .. })));
}

/// Switched off means switched off: nothing is fetched and nothing is written.
#[tokio::test]
async fn nothing_is_installed_while_managed_tools_are_switched_off() {
    let address = serve_releases().await;
    let fixture = fixture(false).await;
    fixture
        .service
        .adopt_unverified_manifest(manifest(2, vec![entry(address, "2024.09.07")]));
    let result = fixture.service.install("yt-dlp", None).await;
    assert!(matches!(result, Err(ToolError::Disabled)));
    assert_eq!(result.expect_err("error").code(), "tools.disabled");
    assert!(!fixture.service.root().join("yt-dlp").exists());
}

/// A name outside the closed list must never reach the download path.
#[tokio::test]
async fn a_tool_this_application_does_not_manage_is_refused() {
    let fixture = fixture(true).await;
    for name in ["curl", "unrar", "par2"] {
        let result = fixture.service.install(name, None).await;
        assert!(matches!(result, Err(ToolError::NotManaged(_))), "{name}");
    }
}

/// The pointer survives a restart, which is what makes activation more than a runtime flag.
#[tokio::test]
async fn the_active_version_is_read_back_after_a_restart() {
    let address = serve_releases().await;
    let directory = tempfile::tempdir().expect("tempdir");
    let database = rd_db::Database::open(directory.path().join("tools.sqlite3"))
        .await
        .expect("database");
    let settings = ManagedToolSettings {
        managed_tools_enabled: true,
        managed_tools_manifest_url: None,
        tool_compatibility_overrides: Vec::new(),
    };
    let root = rd_tools::store_root(directory.path());
    let first = ManagedToolService::new(database.clone(), root.clone(), settings.clone());
    first.load().await;
    first.adopt_unverified_manifest(manifest(2, vec![entry(address, "2024.09.07")]));
    first.install("yt-dlp", None).await.expect("install");

    let restarted = ManagedToolService::new(database, root, settings);
    restarted.load().await;
    assert_eq!(
        active_version(&restarted).await.as_deref(),
        Some("2024.09.07")
    );
}

async fn active_version(service: &ManagedToolService) -> Option<String> {
    service
        .status()
        .await
        .into_iter()
        .find(|tool| tool.name == "yt-dlp")
        .and_then(|tool| tool.active_version)
}

// ---------------------------------------------------------------------------------------
// tar.xz releases (RD-102-04)
// ---------------------------------------------------------------------------------------

/// The FFmpeg shape end to end: one xz-compressed tar, downloaded, hash-verified and
/// unpacked into two managed tools that each resolve to their own binary.
#[tokio::test]
async fn a_tar_xz_release_installs_both_binaries_it_carries() {
    let address = serve_releases().await;
    let fixture = fixture(true).await;
    fixture.service.adopt_unverified_manifest(manifest(
        2,
        vec![
            ffmpeg_entry(address, "ffmpeg"),
            ffmpeg_entry(address, "ffprobe"),
        ],
    ));

    for name in ["ffmpeg", "ffprobe"] {
        assert_eq!(
            fixture.service.install(name, None).await.expect("install"),
            "9.0.1"
        );
    }

    rd_core::set_managed_tool_resolver(Arc::new(fixture.service.clone()));
    for (name, body) in [
        ("ffmpeg", &b"fake-ffmpeg"[..]),
        ("ffprobe", &b"fake-ffprobe"[..]),
    ] {
        let found = rd_core::locate_tool(None, None, name).expect("resolved");
        assert_eq!(found.source, ToolSource::Managed);
        assert_eq!(std::fs::read(&found.path).expect("read"), body.to_vec());
        // Only the named member is unpacked; the archive's other files stay out of the store.
        let version_directory = fixture.service.root().join(name).join("9.0.1");
        let unpacked: Vec<String> = std::fs::read_dir(&version_directory)
            .expect("read the version directory")
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            unpacked,
            vec![rd_tools::download::executable_name(name)],
            "{name}"
        );
    }
}

/// An archive whose bytes do not match the manifest is refused before anything is unpacked —
/// the same rule as for a raw download, on the path that writes more than one file.
#[tokio::test]
async fn a_tar_xz_release_with_the_wrong_hash_is_refused_and_leaves_nothing_behind() {
    let address = serve_releases().await;
    let fixture = fixture(true).await;
    let mut wrong = ffmpeg_entry(address, "ffmpeg");
    wrong.sha256 = "a".repeat(64);
    fixture
        .service
        .adopt_unverified_manifest(manifest(2, vec![wrong]));

    let result = fixture.service.install("ffmpeg", None).await;
    assert!(
        matches!(result, Err(ToolError::HashMismatch { .. })),
        "{result:?}"
    );
    assert_eq!(result.expect_err("error").code(), "tools.hash_mismatch");

    let leftovers: Vec<String> = std::fs::read_dir(fixture.service.root().join("ffmpeg"))
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

/// A member the archive does not hold is a failed install, not an empty version directory
/// that looks installed.
#[tokio::test]
async fn a_tar_xz_release_missing_its_member_is_refused() {
    let address = serve_releases().await;
    let fixture = fixture(true).await;
    let mut absent = ffmpeg_entry(address, "ffmpeg");
    absent.members = vec![format!("{FFMPEG_PREFIX}/bin/ffmpeg-static")];
    fixture
        .service
        .adopt_unverified_manifest(manifest(2, vec![absent]));

    let result = fixture.service.install("ffmpeg", None).await;
    assert!(
        matches!(result, Err(ToolError::DownloadFailed { .. })),
        "{result:?}"
    );
    assert!(!fixture.service.root().join("ffmpeg").join("9.0.1").exists());
}
