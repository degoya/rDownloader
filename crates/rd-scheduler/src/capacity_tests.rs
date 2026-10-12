//! RD-1270-07: a blocked storage target is released only once a download waiting on it could
//! start, not as soon as its free space is back at the threshold.
//!
//! The supervisor is stopped first in every test, so each supervision round is the test's own,
//! and the free space each round sees is handed in as a disk would report it.

use std::path::{Path, PathBuf};

use rd_core::{DownloadKind, DownloadState, EventEnvelope, EventKind};
use rd_files::StorageTarget;
use tokio::sync::broadcast::{Receiver, error::TryRecvError};

use crate::{BlockReason, FileSpec, PackageSpec, SchedulerConfig, SchedulerHandle};

/// What the waiting download still has to write.
const SIZE: u64 = 10 * 1024 * 1024;

async fn scheduler_over(directory: &Path) -> (SchedulerHandle, rd_db::Database) {
    let database = rd_db::Database::open(directory.join("capacity.sqlite3"))
        .await
        .expect("database");
    let secrets = rd_secrets::SecretStore::open(directory.join("secrets"))
        .await
        .expect("secrets");
    let scheduler = SchedulerHandle::start(
        database.clone(),
        SchedulerConfig::for_directory(directory.join("downloads")),
        secrets,
        None,
        Vec::new(),
    )
    .await
    .expect("scheduler");
    scheduler.shutdown().await.expect("shutdown");
    (scheduler, database)
}

/// The fallback target every package outside a configured root lands on, with its path.
async fn fallback(scheduler: &SchedulerHandle) -> PathBuf {
    scheduler
        .capacity()
        .targets()
        .await
        .into_iter()
        .find(|(target, _)| *target == StorageTarget::Fallback)
        .map(|(_, path)| path)
        .expect("the download directory is a target")
}

fn spec(directory: &Path) -> PackageSpec {
    PackageSpec {
        name: "big".to_owned(),
        destination: directory.join("storage"),
        category_id: None,
        priority: rd_core::DownloadPriority::default(),
        password: None,
        start_paused: false,
        postprocess_level: None,
        script: None,
        enrichment: Vec::new(),
    }
}

fn file(name: &str, size: Option<u64>) -> FileSpec {
    FileSpec {
        source: format!("ftp://files.example/{name}").parse().expect("url"),
        file_name: name.to_owned(),
        size: size.map(|size| rd_core::ByteCount::new(size).expect("size")),
        account_id: None,
        proxy_profile_id: None,
        auth_profile: rd_core::AuthProfileSelection::default(),
        kind: DownloadKind::Ftp,
        media: None,
        remote_credential_id: None,
        replay: None,
        mirror_group: None,
        skipped: false,
        enrichment: Vec::new(),
        secret_fragment: None,
        source_set: None,
    }
}

/// Enqueues one file of `size` and stops it on the start check, as a worker does.
async fn blocked_download(
    scheduler: &SchedulerHandle,
    database: &rd_db::Database,
    directory: &Path,
    size: Option<u64>,
) -> rd_core::DownloadId {
    let (_, files) = scheduler
        .enqueue_package(spec(directory), vec![file("big.bin", size)])
        .await
        .expect("enqueue");
    let id = files[0].id;
    database
        .transition_download(id, DownloadState::Resolving)
        .await
        .expect("resolving");
    database
        .block_download(id, BlockReason::Capacity.as_str())
        .await
        .expect("blocked");
    id
}

/// The `change` of every `storage.capacity` event received since the last call.
fn capacity_changes(events: &mut Receiver<EventEnvelope>) -> Vec<String> {
    let mut changes = Vec::new();
    loop {
        match events.try_recv() {
            Ok(event) if event.kind == EventKind::StorageCapacity => {
                // The shape the automation trigger reads (RD-1270-03/04): a count and the change.
                assert!(event.payload["blocked"].is_u64(), "{}", event.payload);
                changes.push(event.payload["change"].as_str().unwrap_or("").to_owned());
            }
            Ok(_) | Err(TryRecvError::Lagged(_)) => {}
            Err(TryRecvError::Empty | TryRecvError::Closed) => return changes,
        }
    }
}

async fn state_of(database: &rd_db::Database, id: rd_core::DownloadId) -> DownloadState {
    database
        .get_download(id)
        .await
        .expect("read")
        .expect("there")
        .state
}

#[tokio::test]
async fn a_root_in_the_gap_stays_blocked_until_the_waiting_download_fits() {
    let directory = tempfile::tempdir().expect("temp");
    let (scheduler, database) = scheduler_over(directory.path()).await;
    let path = fallback(&scheduler).await;
    let minimum = scheduler.capacity().minimum_free_bytes(&path).await;
    let id = blocked_download(&scheduler, &database, directory.path(), Some(SIZE)).await;
    let mut events = database.subscribe();

    scheduler
        .supervise_target(StorageTarget::Fallback, &path, minimum - 1)
        .await
        .expect("below the threshold");
    assert!(
        scheduler
            .capacity()
            .is_blocked(StorageTarget::Fallback)
            .await
    );
    assert_eq!(capacity_changes(&mut events), ["blocked"]);

    // Above the threshold, below threshold plus the remaining size: the download would block
    // the root again on its start, so nothing is released and nothing is announced.
    for free in [minimum, minimum + SIZE / 2, minimum + SIZE - 1, minimum + 1] {
        scheduler
            .supervise_target(StorageTarget::Fallback, &path, free)
            .await
            .expect("in the gap");
        assert!(
            scheduler
                .capacity()
                .is_blocked(StorageTarget::Fallback)
                .await,
            "held at {free}"
        );
        assert_eq!(state_of(&database, id).await, DownloadState::Blocked);
    }
    assert!(
        capacity_changes(&mut events).is_empty(),
        "no event in the gap"
    );

    // Room for the download and the threshold: one release, one event, the download requeued.
    for _ in 0..2 {
        scheduler
            .supervise_target(StorageTarget::Fallback, &path, minimum + SIZE)
            .await
            .expect("room");
    }
    assert!(
        !scheduler
            .capacity()
            .is_blocked(StorageTarget::Fallback)
            .await
    );
    assert_eq!(capacity_changes(&mut events), ["released"]);
    assert_eq!(state_of(&database, id).await, DownloadState::Queued);
}

#[tokio::test]
async fn an_unknown_size_is_released_at_the_headroom_the_start_check_asks() {
    let directory = tempfile::tempdir().expect("temp");
    let (scheduler, database) = scheduler_over(directory.path()).await;
    let path = fallback(&scheduler).await;
    let minimum = scheduler.capacity().minimum_free_bytes(&path).await;
    let headroom = u64::from(
        scheduler
            .capacity()
            .settings()
            .await
            .storage_unknown_size_headroom
            .max(1),
    );
    let id = blocked_download(&scheduler, &database, directory.path(), None).await;
    let mut events = database.subscribe();

    scheduler
        .supervise_target(StorageTarget::Fallback, &path, minimum - 1)
        .await
        .expect("below the threshold");
    scheduler
        .supervise_target(StorageTarget::Fallback, &path, minimum * headroom - 1)
        .await
        .expect("short of the headroom");
    assert!(
        scheduler
            .capacity()
            .is_blocked(StorageTarget::Fallback)
            .await
    );
    assert_eq!(capacity_changes(&mut events), ["blocked"]);

    scheduler
        .supervise_target(StorageTarget::Fallback, &path, minimum * headroom)
        .await
        .expect("the headroom");
    assert!(
        !scheduler
            .capacity()
            .is_blocked(StorageTarget::Fallback)
            .await
    );
    assert_eq!(capacity_changes(&mut events), ["released"]);
    assert_eq!(state_of(&database, id).await, DownloadState::Queued);
}

#[tokio::test]
async fn a_root_nothing_waits_on_is_released_at_the_threshold() {
    let directory = tempfile::tempdir().expect("temp");
    let (scheduler, database) = scheduler_over(directory.path()).await;
    let path = fallback(&scheduler).await;
    let minimum = scheduler.capacity().minimum_free_bytes(&path).await;
    let mut events = database.subscribe();

    scheduler
        .supervise_target(StorageTarget::Fallback, &path, minimum - 1)
        .await
        .expect("below the threshold");
    assert!(
        scheduler
            .capacity()
            .is_blocked(StorageTarget::Fallback)
            .await
    );
    scheduler
        .supervise_target(StorageTarget::Fallback, &path, minimum)
        .await
        .expect("at the threshold");
    assert!(
        !scheduler
            .capacity()
            .is_blocked(StorageTarget::Fallback)
            .await
    );
    assert_eq!(capacity_changes(&mut events), ["blocked", "released"]);
}

/// An external runner the start check must keep from ever being handed its file.
struct NeverRun;

#[async_trait::async_trait]
impl crate::runner::ExternalRunner for NeverRun {
    fn kind(&self) -> DownloadKind {
        DownloadKind::Ftp
    }

    fn slot_capacity(&self) -> usize {
        1
    }

    fn reuse(&self) -> rd_core::ReuseCapability {
        rd_core::ReuseCapability::default()
    }

    async fn run(
        &self,
        _file: &rd_core::DownloadFile,
        _package: &rd_core::DownloadPackage,
        _cancellation: tokio_util::sync::CancellationToken,
        _limits: crate::runner::RunLimits,
    ) -> anyhow::Result<crate::runner::RunOutcome> {
        panic!("a download the start check blocked reached its runner");
    }
}

/// RD-1270-08: an external download the start check stops is blocked for `capacity`, as an
/// HTTP one is, so releasing the root requeues it instead of leaving it for a manual resume.
#[tokio::test]
async fn an_external_download_blocked_for_storage_is_requeued_by_the_release() {
    let directory = tempfile::tempdir().expect("temp");
    let (scheduler, database) = scheduler_over(directory.path()).await;
    let path = fallback(&scheduler).await;
    let minimum = scheduler.capacity().minimum_free_bytes(&path).await;
    // More than any disk the test runs on has free, so the real probe refuses the start.
    let huge = 1_u64 << 60;
    std::fs::create_dir_all(directory.path().join("storage")).expect("destination");
    let (_, files) = scheduler
        .enqueue_package(spec(directory.path()), vec![file("huge.bin", Some(huge))])
        .await
        .expect("enqueue");
    let id = files[0].id;
    let queued = database
        .get_download(id)
        .await
        .expect("read")
        .expect("there");
    let mut events = database.subscribe();

    scheduler
        .run_external(
            std::sync::Arc::new(NeverRun),
            &queued,
            tokio_util::sync::CancellationToken::new(),
        )
        .await
        .expect("the start check");
    assert_eq!(state_of(&database, id).await, DownloadState::Blocked);
    assert_eq!(
        database
            .downloads_blocked_by(BlockReason::Capacity.as_str())
            .await
            .expect("blocked downloads"),
        [id],
        "blocked with the reason the release looks for"
    );
    assert!(
        scheduler
            .capacity()
            .is_blocked(StorageTarget::Fallback)
            .await
    );
    assert_eq!(capacity_changes(&mut events), ["blocked"]);

    scheduler
        .supervise_target(StorageTarget::Fallback, &path, minimum + huge)
        .await
        .expect("room");
    assert!(
        !scheduler
            .capacity()
            .is_blocked(StorageTarget::Fallback)
            .await
    );
    assert_eq!(capacity_changes(&mut events), ["released"]);
    assert_eq!(state_of(&database, id).await, DownloadState::Queued);
}

/// Every `storage.capacity` payload received since the last call.
fn capacity_payloads(events: &mut Receiver<EventEnvelope>) -> Vec<serde_json::Value> {
    let mut payloads = Vec::new();
    loop {
        match events.try_recv() {
            Ok(event) if event.kind == EventKind::StorageCapacity => payloads.push(event.payload),
            Ok(_) | Err(TryRecvError::Lagged(_)) => {}
            Err(TryRecvError::Empty | TryRecvError::Closed) => return payloads,
        }
    }
}

/// RD-1270-18: each block and each release names its target -- key, name and the path its free
/// space is probed on -- one event per target, so two roots full in one round are two events.
#[tokio::test]
async fn every_storage_event_names_the_one_target_it_is_about() {
    let directory = tempfile::tempdir().expect("temp");
    let (scheduler, database) = scheduler_over(directory.path()).await;
    let media_path = directory.path().join("media");
    let media = database
        .create_storage_root(
            rd_core::StorageRootId::new(),
            rd_db::NewStorageRoot {
                name: "Media".to_owned(),
                path: media_path.to_string_lossy().into_owned(),
                is_default: false,
                minimum_free_bytes: None,
            },
        )
        .await
        .expect("storage root");
    scheduler
        .reload_capacity_config()
        .await
        .expect("roots loaded");
    let fallback_path = fallback(&scheduler).await;
    let minimum = scheduler
        .capacity()
        .minimum_free_bytes(&fallback_path)
        .await;
    let mut events = database.subscribe();

    // One supervision round in which both targets run out of space.
    scheduler
        .supervise_target(StorageTarget::Root(media.id), &media_path, 0)
        .await
        .expect("media full");
    scheduler
        .supervise_target(StorageTarget::Fallback, &fallback_path, 0)
        .await
        .expect("downloads full");
    let blocked = capacity_payloads(&mut events);
    assert_eq!(blocked.len(), 2, "one event per target: {blocked:?}");
    assert_eq!(blocked[0]["change"], "blocked");
    assert_eq!(blocked[0]["target"], media.id.to_string());
    assert_eq!(blocked[0]["name"], "Media");
    assert_eq!(
        blocked[0]["path"],
        media_path.to_string_lossy().into_owned()
    );
    assert_eq!(blocked[0]["blocked"], 1);
    assert_eq!(blocked[1]["change"], "blocked");
    assert_eq!(blocked[1]["target"], StorageTarget::FALLBACK_KEY);
    assert_eq!(blocked[1]["name"], StorageTarget::FALLBACK_NAME);
    assert_eq!(
        blocked[1]["path"],
        fallback_path.to_string_lossy().into_owned()
    );
    assert_eq!(blocked[1]["blocked"], 2);

    // The manual release names the root it released, and nothing in the payload is more than
    // the five keys: no credential, no destination of a download.
    assert!(
        scheduler
            .resume_storage(StorageTarget::Root(media.id))
            .await
            .expect("released")
    );
    let released = capacity_payloads(&mut events);
    assert_eq!(released.len(), 1, "{released:?}");
    assert_eq!(released[0]["change"], "released");
    assert_eq!(released[0]["target"], media.id.to_string());
    assert_eq!(released[0]["name"], "Media");
    assert_eq!(released[0]["blocked"], 1);
    let mut keys: Vec<_> = released[0]
        .as_object()
        .expect("an object")
        .keys()
        .cloned()
        .collect();
    keys.sort();
    assert_eq!(keys, ["blocked", "change", "name", "path", "target"]);

    // The fallback above its threshold, nothing waiting: released, and named.
    scheduler
        .supervise_target(StorageTarget::Fallback, &fallback_path, minimum)
        .await
        .expect("room");
    let released = capacity_payloads(&mut events);
    assert_eq!(released.len(), 1, "{released:?}");
    assert_eq!(released[0]["target"], StorageTarget::FALLBACK_KEY);
    assert_eq!(released[0]["blocked"], 0);
}
