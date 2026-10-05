//! Database facade for automations, their versions and the run history (RD-090-04).

use anyhow::Result;

use crate::{Database, automation_store, commands::NotifyCommand, writer};

/// Automations, their versions and the run history (RD-090-04).
impl Database {
    pub async fn list_automations(&self) -> Result<Vec<rd_automation::Automation>> {
        automation_store::list(&self.readers).await
    }

    /// The definition in force for every enabled automation.
    pub async fn active_automation_versions(
        &self,
    ) -> Result<Vec<rd_automation::AutomationVersion>> {
        automation_store::active_versions(&self.readers).await
    }

    pub async fn automation_version(
        &self,
        id: rd_core::AutomationVersionId,
    ) -> Result<Option<rd_automation::AutomationVersion>> {
        automation_store::version(&self.readers, id).await
    }

    pub async fn automation_versions(
        &self,
        automation_id: rd_core::AutomationId,
    ) -> Result<Vec<rd_automation::AutomationVersion>> {
        automation_store::versions(&self.readers, automation_id).await
    }

    pub async fn automation_runs(
        &self,
        automation_id: Option<rd_core::AutomationId>,
        limit: u32,
    ) -> Result<Vec<rd_automation::Run>> {
        automation_store::runs(&self.readers, automation_id, limit).await
    }

    /// Runs waiting to start, and retries that have come due.
    pub async fn due_automation_runs(
        &self,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<rd_automation::Run>> {
        automation_store::due_runs(&self.readers, now).await
    }

    pub async fn upsert_automation(
        &self,
        id: Option<rd_core::AutomationId>,
        input: automation_store::NewAutomation,
    ) -> Result<rd_automation::Automation> {
        writer::request(&self.writer, |reply| NotifyCommand::UpsertAutomation {
            id,
            input,
            reply,
        })
        .await
    }

    pub async fn set_automation_enabled(
        &self,
        id: rd_core::AutomationId,
        enabled: bool,
    ) -> Result<rd_automation::Automation> {
        writer::request(&self.writer, |reply| NotifyCommand::SetAutomationEnabled {
            id,
            enabled,
            reply,
        })
        .await
    }

    pub async fn delete_automation(&self, id: rd_core::AutomationId) -> Result<()> {
        writer::request(&self.writer, |reply| NotifyCommand::DeleteAutomation {
            id,
            reply,
        })
        .await
    }

    /// Queues a run; `false` means this event already produced one for this version.
    pub async fn queue_automation_run(&self, input: automation_store::NewRun) -> Result<bool> {
        writer::request(&self.writer, |reply| NotifyCommand::QueueAutomationRun {
            input,
            reply,
        })
        .await
    }

    pub async fn record_automation_attempt(
        &self,
        id: rd_core::AutomationRunId,
        state: rd_automation::RunState,
        action_index: u32,
        attempt: u32,
        next_attempt_at: Option<chrono::DateTime<chrono::Utc>>,
        message: Option<String>,
    ) -> Result<()> {
        writer::request(&self.writer, |reply| {
            NotifyCommand::RecordAutomationAttempt {
                id,
                state,
                action_index,
                attempt,
                next_attempt_at,
                message,
                reply,
            }
        })
        .await
    }

    /// Re-queues runs that were mid-flight when the service stopped.
    pub async fn recover_automation_runs(&self) -> Result<u64> {
        writer::request(&self.writer, |reply| NotifyCommand::RecoverAutomationRuns {
            reply,
        })
        .await
    }
}
