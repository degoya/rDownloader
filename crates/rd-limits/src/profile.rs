//! A reusable set of limits, activated by the schedule.

use rd_core::BandwidthProfileId;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{budget::BudgetLimits, scope::LimitScope};

/// One scope's own limit inside a profile.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct ScopeLimit {
    #[serde(flatten)]
    pub scope: LimitScope,
    /// Download limit in bytes per second for everything in this scope.
    pub bytes_per_second: u64,
}

/// A named set of limits — "Day", "Night", "Unlimited".
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct BandwidthProfile {
    pub id: BandwidthProfileId,
    pub name: String,
    /// Global download limit; `None` = unlimited.
    pub download_bytes_per_second: Option<rd_core::ByteCount>,
    /// Global torrent upload limit; `None` = unlimited.
    pub upload_bytes_per_second: Option<rd_core::ByteCount>,
    /// Overrides the queue's parallelism while the profile is active; `None` keeps it.
    pub max_active_files: Option<u32>,
    /// Traffic budget for one day in the schedule's timezone.
    pub daily_budget_bytes: Option<rd_core::ByteCount>,
    /// Traffic budget for one calendar month in the schedule's timezone.
    pub monthly_budget_bytes: Option<rd_core::ByteCount>,
    pub scopes: Vec<ScopeLimit>,
}

impl BandwidthProfile {
    /// Creates an otherwise unlimited profile.
    #[must_use]
    pub fn new(name: String) -> Self {
        Self {
            id: BandwidthProfileId::new(),
            name,
            download_bytes_per_second: None,
            upload_bytes_per_second: None,
            max_active_files: None,
            daily_budget_bytes: None,
            monthly_budget_bytes: None,
            scopes: Vec::new(),
        }
    }

    #[must_use]
    pub fn budget_limits(&self) -> BudgetLimits {
        BudgetLimits {
            daily_bytes: self.daily_budget_bytes.map(rd_core::ByteCount::get),
            monthly_bytes: self.monthly_budget_bytes.map(rd_core::ByteCount::get),
        }
    }

    /// The scope limits in the shape [`crate::LimiterRegistry::apply`] expects.
    #[must_use]
    pub fn scope_limits(&self) -> Vec<(LimitScope, u64)> {
        self.scopes
            .iter()
            .filter(|limit| limit.bytes_per_second > 0)
            .map(|limit| (limit.scope.clone(), limit.bytes_per_second))
            .collect()
    }
}
