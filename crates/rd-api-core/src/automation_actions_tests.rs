//! The actions of RD-1240-10, each against the capability it reaches.

use rd_automation::{Action, LinkDestination, Trigger};
use rd_core::{DownloadPriority, PackageId};

use super::{About, automation_message, execute, message_body};
use crate::automation_test_support::context;

async fn package(database: &rd_db::Database, directory: &std::path::Path) -> PackageId {
    database
        .create_package(rd_db::NewPackage {
            id: PackageId::new(),
            name: "Automated".to_owned(),
            destination: directory
                .join("downloads")
                .join("Automated")
                .to_string_lossy()
                .into_owned(),
            category_id: None,
            priority: DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package")
        .id
}

async fn open(directory: &std::path::Path) -> rd_db::Database {
    rd_db::Database::open(directory.join("actions.sqlite3"))
        .await
        .expect("database")
}

#[tokio::test]
async fn set_priority_gives_the_package_its_priority() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = open(directory.path()).await;
    let context = context(&database, directory.path()).await;
    let package = package(&database, directory.path()).await;
    let action = Action::SetPriority {
        priority: DownloadPriority::High,
    };
    execute(
        &context,
        &action,
        Some(package),
        Trigger::PackageCompleted,
        "run:0",
    )
    .await
    .expect("set priority");
    let stored = database
        .get_package(package)
        .await
        .expect("read")
        .expect("package");
    assert_eq!(stored.priority, DownloadPriority::High);
}

#[tokio::test]
async fn a_package_action_without_a_package_fails_rather_than_guessing() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = open(directory.path()).await;
    let context = context(&database, directory.path()).await;
    for action in [
        Action::SetPriority {
            priority: DownloadPriority::Low,
        },
        Action::ExtractPackage,
    ] {
        assert!(
            execute(&context, &action, None, Trigger::StorageThreshold, "run:0")
                .await
                .is_err(),
            "{action:?} ran without a package"
        );
    }
}

#[tokio::test]
async fn extract_refuses_a_package_with_nothing_finished() {
    // The same refusal the package menu's "Extract" answers with; retried, then given up.
    let directory = tempfile::tempdir().expect("tempdir");
    let database = open(directory.path()).await;
    let context = context(&database, directory.path()).await;
    let package = package(&database, directory.path()).await;
    let error = execute(
        &context,
        &Action::ExtractPackage,
        Some(package),
        Trigger::PackageCompleted,
        "run:0",
    )
    .await
    .expect_err("nothing to extract");
    assert!(error.to_string().contains("no completed files"), "{error}");
}

#[tokio::test]
async fn start_queue_ends_a_pause_of_the_whole_queue() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = open(directory.path()).await;
    let context = context(&database, directory.path()).await;
    // Not paused: already started, which is no failure.
    execute(
        &context,
        &Action::StartQueue,
        None,
        Trigger::Schedule,
        "run:0",
    )
    .await
    .expect("start an unpaused queue");
    context
        .scheduler
        .pause_queue_until_resumed()
        .await
        .expect("pause");
    assert!(context.scheduler.queue_pause().await.is_some());
    execute(
        &context,
        &Action::StartQueue,
        None,
        Trigger::Schedule,
        "run:0",
    )
    .await
    .expect("start");
    assert!(context.scheduler.queue_pause().await.is_none());
}

/// RD-1240-30: `pause_queue` pauses the whole queue with no end, and `start_queue` ends it, so
/// two timed automations make a download window.
#[tokio::test]
async fn pause_queue_holds_the_queue_until_start_queue_ends_it() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = open(directory.path()).await;
    let context = context(&database, directory.path()).await;
    execute(
        &context,
        &Action::PauseQueue,
        None,
        Trigger::Schedule,
        "run:0",
    )
    .await
    .expect("pause");
    let pause = context.scheduler.queue_pause().await.expect("paused");
    assert_eq!(pause.until, None, "it lasts until the queue is started");
    // Pausing a paused queue again is no failure either.
    execute(
        &context,
        &Action::PauseQueue,
        None,
        Trigger::Schedule,
        "run:1",
    )
    .await
    .expect("pause again");
    execute(
        &context,
        &Action::StartQueue,
        None,
        Trigger::Schedule,
        "run:2",
    )
    .await
    .expect("start");
    assert!(context.scheduler.queue_pause().await.is_none());
}

#[tokio::test]
async fn notify_needs_a_target_that_still_exists() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = open(directory.path()).await;
    let context = context(&database, directory.path()).await;
    let action = Action::Notify {
        target_id: rd_core::NotificationTargetId::new(),
        message: "Night queue started".to_owned(),
    };
    let error = execute(&context, &action, None, Trigger::Schedule, "run:0")
        .await
        .expect_err("missing target");
    assert!(error.to_string().contains("no longer exists"), "{error}");
}

#[test]
fn a_notification_says_the_author_s_words_first() {
    assert_eq!(
        message_body(Some(" Done "), Some("Release")),
        "Done\nRelease"
    );
    assert_eq!(message_body(Some("Done"), None), "Done");
    // A webhook keeps what it always sent.
    assert_eq!(message_body(None, Some("Release")), "Release");
    assert_eq!(message_body(None, None), "no package");
}

#[tokio::test]
async fn add_links_to_the_downloads_queues_each_link_once() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = open(directory.path()).await;
    let context = context(&database, directory.path()).await;
    // Held, so nothing here reaches the network.
    context
        .scheduler
        .pause_queue_until_resumed()
        .await
        .expect("pause");
    let action = Action::AddLinks {
        links: vec![
            "https://downloads.example.invalid/files/first.zip".to_owned(),
            "https://downloads.example.invalid/files/second.zip".to_owned(),
        ],
        destination: LinkDestination::Downloads,
    };
    execute(&context, &action, None, Trigger::Schedule, "run:0")
        .await
        .expect("add links");
    // A retry after a partial failure adds only what is missing.
    execute(&context, &action, None, Trigger::Schedule, "run:0")
        .await
        .expect("again");
    let mut names: Vec<String> = database
        .list_downloads()
        .await
        .expect("downloads")
        .into_iter()
        .map(|file| file.file_name)
        .collect();
    names.sort();
    assert_eq!(names, ["first.zip", "second.zip"]);
}

#[tokio::test]
async fn add_links_to_the_linkgrabber_without_its_intake_is_a_retryable_failure() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = open(directory.path()).await;
    let context = context(&database, directory.path()).await;
    let action = Action::AddLinks {
        links: vec!["https://example.invalid/a".to_owned()],
        destination: LinkDestination::LinkGrabber,
    };
    assert!(
        execute(&context, &action, None, Trigger::Schedule, "run:0")
            .await
            .is_err()
    );
}

#[tokio::test]
async fn add_links_to_the_downloads_refuses_this_machine_and_queues_nothing() {
    // An automation's links take the reach of a proposed link from the person's own intake, as
    // the hot folder's `.rdlinks` does: their own network, never this machine.
    let directory = tempfile::tempdir().expect("tempdir");
    let database = open(directory.path()).await;
    let context = context(&database, directory.path()).await;
    context
        .scheduler
        .pause_queue_until_resumed()
        .await
        .expect("pause");
    let action = Action::AddLinks {
        links: vec![
            "https://downloads.example.invalid/files/fine.zip".to_owned(),
            "http://127.0.0.1:9/files/local.zip".to_owned(),
        ],
        destination: LinkDestination::Downloads,
    };
    let error = execute(&context, &action, None, Trigger::Schedule, "run:0")
        .await
        .expect_err("a link to this machine");
    assert!(
        error.to_string().contains(rd_core::CODE_INTERNAL_ADDRESS),
        "{error}"
    );
    assert!(
        database
            .list_downloads()
            .await
            .expect("downloads")
            .is_empty(),
        "a refused action left part of its links queued"
    );
}

/// RD-1240-28: the message carries the run's key and an automation's label, not an empty key
/// and `package_completed`.
#[test]
fn an_automation_message_is_keyed_by_its_run_and_labelled_as_an_automation() {
    let about = About {
        package_id: None,
        trigger: Trigger::Schedule,
        delivery_key: "01a12636-f214-701a-ab2b-57f171851bd9:2",
    };
    let message = automation_message(&about, Some(" lca tick "), None);
    assert_eq!(
        message.idempotency_key,
        "01a12636-f214-701a-ab2b-57f171851bd9:2"
    );
    assert_eq!(message.event, rd_notify::NotificationEvent::Automation);
    assert_eq!(message.payload["event"], "automation");
    assert_eq!(message.payload["trigger"], "schedule");
    assert_eq!(message.payload["message"], "lca tick");
    assert_eq!(message.body, "lca tick");
}
