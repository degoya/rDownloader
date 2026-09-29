//! The words a full backup run is described by (RD-160-01), and the one a verification of an
//! archive ends with (RD-160-02).
//!
//! Shared because three layers read them: the store keeps them as text, the backup service
//! decides them, and the API hands them to the interface, which translates each one.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// What started a backup run.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum BackupOrigin {
    /// The schedule found it due.
    Scheduled,
    /// Somebody asked for it, in the interface or through the API.
    Manual,
}

/// Where a backup run stands.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum BackupRunState {
    /// Still writing. At most one run is in this state at a time.
    Running,
    /// The archive is at its destination.
    Succeeded,
    /// The run stopped with a stable error code; nothing reached the destination.
    Failed,
    /// The process stopped while the run was writing; found by the next start.
    Interrupted,
}

impl BackupOrigin {
    /// The stored word.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Scheduled => "scheduled",
            Self::Manual => "manual",
        }
    }

    /// Parses the stored word.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        [Self::Scheduled, Self::Manual]
            .into_iter()
            .find(|origin| origin.as_str() == value)
    }
}

impl BackupRunState {
    /// The stored word.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Interrupted => "interrupted",
        }
    }

    /// Parses the stored word.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        [
            Self::Running,
            Self::Succeeded,
            Self::Failed,
            Self::Interrupted,
        ]
        .into_iter()
        .find(|state| state.as_str() == value)
    }
}

/// Where a verification of one archive at one destination stands (RD-160-02).
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum BackupVerifyState {
    /// Still fetching or reading the archive.
    Running,
    /// The archive is whole and, when its key was at hand, opens with every member intact.
    Passed,
    /// Missing, changed, cut or unreadable; the stable code says which.
    Failed,
    /// The process stopped during the check; found by the next start.
    Interrupted,
}

impl BackupVerifyState {
    /// The stored word.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::Interrupted => "interrupted",
        }
    }

    /// Parses the stored word.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        [Self::Running, Self::Passed, Self::Failed, Self::Interrupted]
            .into_iter()
            .find(|state| state.as_str() == value)
    }
}

#[cfg(test)]
mod tests {
    use super::{BackupOrigin, BackupRunState, BackupVerifyState};

    #[test]
    fn the_stored_words_are_the_serde_words_and_parse_back() {
        for origin in [BackupOrigin::Scheduled, BackupOrigin::Manual] {
            assert_eq!(BackupOrigin::parse(origin.as_str()), Some(origin));
            assert_eq!(
                serde_json::to_value(origin).expect("json"),
                serde_json::json!(origin.as_str())
            );
        }
        for state in [
            BackupRunState::Running,
            BackupRunState::Succeeded,
            BackupRunState::Failed,
            BackupRunState::Interrupted,
        ] {
            assert_eq!(BackupRunState::parse(state.as_str()), Some(state));
            assert_eq!(
                serde_json::to_value(state).expect("json"),
                serde_json::json!(state.as_str())
            );
        }
        assert_eq!(BackupRunState::parse("done"), None);
        for state in [
            BackupVerifyState::Running,
            BackupVerifyState::Passed,
            BackupVerifyState::Failed,
            BackupVerifyState::Interrupted,
        ] {
            assert_eq!(BackupVerifyState::parse(state.as_str()), Some(state));
            assert_eq!(
                serde_json::to_value(state).expect("json"),
                serde_json::json!(state.as_str())
            );
        }
    }
}
