//! The automation engine's two background loops (RD-090-04, RD-090-06).
//!
//! One subscribes to the event bus and turns matching events into queued runs; the other
//! works the queue. Separated for the same reason the notification hub separates them: a
//! webhook that hangs must never delay the event stream.
//!
//! The engine is a bus subscriber rather than a hook inside the writer. An automation that
//! fails, loops or blocks then cannot affect the download that triggered it — which is the
//! property that makes it safe to let people write their own.

use std::{collections::HashMap, sync::Arc, time::Duration};

use rd_automation::{Automation, AutomationVersion, Run, RunState, Trigger};
use rd_core::AutomationId;
use tokio_util::sync::CancellationToken;

use crate::automation_actions::{ActionContext, execute};

/// How often the run queue is swept.
const SWEEP: Duration = Duration::from_secs(5);

/// Newest runs returned by the history endpoint by default.
pub const DEFAULT_HISTORY: u32 = 100;

/// The definition in force of each of `automations`, keyed by automation (audit 1.9.1, API-11).
///
/// The list routes and the area export used to ask for every automation's whole version history
/// in turn. The enabled ones now come from one query, the engine's own; a disabled automation,
/// or one changed between the two reads, still costs one lookup of its versions, because the
/// store has no unfiltered form of that join. An automation without a stored definition is
/// absent from the map.
///
/// # Errors
///
/// The store's error.
pub async fn current_definitions(
    database: &rd_db::Database,
    automations: &[Automation],
) -> anyhow::Result<HashMap<AutomationId, AutomationVersion>> {
    let mut definitions: HashMap<AutomationId, AutomationVersion> = database
        .active_automation_versions()
        .await?
        .into_iter()
        .map(|version| (version.automation_id, version))
        .collect();
    for automation in automations {
        if definitions
            .get(&automation.id)
            .is_some_and(|version| version.version == automation.version)
        {
            continue;
        }
        definitions.remove(&automation.id);
        if let Some(version) = database
            .automation_versions(automation.id)
            .await?
            .into_iter()
            .find(|version| version.version == automation.version)
        {
            definitions.insert(automation.id, version);
        }
    }
    Ok(definitions)
}

/// How much of a failure message is kept in the history.
const MAX_MESSAGE_CHARS: usize = 300;

struct Inner {
    context: ActionContext,
    shutdown: CancellationToken,
}

/// Cloneable handle of the automation engine.
#[derive(Clone)]
pub struct AutomationService {
    inner: Arc<Inner>,
}

impl AutomationService {
    /// Starts both loops and re-queues anything that was mid-flight.
    #[must_use]
    pub fn start(
        database: rd_db::Database,
        secrets: rd_secrets::SecretStore,
        scheduler: rd_scheduler::SchedulerHandle,
        extraction: rd_extract::ExtractionService,
    ) -> Self {
        let service = Self {
            inner: Arc::new(Inner {
                context: ActionContext {
                    database,
                    secrets,
                    scheduler,
                    extraction,
                    http: reqwest::Client::builder()
                        .timeout(Duration::from_secs(30))
                        .build()
                        .unwrap_or_default(),
                },
                shutdown: CancellationToken::new(),
            }),
        };
        tokio::spawn(service.clone().watch_events());
        tokio::spawn(service.clone().run_loop());
        service
    }

    /// Stops both loops.
    pub fn shutdown(&self) {
        self.inner.shutdown.cancel();
    }

    /// Evaluates a trigger and a sample package against the enabled automations.
    ///
    /// This is what the editor's dry run calls. It cannot have an effect by construction:
    /// executing an action is the other loop's job, and nothing here queues a run.
    pub async fn dry_run(
        &self,
        trigger: Trigger,
        package_id: Option<rd_core::PackageId>,
        only: Option<rd_core::AutomationId>,
    ) -> anyhow::Result<Vec<DryRunMatch>> {
        let database = &self.inner.context.database;
        let context = crate::automation_context::package_context(database, package_id).await?;
        Ok(database
            .active_automation_versions()
            .await?
            .into_iter()
            .filter(|version| only.is_none_or(|id| version.automation_id == id))
            .map(|version| DryRunMatch {
                automation_id: version.automation_id,
                trigger_matches: version.trigger == trigger,
                condition_matches: version.condition.matches(&context),
            })
            .collect())
    }

    async fn watch_events(self) {
        // Recovery first: a run left `running` means the process died between claiming it
        // and recording its outcome, and it belongs back in the queue.
        match self.inner.context.database.recover_automation_runs().await {
            Ok(0) => {}
            Ok(count) => tracing::info!(count, "re-queued interrupted automation runs"),
            Err(error) => tracing::warn!(%error, "automation recovery failed"),
        }
        let database = self.inner.context.database.clone();
        let mut events = database.subscribe();
        let mut last = None;
        loop {
            tokio::select! {
                () = self.inner.shutdown.cancelled() => return,
                received = events.recv() => match received {
                    Ok(event) => {
                        last = Some(event.id);
                        self.intake(&event).await;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(missed)) => {
                        // The bus keeps the recent events beside its channel (RD-110-23): what
                        // the channel dropped in a burst is taken from there, with a receiver
                        // from the same step, so no trigger is lost to the burst (DB-05).
                        match last.map(|after| database.resume(after)) {
                            Some((rd_db::Replay::Events(replayed), receiver)) => {
                                events = receiver;
                                for event in replayed {
                                    last = Some(event.id);
                                    self.intake(&event).await;
                                }
                            }
                            _ => tracing::error!(
                                missed,
                                code = "automation.events_missed",
                                "the automation engine fell behind the event bus; the triggers of the missed events did not fire"
                            ),
                        }
                    }
                    Err(_) => return,
                }
            }
        }
    }

    async fn intake(&self, event: &rd_core::EventEnvelope) {
        if let Err(error) = self.queue_for(event).await {
            tracing::warn!(%error, "automation intake failed");
        }
    }

    /// Queues a run for every enabled automation the event satisfies.
    async fn queue_for(&self, event: &rd_core::EventEnvelope) -> anyhow::Result<()> {
        let database = &self.inner.context.database;
        // Most events are none an automation hangs off, and most installations have none
        // switched on: neither costs more than this one read (DB-05).
        if !crate::automation_context::may_trigger(event) {
            return Ok(());
        }
        let versions = database.active_automation_versions().await?;
        if versions.is_empty() {
            return Ok(());
        }
        let Some(matched) = crate::automation_context::classify(database, event).await? else {
            return Ok(());
        };
        for version in versions {
            if !matched.triggers.contains(&version.trigger) {
                continue;
            }
            if !version.condition.matches(&matched.context) {
                continue;
            }
            database
                .queue_automation_run(rd_db::NewRun {
                    automation_id: version.automation_id,
                    automation_version_id: version.id,
                    event_id: event.id.to_string(),
                    package_id: matched.package_id,
                    idempotency_key: rd_automation::idempotency_key(version.id, &event.id),
                })
                .await?;
        }
        Ok(())
    }

    async fn run_loop(self) {
        let mut ticker = tokio::time::interval(SWEEP);
        loop {
            tokio::select! {
                () = self.inner.shutdown.cancelled() => return,
                _ = ticker.tick() => {
                    if let Err(error) = self.sweep().await {
                        tracing::warn!(%error, "automation sweep failed");
                    }
                }
            }
        }
    }

    async fn sweep(&self) -> anyhow::Result<()> {
        let database = &self.inner.context.database;
        for run in database.due_automation_runs(chrono::Utc::now()).await? {
            if let Err(error) = self.advance(&run).await {
                tracing::warn!(%error, run = %run.id, "automation run failed");
            }
        }
        Ok(())
    }

    /// Executes the run's current action and records what happened.
    async fn advance(&self, run: &Run) -> anyhow::Result<()> {
        let database = &self.inner.context.database;
        let Some(version) = database
            .automation_version(run.automation_version_id)
            .await?
        else {
            // The definition was deleted under a queued run. Nothing sensible is left to do.
            return database
                .record_automation_attempt(
                    run.id,
                    RunState::Failed,
                    run.action_index,
                    run.attempt,
                    None,
                    Some("automation version no longer exists".to_owned()),
                )
                .await;
        };
        let Some(action) = version.actions.get(run.action_index as usize) else {
            return database
                .record_automation_attempt(
                    run.id,
                    RunState::Completed,
                    run.action_index,
                    run.attempt,
                    None,
                    None,
                )
                .await;
        };
        database
            .record_automation_attempt(
                run.id,
                RunState::Running,
                run.action_index,
                run.attempt,
                None,
                None,
            )
            .await?;
        match execute(&self.inner.context, action, run.package_id, version.trigger).await {
            Ok(()) => {
                // A stop here leaves the action done and the run `running`: the next start
                // queues it again at this action (RD-180-12, recovery matrix).
                rd_core::failpoint!("automation.before_outcome_recorded", || anyhow::anyhow!(
                    "crash point"
                ));
                let next = run.action_index + 1;
                let done = next as usize >= version.actions.len();
                database
                    .record_automation_attempt(
                        run.id,
                        if done {
                            RunState::Completed
                        } else {
                            RunState::Queued
                        },
                        next,
                        0,
                        None,
                        None,
                    )
                    .await
            }
            Err(error) => {
                let attempt = run.attempt + 1;
                let next_attempt_at = rd_automation::next_attempt_at(attempt, chrono::Utc::now());
                let message = rd_core::redact_text(&format!("{}: {error}", action.kind()))
                    .chars()
                    .take(MAX_MESSAGE_CHARS)
                    .collect::<String>();
                database
                    .record_automation_attempt(
                        run.id,
                        if next_attempt_at.is_some() {
                            RunState::Retrying
                        } else {
                            RunState::Failed
                        },
                        run.action_index,
                        attempt,
                        next_attempt_at,
                        Some(message),
                    )
                    .await
            }
        }
    }
}

/// What a dry run found for one automation.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct DryRunMatch {
    pub automation_id: rd_core::AutomationId,
    /// Whether this automation listens for the trigger that was tried.
    pub trigger_matches: bool,
    /// Whether its condition holds for the sample package.
    pub condition_matches: bool,
}

/// Crash and restart of a run (RD-180-12, recovery matrix).
#[cfg(all(test, feature = "failpoints"))]
mod crash_tests {
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
            let guard =
                rd_core::failpoint::FailpointGuard::once("automation.before_outcome_recorded");
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
}
