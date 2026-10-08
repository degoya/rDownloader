//! An account whose traffic its hoster reports used up (RD-1190-13, RD-1190-14).
//!
//! A premium account's daily or rolling traffic quota is a property of the account, not of the
//! one link that ran into it: every other link through the same account is refused the same way
//! until the quota frees up again. A plugin says so with a `rate-limited` failure whose stable
//! code ends in [`TRAFFIC_EXHAUSTED_SUFFIX`] (`ddownload.traffic_exhausted`,
//! `keep2share.traffic_exhausted`, `nitroflare.traffic_exhausted`); the scheduler then lets the
//! file wait and does with the account what [`AccountTrafficAction`] says.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{Failure, FailureKind};

/// The end of every stable code that reports an account's traffic as used up.
pub const TRAFFIC_EXHAUSTED_SUFFIX: &str = ".traffic_exhausted";

/// What the queue does while an account's traffic is used up.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    Deserialize,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
    Serialize,
    ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum AccountTrafficAction {
    /// Only the files that ran into the limit wait; the account's other files still start and
    /// find out for themselves.
    Nothing,
    /// The account's other files wait too, until the hoster's wait ends or a check reports
    /// traffic again; other accounts and hosters keep downloading.
    #[default]
    PauseAccount,
    /// The whole queue holds new starts until then; running transfers finish.
    PauseQueue,
}

/// Whether `failure` reports the traffic of the account it ran through as used up.
///
/// Only a `rate-limited` failure counts: the code alone could be reused for a refusal that no
/// wait ends.
#[must_use]
pub fn is_account_traffic_exhausted(failure: &Failure) -> bool {
    matches!(failure.category, FailureKind::RateLimited { .. })
        && failure
            .code
            .as_deref()
            .is_some_and(|code| code.ends_with(TRAFFIC_EXHAUSTED_SUFFIX))
}

#[cfg(test)]
mod tests {
    use super::{AccountTrafficAction, is_account_traffic_exhausted};
    use crate::{Failure, FailureKind};

    fn limited(code: &str) -> Failure {
        Failure::coded(
            FailureKind::RateLimited {
                retry_after_seconds: Some(3600),
            },
            code,
            "limit",
        )
    }

    #[test]
    fn only_a_rate_limit_coded_as_used_up_traffic_counts() {
        assert!(is_account_traffic_exhausted(&limited(
            "ddownload.traffic_exhausted"
        )));
        assert!(is_account_traffic_exhausted(&limited(
            "keep2share.traffic_exhausted"
        )));
        assert!(!is_account_traffic_exhausted(&limited(
            "realdebrid.limit_reached"
        )));
        assert!(!is_account_traffic_exhausted(&Failure::coded(
            FailureKind::AccountInvalid,
            "ddownload.traffic_exhausted",
            "not a wait"
        )));
        assert!(!is_account_traffic_exhausted(&Failure::new(
            FailureKind::RateLimited {
                retry_after_seconds: None
            },
            "uncoded"
        )));
    }

    #[test]
    fn the_action_defaults_to_holding_the_account_and_reads_in_snake_case() {
        assert_eq!(
            AccountTrafficAction::default(),
            AccountTrafficAction::PauseAccount
        );
        assert_eq!(
            serde_json::to_string(&AccountTrafficAction::PauseQueue).expect("json"),
            "\"pause_queue\""
        );
        assert_eq!(
            serde_json::from_str::<AccountTrafficAction>("\"nothing\"").expect("json"),
            AccountTrafficAction::Nothing
        );
    }
}
