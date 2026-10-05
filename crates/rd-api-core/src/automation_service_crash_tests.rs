use std::{sync::Arc, time::Duration};

use rd_automation::{Action, RunState};
use tokio_util::sync::CancellationToken;

use super::{AutomationService, Inner};
use crate::automation_actions::ActionContext;

/// What the actions may reach; nothing here dispatches a download.
async fn context(database: &rd_db::Database, directory: &std::path::Path) -> ActionContext {
    let secrets = rd_secrets::SecretStore::open(directory.join("secrets"))
        .await
        .expect("secrets");
    let scheduler = rd_scheduler::SchedulerHandle::start(
        database.clone(),
        rd_scheduler::SchedulerConfig::for_directory(directory.join("downloads")),
        secrets.clone(),
        None,
        Vec::new(),
    )
    .await
    .expect("scheduler");
    let extraction = rd_extract::ExtractionService::start(
        database.clone(),
        rd_extract::ExtractionConfig {
            default_passwords_file: directory.join("passwords.txt"),
            rar_timeout: Duration::from_secs(5),
            default_scripts_directory: directory.join("scripts"),
            hold: rd_core::PostprocessHold::new(),
            quiet_hold: rd_core::PostprocessHold::new(),
            upload_limit: None,
        },
    );
    ActionContext {
        database: database.clone(),
        secrets,
        scheduler,
        extraction,
        http: reqwest::Client::new(),
    }
}

/// The engine without its two loops, so the test decides when a run advances.
fn engine(context: &ActionContext) -> AutomationService {
    AutomationService {
        inner: Arc::new(Inner {
            context: context.clone(),
            shutdown: CancellationToken::new(),
        }),
    }
}

async fn only_run(
    database: &rd_db::Database,
    automation: rd_core::AutomationId,
) -> rd_automation::Run {
    database
        .automation_runs(Some(automation), 1)
        .await
        .expect("runs")
        .remove(0)
}

/// `automation.before_outcome_recorded`: the action took effect, the run does not say so.
#[tokio::test]
async fn a_run_stopped_after_its_action_is_queued_again_at_that_action_and_completes() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = rd_db::Database::open(directory.path().join("automations.sqlite3"))
        .await
        .expect("database");
    let context = context(&database, directory.path()).await;
    let automation = database
        .upsert_automation(
            None,
            rd_db::NewAutomation {
                name: "Pause twice".to_owned(),
                enabled: true,
                trigger: rd_automation::Trigger::PackageCompleted,
                condition: rd_automation::ConditionNode::Always,
                actions: vec![Action::PausePackage, Action::PausePackage],
            },
        )
        .await
        .expect("automation");
    let version = database
        .active_automation_versions()
        .await
        .expect("versions")
        .remove(0);
    // A package without files: pausing it touches nothing, so the run's own record is
    // the only state in play.
    let package = database
        .create_package(rd_db::NewPackage {
            id: rd_core::PackageId::new(),
            name: "Automated".to_owned(),
            destination: directory
                .path()
                .join("downloads")
                .join("Automated")
                .to_string_lossy()
                .into_owned(),
            category_id: None,
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");
    let event = rd_core::EventId::new();
    database
        .queue_automation_run(rd_db::NewRun {
            automation_id: automation.id,
            automation_version_id: version.id,
            event_id: event.to_string(),
            package_id: Some(package.id),
            idempotency_key: rd_automation::idempotency_key(version.id, &event),
        })
        .await
        .expect("queue");

    let first = engine(&context);
    {
        let guard = rd_core::failpoint::FailpointGuard::once("automation.before_outcome_recorded");
        let due = database
            .due_automation_runs(chrono::Utc::now())
            .await
            .expect("due");
        assert_eq!(due.len(), 1);
        assert!(first.advance(&due[0]).await.is_err());
        assert!(guard.fired(), "the crash point was never reached");
    }
    let stopped = only_run(&database, automation.id).await;
    assert_eq!(stopped.state, RunState::Running);
    assert_eq!(stopped.action_index, 0);
    // Claimed: no sweep takes it up again without the recovery a start runs.
    assert!(
        database
            .due_automation_runs(chrono::Utc::now())
            .await
            .expect("due")
            .is_empty()
    );

    // The restart: `watch_events` recovers before it does anything else.
    assert_eq!(
        database.recover_automation_runs().await.expect("recover"),
        1
    );
    let restarted = engine(&context);
    let mut passes = 0;
    loop {
        let due = database
            .due_automation_runs(chrono::Utc::now())
            .await
            .expect("due");
        let Some(run) = due.first() else {
            break;
        };
        if passes == 0 {
            // The action the stop interrupted runs again; it is never skipped.
            assert_eq!(run.state, RunState::Queued);
            assert_eq!(run.action_index, 0);
        }
        restarted.advance(run).await.expect("advance");
        passes += 1;
        assert!(passes <= 2, "the run did not finish in two passes");
    }
    assert_eq!(passes, 2);
    let finished = only_run(&database, automation.id).await;
    assert_eq!(finished.state, RunState::Completed);
    assert_eq!(finished.action_index, 2);
}
