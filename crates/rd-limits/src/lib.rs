//! Scheduled bandwidth profiles, scoped limits and traffic budgets (RD-050-12).
//!
//! The crate holds the policy only: which limit applies to what, which profile is active
//! when, and how much a period has used. Persisting it and applying it to the transports is
//! the scheduler's job.

mod budget;
mod capabilities;
mod limiter;
mod profile;
mod quiet;
mod schedule;
mod scope;

pub use budget::{
    BudgetExceeded, BudgetKind, BudgetLimits, BudgetPeriod, BudgetState, BudgetStates,
};
pub use capabilities::{RunnerLimitSupport, limit_capabilities};
pub use limiter::{BandwidthLimiter, BindingLimit, LimiterRegistry, ScopedLimiter};
pub use profile::{BandwidthProfile, ScopeLimit};
pub use quiet::{QuietHours, QuietWindow};
pub use schedule::{
    DaySet, MINUTES_PER_DAY, ScheduleWindow, WeeklySchedule, covers_local, default_timezone,
    local_position, parse_timezone,
};
pub use scope::{LimitScope, LimitSource, TransferScope, normalize_host};
