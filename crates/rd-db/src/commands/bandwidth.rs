//! The commands of `writer/bandwidth.rs`.

use super::Reply;

/// The commands `Writer::handle_bandwidth` applies.
pub(crate) enum BandwidthCommand {
    CreateBandwidthProfile {
        input: crate::bandwidth_store::NewBandwidthProfile,
        reply: Reply<rd_limits::BandwidthProfile>,
    },
    UpdateBandwidthProfile {
        id: rd_core::BandwidthProfileId,
        input: crate::bandwidth_store::NewBandwidthProfile,
        reply: Reply<rd_limits::BandwidthProfile>,
    },
    DeleteBandwidthProfile {
        id: rd_core::BandwidthProfileId,
        reply: Reply<()>,
    },
    ReplaceBandwidthWindows {
        windows: Vec<crate::bandwidth_store::NewScheduleWindow>,
        reply: Reply<Vec<rd_limits::ScheduleWindow>>,
    },
    StoreBandwidthBudget {
        profile_id: rd_core::BandwidthProfileId,
        state: rd_limits::BudgetState,
        reply: Reply<()>,
    },
}
