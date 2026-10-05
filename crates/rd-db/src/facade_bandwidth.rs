//! Database facade for bandwidth profiles, their weekly schedule and the traffic counters
//! (RD-050-12).

use anyhow::Result;

use crate::{Database, bandwidth_store, commands::BandwidthCommand, writer};

/// Bandwidth profiles, their weekly schedule and the traffic counters (RD-050-12).
impl Database {
    pub async fn list_bandwidth_profiles(&self) -> Result<Vec<rd_limits::BandwidthProfile>> {
        bandwidth_store::list_profiles(&self.readers).await
    }

    pub async fn list_bandwidth_windows(&self) -> Result<Vec<rd_limits::ScheduleWindow>> {
        bandwidth_store::list_windows(&self.readers).await
    }

    pub async fn create_bandwidth_profile(
        &self,
        input: bandwidth_store::NewBandwidthProfile,
    ) -> Result<rd_limits::BandwidthProfile> {
        writer::request(&self.writer, |reply| {
            BandwidthCommand::CreateBandwidthProfile { input, reply }
        })
        .await
    }

    pub async fn update_bandwidth_profile(
        &self,
        id: rd_core::BandwidthProfileId,
        input: bandwidth_store::NewBandwidthProfile,
    ) -> Result<rd_limits::BandwidthProfile> {
        writer::request(&self.writer, |reply| {
            BandwidthCommand::UpdateBandwidthProfile { id, input, reply }
        })
        .await
    }

    pub async fn delete_bandwidth_profile(&self, id: rd_core::BandwidthProfileId) -> Result<()> {
        writer::request(&self.writer, |reply| {
            BandwidthCommand::DeleteBandwidthProfile { id, reply }
        })
        .await
    }

    /// Replaces the weekly schedule as one document.
    pub async fn replace_bandwidth_windows(
        &self,
        windows: Vec<bandwidth_store::NewScheduleWindow>,
    ) -> Result<Vec<rd_limits::ScheduleWindow>> {
        writer::request(&self.writer, |reply| {
            BandwidthCommand::ReplaceBandwidthWindows { windows, reply }
        })
        .await
    }

    pub async fn bandwidth_budgets(&self) -> Result<rd_limits::BudgetStates> {
        bandwidth_store::budget_states(&self.readers).await
    }

    pub async fn store_bandwidth_budget(
        &self,
        profile_id: rd_core::BandwidthProfileId,
        state: rd_limits::BudgetState,
    ) -> Result<()> {
        writer::request(&self.writer, |reply| {
            BandwidthCommand::StoreBandwidthBudget {
                profile_id,
                state,
                reply,
            }
        })
        .await
    }
}
