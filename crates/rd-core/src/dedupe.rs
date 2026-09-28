//! What happens when a finished file meets a name that is already taken, and what a runner can
//! reuse of data that is already on disk (RD-150-01, RD-150-02).
//!
//! Only the vocabulary lives here: the scheduler applies a policy, `rd-db` stores it, `rd-api`
//! answers with it and the interface translates it, and each would otherwise keep its own
//! spelling of the same five words.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// What the queue does when a file is about to be put where a file of that name already is.
///
/// One policy is effective per collision, and it is always the one [`effective_collision_policy`]
/// names: the package's own, else its category's, else the global one from the settings.
#[derive(
    Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord, ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum CollisionPolicy {
    /// The new file lands beside the old one as `name (1).ext`. The behaviour before 1.5, and
    /// still the default: it never loses anything.
    #[default]
    Rename,
    /// The new file is not written; the download ends with `download.collision_skipped` and the
    /// existing file stays exactly as it was.
    Skip,
    /// The new file replaces the old one. Explicit only, recorded in the audit log, and refused
    /// while the old file belongs to a transfer that is running or seeding.
    Overwrite,
    /// The two are compared by content. Identical: the existing file is adopted and nothing is
    /// written twice. Different, or not comparable: the new file is renamed.
    Compare,
    /// The download stops in `Blocked` with its own prompt until somebody decides. The prompt
    /// is a row, so it survives a restart, and only this download waits for it.
    Ask,
}

impl CollisionPolicy {
    pub const ALL: [Self; 5] = [
        Self::Rename,
        Self::Skip,
        Self::Overwrite,
        Self::Compare,
        Self::Ask,
    ];

    /// The stored word, which is also the translation key suffix.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Rename => "rename",
            Self::Skip => "skip",
            Self::Overwrite => "overwrite",
            Self::Compare => "compare",
            Self::Ask => "ask",
        }
    }

    /// Parses the stored word; an unknown word is `None`, never a default.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        let value = value.trim();
        Self::ALL
            .into_iter()
            .find(|policy| policy.as_str() == value)
    }
}

/// Where the effective policy came from.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CollisionPolicySource {
    Package,
    Category,
    Global,
}

/// The one policy a collision follows, and which level set it.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, ToSchema)]
pub struct EffectiveCollisionPolicy {
    pub policy: CollisionPolicy,
    pub source: CollisionPolicySource,
}

/// The most specific policy wins: package, then category, then the global setting.
#[must_use]
pub fn effective_collision_policy(
    package: Option<CollisionPolicy>,
    category: Option<CollisionPolicy>,
    global: CollisionPolicy,
) -> EffectiveCollisionPolicy {
    match (package, category) {
        (Some(policy), _) => EffectiveCollisionPolicy {
            policy,
            source: CollisionPolicySource::Package,
        },
        (None, Some(policy)) => EffectiveCollisionPolicy {
            policy,
            source: CollisionPolicySource::Category,
        },
        (None, None) => EffectiveCollisionPolicy {
            policy: global,
            source: CollisionPolicySource::Global,
        },
    }
}

/// An answer to an `ask` prompt. `compare` and `ask` are not answers: a person who is asked
/// decides what happens, rather than handing the question back.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CollisionDecision {
    Rename,
    Skip,
    Overwrite,
}

impl CollisionDecision {
    pub const ALL: [Self; 3] = [Self::Rename, Self::Skip, Self::Overwrite];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Rename => "rename",
            Self::Skip => "skip",
            Self::Overwrite => "overwrite",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        let value = value.trim();
        Self::ALL
            .into_iter()
            .find(|decision| decision.as_str() == value)
    }

    /// The policy that carries this decision out.
    #[must_use]
    pub const fn policy(self) -> CollisionPolicy {
        match self {
            Self::Rename => CollisionPolicy::Rename,
            Self::Skip => CollisionPolicy::Skip,
            Self::Overwrite => CollisionPolicy::Overwrite,
        }
    }
}

/// When the collision was found: before any byte was fetched, or with the verified file
/// waiting in staging because the name was taken while the transfer ran.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CollisionPhase {
    BeforeTransfer,
    AfterTransfer,
}

impl CollisionPhase {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BeforeTransfer => "before_transfer",
            Self::AfterTransfer => "after_transfer",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "before_transfer" => Some(Self::BeforeTransfer),
            "after_transfer" => Some(Self::AfterTransfer),
            _ => None,
        }
    }
}

/// The download ended without writing because the policy said `skip`.
pub const CODE_COLLISION_SKIPPED: &str = "download.collision_skipped";

/// What a runner can do with data that is already on disk (RD-150-02).
///
/// Declared by every runner rather than inferred, so the interface can say why one kind of
/// transfer picks up where it stopped and another starts over, and so a runner added later has
/// to say it too: `rd_scheduler::ExternalRunner::reuse` has no default.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq, ToSchema)]
pub struct ReuseCapability {
    /// A stopped transfer continues from the data it already wrote.
    pub resume_partial: bool,
    /// That data is checked against the source (piece hashes, article checksums, validators)
    /// before it is trusted, rather than taken on the length alone.
    pub recheck_partial: bool,
    /// A finished file already in its final place is recognised and adopted instead of being
    /// fetched a second time.
    pub adopt_completed: bool,
    /// The finished payload is verified against a digest before it counts as complete.
    pub verify_completed: bool,
    /// A finished file that meets a taken name follows the collision policy. `false` for a
    /// runner whose tool names and places its own files (a torrent's tree, yt-dlp's template,
    /// an NZB's articles): those keep their own rule, which the interface states beside the
    /// policy instead of pretending the policy decides.
    pub applies_collision_policy: bool,
}

/// What kind of storage work a history row describes.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum StorageOperationKind {
    /// A finished file carried to another folder, verified by hash when it crossed a device.
    Move,
    /// A duplicate replaced by a link to the identical original.
    Dedupe,
}

impl StorageOperationKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Move => "move",
            Self::Dedupe => "dedupe",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "move" => Some(Self::Move),
            "dedupe" => Some(Self::Dedupe),
            _ => None,
        }
    }
}

/// How a storage operation ended, or that it has not.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum StorageOperationState {
    Running,
    Completed,
    Failed,
    /// The process stopped while it ran. The data is at the old place or verified at the new
    /// one; the next pass of the move finds out which and finishes it.
    Interrupted,
}

impl StorageOperationState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Interrupted => "interrupted",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "running" => Some(Self::Running),
            "completed" => Some(Self::Completed),
            "failed" => Some(Self::Failed),
            "interrupted" => Some(Self::Interrupted),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CollisionDecision, CollisionPhase, CollisionPolicy, CollisionPolicySource,
        StorageOperationKind, StorageOperationState, effective_collision_policy,
    };

    #[test]
    fn every_policy_word_parses_back_and_nothing_else_does() {
        for policy in CollisionPolicy::ALL {
            assert_eq!(CollisionPolicy::parse(policy.as_str()), Some(policy));
            let json = serde_json::to_string(&policy).expect("serialise");
            assert_eq!(json, format!("\"{}\"", policy.as_str()));
        }
        assert_eq!(CollisionPolicy::parse("replace"), None);
        assert_eq!(CollisionPolicy::default(), CollisionPolicy::Rename);
    }

    /// The policy matrix: every combination of the three levels names exactly one policy, and
    /// the most specific level that has one is the one named.
    #[test]
    fn the_most_specific_level_decides_and_names_itself() {
        let levels = std::iter::once(None).chain(CollisionPolicy::ALL.map(Some));
        for package in levels.clone() {
            for category in levels.clone() {
                for global in CollisionPolicy::ALL {
                    let effective = effective_collision_policy(package, category, global);
                    let (policy, source) = match (package, category) {
                        (Some(policy), _) => (policy, CollisionPolicySource::Package),
                        (None, Some(policy)) => (policy, CollisionPolicySource::Category),
                        (None, None) => (global, CollisionPolicySource::Global),
                    };
                    assert_eq!(effective.policy, policy);
                    assert_eq!(effective.source, source);
                }
            }
        }
    }

    #[test]
    fn a_decision_is_carried_out_by_the_policy_of_the_same_name() {
        for decision in CollisionDecision::ALL {
            assert_eq!(CollisionDecision::parse(decision.as_str()), Some(decision));
            assert_eq!(decision.policy().as_str(), decision.as_str());
        }
        assert_eq!(CollisionDecision::parse("ask"), None);
        assert_eq!(CollisionDecision::parse("compare"), None);
    }

    #[test]
    fn stored_words_of_phases_and_operations_round_trip() {
        for phase in [
            CollisionPhase::BeforeTransfer,
            CollisionPhase::AfterTransfer,
        ] {
            assert_eq!(CollisionPhase::parse(phase.as_str()), Some(phase));
        }
        for kind in [StorageOperationKind::Move, StorageOperationKind::Dedupe] {
            assert_eq!(StorageOperationKind::parse(kind.as_str()), Some(kind));
        }
        for state in [
            StorageOperationState::Running,
            StorageOperationState::Completed,
            StorageOperationState::Failed,
            StorageOperationState::Interrupted,
        ] {
            assert_eq!(StorageOperationState::parse(state.as_str()), Some(state));
        }
    }
}
