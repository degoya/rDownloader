//! The persisted shape of an automation: what fires it, what has to hold, what it does.

use chrono::{DateTime, Utc};
use rd_core::{AutomationId, AutomationRunId, AutomationVersionId, CategoryId};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::condition::ConditionNode;

/// Upper bound on actions in one automation, so a single event cannot fan out unbounded.
pub const MAX_ACTIONS: usize = 10;
/// Upper bound on the depth of a condition tree.
pub const MAX_CONDITION_DEPTH: usize = 6;

/// A named automation. The definition lives in its versions, not here.
///
/// Splitting the two is what makes an edit safe while runs are in flight: a run holds the
/// version it started with, so changing an automation never rewrites the rules a running
/// attempt is being judged by.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct Automation {
    pub id: AutomationId,
    pub name: String,
    pub enabled: bool,
    /// Version number of the definition currently in force.
    pub version: u32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// One immutable definition of an automation.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct AutomationVersion {
    pub id: AutomationVersionId,
    pub automation_id: AutomationId,
    pub version: u32,
    pub trigger: Trigger,
    pub condition: ConditionNode,
    pub actions: Vec<Action>,
    pub created_at: DateTime<Utc>,
}

/// What starts an automation.
///
/// A closed set rather than "any event": every variant is one thing a person would describe
/// as a moment in a download's life, and each one has a contract test. The domain event bus
/// carries more than this — configuration changes, captcha prompts, tracker statistics — and
/// none of it is something to hang an action off.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Trigger {
    /// Links arrived in the LinkGrabber.
    IntakeReceived,
    /// A link resolved to a concrete download.
    DownloadResolved,
    /// A download began transferring.
    DownloadStarted,
    /// A download finished successfully.
    DownloadCompleted,
    /// A download failed for good.
    DownloadFailed,
    /// Every download of a package finished and post-processing is done.
    PackageCompleted,
    /// A package ended in failure.
    PackageFailed,
    /// An unpack step finished.
    ExtractionFinished,
    /// A user post-processing script finished.
    ScriptFinished,
    /// An upload step finished.
    UploadFinished,
    /// A storage root started or stopped blocking on its free-space threshold.
    StorageThreshold,
    /// A subscription accepted a new item.
    SubscriptionItem,
}

impl Trigger {
    /// Every trigger, for the editor and for the contract test that iterates them.
    #[must_use]
    pub const fn all() -> [Self; 12] {
        [
            Self::IntakeReceived,
            Self::DownloadResolved,
            Self::DownloadStarted,
            Self::DownloadCompleted,
            Self::DownloadFailed,
            Self::PackageCompleted,
            Self::PackageFailed,
            Self::ExtractionFinished,
            Self::ScriptFinished,
            Self::UploadFinished,
            Self::StorageThreshold,
            Self::SubscriptionItem,
        ]
    }
}

/// What an automation does when it fires.
///
/// Every variant names its effect exactly. There is no "run this command" action: a script
/// is addressed by file name inside the configured scripts directory, which is the same
/// confinement post-processing scripts already have.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Action {
    /// POST a signed JSON payload to a configured notification target.
    Webhook {
        target_id: rd_core::NotificationTargetId,
    },
    /// Run a script from the post-processing scripts directory.
    Script { name: String },
    /// Move the package to a category.
    SetCategory { category_id: CategoryId },
    /// Pause every download of the package.
    PausePackage,
    /// Resume every download of the package.
    ResumePackage,
}

impl Action {
    /// A short, stable identifier of the action kind, for logs and history rows.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Webhook { .. } => "webhook",
            Self::Script { .. } => "script",
            Self::SetCategory { .. } => "set_category",
            Self::PausePackage => "pause_package",
            Self::ResumePackage => "resume_package",
        }
    }
}

/// State of one automation run.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    /// Accepted, not started yet.
    Queued,
    /// An action is being executed.
    Running,
    /// Every action succeeded.
    Completed,
    /// An action failed and will be tried again.
    Retrying,
    /// Given up on after the last attempt.
    Failed,
}

/// One execution of one automation version against one event.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct Run {
    pub id: AutomationRunId,
    pub automation_id: AutomationId,
    pub automation_version_id: AutomationVersionId,
    /// The event that started it, for display and for the idempotency key.
    pub event_id: String,
    /// The package the actions operate on; `None` for a trigger that names none.
    pub package_id: Option<rd_core::PackageId>,
    pub state: RunState,
    /// Index of the action being executed or waiting to be retried.
    pub action_index: u32,
    pub attempt: u32,
    pub next_attempt_at: Option<DateTime<Utc>>,
    /// Redacted outcome of the last attempt.
    pub message: Option<String>,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

/// Why an automation definition cannot be stored or enabled.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DefinitionError {
    /// The name is empty or too long.
    Name,
    /// No actions, or more than [`MAX_ACTIONS`].
    ActionCount,
    /// The condition tree nests deeper than [`MAX_CONDITION_DEPTH`].
    ConditionDepth,
    /// A predicate carries a value the operator cannot use.
    Predicate(String),
    /// A script action names a file that cannot be a script name.
    ScriptName,
}

impl DefinitionError {
    /// The stable error code the API reports.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Name => "automation.name_invalid",
            Self::ActionCount => "automation.action_count_invalid",
            Self::ConditionDepth => "automation.condition_too_deep",
            Self::Predicate(_) => "automation.predicate_invalid",
            Self::ScriptName => "automation.script_name_invalid",
        }
    }
}

impl std::fmt::Display for DefinitionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Name => f.write_str("Automation name must be between 1 and 100 characters"),
            Self::ActionCount => write!(f, "An automation needs 1 to {MAX_ACTIONS} actions"),
            Self::ConditionDepth => {
                write!(
                    f,
                    "Conditions may nest at most {MAX_CONDITION_DEPTH} levels"
                )
            }
            Self::Predicate(detail) => write!(f, "Condition is not valid: {detail}"),
            Self::ScriptName => f.write_str("Script name is not a valid file name"),
        }
    }
}

/// Validates a definition before it is stored.
///
/// Runs on the way in rather than on the way out: an automation that cannot be evaluated
/// must not reach the database, because the engine would then have to decide at trigger time
/// what to do with it — and the only safe thing there is to ignore it silently, which is how
/// an automation that never fires becomes impossible to debug.
pub fn validate(
    name: &str,
    condition: &ConditionNode,
    actions: &[Action],
) -> Result<(), DefinitionError> {
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed.chars().count() > 100 {
        return Err(DefinitionError::Name);
    }
    if actions.is_empty() || actions.len() > MAX_ACTIONS {
        return Err(DefinitionError::ActionCount);
    }
    if condition.depth() > MAX_CONDITION_DEPTH {
        return Err(DefinitionError::ConditionDepth);
    }
    condition
        .validate()
        .map_err(|detail| DefinitionError::Predicate(detail.to_string()))?;
    for action in actions {
        if let Action::Script { name } = action
            && !is_script_name(name)
        {
            return Err(DefinitionError::ScriptName);
        }
    }
    Ok(())
}

/// Whether a string can name a script in the scripts directory.
///
/// The same rule the post-processing runner applies: one file name, no directory, no leading
/// dot. Checked here as well so an automation is refused while it is being written rather
/// than failing on its first run.
#[must_use]
pub fn is_script_name(name: &str) -> bool {
    let name = name.trim();
    !name.is_empty()
        && name.chars().count() <= 128
        && !name.starts_with('.')
        && name.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-')
        })
}
