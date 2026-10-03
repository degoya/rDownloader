//! Switching a profile on by hand, in front of the schedule (RD-190-20).
//!
//! A child of `bandwidth` so it reaches the shared state directly, as the reload does.

use anyhow::Result;
use chrono::{DateTime, Utc};
use rd_core::BandwidthProfileId;
use rd_limits::{BandwidthProfile, ManualEnd, ManualProfile};

use crate::SchedulerHandle;

/// The profile switched on by hand, if any; JSON `null` once it has ended or been switched back.
const MANUAL_PROFILE_KEY: &str = "bandwidth.manual_profile";

impl SchedulerHandle {
    /// Switches to `profile_id` by hand — `None` for no limits — until `ends` says otherwise
    /// (RD-190-20). The caller has checked that the profile exists and that `at` lies ahead.
    pub async fn switch_bandwidth_profile(
        &self,
        profile_id: Option<BandwidthProfileId>,
        ends: ManualEnd,
        at: Option<DateTime<Utc>>,
    ) -> Result<ManualProfile> {
        let manual = {
            let state = self.config.bandwidth.state.read().await;
            ManualProfile::new(profile_id, ends, at, &state.schedule, Utc::now())
        };
        self.store_manual_profile(Some(&manual)).await?;
        self.announce_manual(Some(&manual));
        self.reload_bandwidth().await?;
        Ok(manual)
    }

    /// Ends a hand-made switch, so the schedule decides again.
    pub async fn return_to_bandwidth_schedule(&self) -> Result<()> {
        self.store_manual_profile(None).await?;
        self.announce_manual(None);
        self.reload_bandwidth().await
    }

    /// The stored switch, if it still holds. One that has ended, or whose profile was deleted
    /// since, is cleared here: the schedule takes over, and nothing is left to reappear later.
    pub(super) async fn load_manual_profile(
        &self,
        profiles: &[BandwidthProfile],
    ) -> Result<Option<ManualProfile>> {
        let Some(stored) = self.database.get_setting(MANUAL_PROFILE_KEY).await? else {
            return Ok(None);
        };
        if stored.is_null() {
            return Ok(None);
        }
        let manual = match serde_json::from_value::<ManualProfile>(stored) {
            Ok(manual) => manual,
            Err(error) => {
                tracing::warn!(%error, "the stored manual bandwidth profile was unreadable and is dropped");
                self.store_manual_profile(None).await?;
                return Ok(None);
            }
        };
        let exists = manual
            .profile_id
            .is_none_or(|id| profiles.iter().any(|profile| profile.id == id));
        if manual.in_force(Utc::now()) && exists {
            return Ok(Some(manual));
        }
        self.store_manual_profile(None).await?;
        self.announce_manual(None);
        Ok(None)
    }

    async fn store_manual_profile(&self, manual: Option<&ManualProfile>) -> Result<()> {
        self.database
            .set_setting(MANUAL_PROFILE_KEY.to_owned(), serde_json::to_value(manual)?)
            .await
    }

    /// Tells the interface the switch changed even when the profile in force did not: picking
    /// the profile the schedule had chosen anyway still changes why it is active.
    fn announce_manual(&self, manual: Option<&ManualProfile>) {
        self.database.broadcast(rd_core::EventEnvelope::new(
            rd_core::EventKind::BandwidthChanged,
            serde_json::json!({
                "entity": "manual",
                "profile": manual.and_then(|manual| manual.profile_id),
                "until": manual.and_then(|manual| manual.until),
            }),
        ));
    }
}
