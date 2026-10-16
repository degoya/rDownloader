//! RD-1320-05 — a mirror on another host carries the profile of its own host; the download's
//! own host keeps the download's profile as before.
//!
//! What is checked is the header list every request of a source is sent with — the probe's and
//! the chunks' — as the multi-source attempt decides it per address. An attempt end to end
//! cannot show it here: every mirror on this machine is isolated by the address rule.

use std::path::Path;

use rd_core::{AuthMethod, AuthOrigin, AuthProfileId, AuthProfileSelection, AuthScope};

use crate::{FileSpec, PackageSpec, SchedulerConfig, SchedulerHandle};

const ORIGIN: &str = "https://origin.test/release.mkv";
const MIRROR: &str = "https://mirror.test/release.mkv";
const COOKIES: &str = "https://cookies.test/release.mkv";

struct Fixture {
    scheduler: SchedulerHandle,
    database: rd_db::Database,
    own: AuthProfileId,
}

async fn profile(
    database: &rd_db::Database,
    secrets: &rd_secrets::SecretStore,
    scope: &str,
    method: AuthMethod,
    secret: &str,
) -> AuthProfileId {
    let secret_ref = secrets.put_string(secret.to_owned()).await.expect("vault");
    database
        .create_auth_profile(rd_db::NewAuthProfile {
            name: scope.to_owned(),
            scope: AuthScope::parse(scope, false).expect("scope"),
            method,
            origin: AuthOrigin::Manual,
            enabled: true,
            expires_at: None,
            username: None,
            secret_ref: Some(secret_ref),
            certificate_ref: None,
        })
        .await
        .expect("profile")
        .id
}

/// A scheduler with a Bearer profile for the origin, one for the mirror's host and a cookie
/// profile for a third host.
async fn fixture(directory: &Path) -> Fixture {
    let database = rd_db::Database::open(directory.join("mirror-profiles.sqlite3"))
        .await
        .expect("database");
    let secrets = rd_secrets::SecretStore::open(directory.join("secrets"))
        .await
        .expect("secrets");
    let scheduler = SchedulerHandle::start(
        database.clone(),
        SchedulerConfig::for_directory(directory.join("downloads")),
        secrets.clone(),
        None,
        Vec::new(),
    )
    .await
    .expect("scheduler");
    let own = profile(
        &database,
        &secrets,
        "origin.test",
        AuthMethod::Bearer,
        "own-token",
    )
    .await;
    profile(
        &database,
        &secrets,
        "mirror.test",
        AuthMethod::Bearer,
        "mirror-token",
    )
    .await;
    profile(
        &database,
        &secrets,
        "cookies.test",
        AuthMethod::Cookies,
        "SID=mirror-cookie",
    )
    .await;
    Fixture {
        scheduler,
        database,
        own,
    }
}

/// One paused download of [`ORIGIN`] with `selection`.
async fn download(
    fixture: &Fixture,
    directory: &Path,
    selection: AuthProfileSelection,
) -> rd_core::DownloadFile {
    let spec = PackageSpec {
        name: "release".to_owned(),
        destination: directory.join("storage"),
        category_id: None,
        priority: rd_core::DownloadPriority::default(),
        password: None,
        start_paused: true,
        postprocess_level: None,
        script: None,
        enrichment: Vec::new(),
    };
    let file = FileSpec {
        source: ORIGIN.parse().expect("source"),
        file_name: "release.mkv".to_owned(),
        size: None,
        account_id: None,
        proxy_profile_id: None,
        auth_profile: selection,
        kind: rd_core::DownloadKind::Http,
        media: None,
        remote_credential_id: None,
        replay: None,
        mirror_group: None,
        skipped: false,
        enrichment: Vec::new(),
        secret_fragment: None,
        source_set: None,
    };
    let (_, files) = fixture
        .scheduler
        .enqueue_package(spec, vec![file])
        .await
        .expect("enqueue");
    let id = files.first().expect("one file").id;
    fixture
        .database
        .get_download(id)
        .await
        .expect("read")
        .expect("download")
}

/// What a request of the download to `target` carries.
async fn headers(fixture: &Fixture, file: &rd_core::DownloadFile, target: &str) -> Vec<String> {
    let network = crate::worker::build_replay_client(&fixture.scheduler, file, None, None)
        .await
        .expect("client");
    super::super::probe::headers_for(
        &fixture.scheduler,
        file,
        &network,
        &target.parse().expect("target"),
    )
    .await
    .expect("headers")
    .expect("no failure")
    .into_iter()
    .map(|(name, value)| format!("{name}: {value}"))
    .collect()
}

/// The acceptance case: the mirror's host gets its own profile, the origin keeps the
/// download's.
#[tokio::test]
async fn a_mirror_carries_its_hosts_profile_and_the_origin_keeps_its_own() {
    let directory = tempfile::tempdir().expect("tempdir");
    let fixture = fixture(directory.path()).await;
    let file = download(&fixture, directory.path(), AuthProfileSelection::Auto).await;

    assert_eq!(
        headers(&fixture, &file, ORIGIN).await,
        ["authorization: Bearer own-token"]
    );
    assert_eq!(
        headers(&fixture, &file, MIRROR).await,
        ["authorization: Bearer mirror-token"]
    );
    assert_eq!(
        headers(&fixture, &file, COOKIES).await,
        ["cookie: SID=mirror-cookie"]
    );
    assert!(
        headers(&fixture, &file, "https://unknown.test/release.mkv")
            .await
            .is_empty()
    );
}

/// A pinned profile is the download's own host's only; a mirror elsewhere still gets its own.
#[tokio::test]
async fn a_pinned_profile_stays_on_the_downloads_own_host() {
    let directory = tempfile::tempdir().expect("tempdir");
    let fixture = fixture(directory.path()).await;
    let pinned = AuthProfileSelection::Pinned(fixture.own);
    let file = download(&fixture, directory.path(), pinned).await;

    assert_eq!(
        headers(&fixture, &file, ORIGIN).await,
        ["authorization: Bearer own-token"]
    );
    assert_eq!(
        headers(&fixture, &file, MIRROR).await,
        ["authorization: Bearer mirror-token"]
    );
}

/// "No profile" is meant for the whole download: no mirror gets one either.
#[tokio::test]
async fn no_profile_sends_none_to_any_mirror() {
    let directory = tempfile::tempdir().expect("tempdir");
    let fixture = fixture(directory.path()).await;
    let file = download(&fixture, directory.path(), AuthProfileSelection::None).await;

    for target in [ORIGIN, MIRROR, COOKIES] {
        assert!(
            headers(&fixture, &file, target).await.is_empty(),
            "{target}"
        );
    }
}
