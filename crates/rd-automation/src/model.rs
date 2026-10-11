//! The persisted shape of an automation: what fires it, what has to hold, what it does.

use chrono::{DateTime, Utc};
use rd_core::{AutomationId, AutomationRunId, AutomationVersionId, CategoryId};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{condition::ConditionNode, schedule::Schedule};

/// Upper bound on actions in one automation, so a single event cannot fan out unbounded.
pub const MAX_ACTIONS: usize = 10;
/// Upper bound on the depth of a condition tree.
pub const MAX_CONDITION_DEPTH: usize = 6;
/// Upper bound on the links one `add_links` action hands over.
pub const MAX_ACTION_LINKS: usize = 50;
/// Longest link an `add_links` action carries, in bytes.
pub const MAX_LINK_LEN: usize = 2_048;
/// Longest message a `notify` action sends, in characters.
pub const MAX_NOTIFY_MESSAGE: usize = 500;

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
    /// When a [`Trigger::Schedule`] automation runs; `None` for every other trigger.
    #[serde(default)]
    pub schedule: Option<Schedule>,
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
    /// A Usenet download was given up as beyond repair (RD-1100-02).
    UsenetJobHopeless,
    /// A time of day or an interval came round (RD-1240-10); the automation's `schedule`
    /// says which. Names no package.
    Schedule,
}

impl Trigger {
    /// Every trigger, for the editor and for the contract test that iterates them.
    #[must_use]
    pub const fn all() -> [Self; 14] {
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
            Self::UsenetJobHopeless,
            Self::Schedule,
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
    /// Give the package a queue priority (RD-1240-10).
    SetPriority { priority: rd_core::DownloadPriority },
    /// Pause the whole queue as "pause all" does, until `StartQueue` or the queue's own resume
    /// ends it (RD-1240-30): waiting and running downloads are paused and nothing new starts.
    /// With a time trigger, a pair of these is a download window for those who prefer
    /// automations: pause at 06:00, start at 22:00.
    PauseQueue,
    /// End a pause of the whole queue -- "pause all", a timed pause or a stop mark -- as the
    /// queue's own resume does (RD-1240-10). Nothing to do when the queue is not paused.
    StartQueue,
    /// Unpack the package's completed files now, as the package menu's "Extract" does
    /// (RD-1240-10).
    ExtractPackage,
    /// Send a message of the author's own through a configured notification target
    /// (RD-1240-10). The target decides the channel; the message is the body.
    Notify {
        target_id: rd_core::NotificationTargetId,
        message: String,
    },
    /// Hand links to the LinkGrabber or straight to the downloads (RD-1240-10).
    ///
    /// An automation runs unattended, so its links keep to the address rule of a link proposed
    /// from the person's own intake, as a hot folder's `.rdlinks` does: they may reach the
    /// person's own network, never this machine.
    AddLinks {
        links: Vec<String>,
        #[serde(default)]
        destination: LinkDestination,
    },
}

/// Where an `add_links` action puts its links.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum LinkDestination {
    /// The LinkGrabber, through the same intake a pasted link takes: routing rules, the
    /// online check and the review all apply.
    #[default]
    LinkGrabber,
    /// The downloads, each link a package of its own, as `POST /api/v1/downloads` creates it.
    Downloads,
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
            Self::SetPriority { .. } => "set_priority",
            Self::PauseQueue => "pause_queue",
            Self::StartQueue => "start_queue",
            Self::ExtractPackage => "extract_package",
            Self::Notify { .. } => "notify",
            Self::AddLinks { .. } => "add_links",
        }
    }

    /// Every action kind, in the editor's order; the vocabulary hands this out.
    pub const KINDS: [&str; 11] = [
        "webhook",
        "script",
        "set_category",
        "pause_package",
        "resume_package",
        "set_priority",
        "pause_queue",
        "start_queue",
        "extract_package",
        "notify",
        "add_links",
    ];

    /// Whether the action works on the run's package and so cannot run without one.
    #[must_use]
    pub const fn needs_package(&self) -> bool {
        matches!(
            self,
            Self::SetCategory { .. }
                | Self::PausePackage
                | Self::ResumePackage
                | Self::SetPriority { .. }
                | Self::ExtractPackage
        )
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
    /// A time trigger without a schedule, or a schedule that names no time.
    Schedule(String),
    /// A package action on a trigger that names no package.
    NeedsPackage,
    /// A notify action without a message, or with one that is too long.
    NotifyMessage,
    /// An `add_links` action without links, with too many, or with one that is not a link
    /// its destination takes.
    Links,
    /// An `add_links` action into the LinkGrabber on the trigger its own intake fires, which
    /// would run again on every batch it adds.
    LinksLoop,
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
            Self::Schedule(_) => "automation.schedule_invalid",
            Self::NeedsPackage => "automation.action_needs_package",
            Self::NotifyMessage => "automation.notify_message_invalid",
            Self::Links => "automation.links_invalid",
            Self::LinksLoop => "automation.links_loop",
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
            Self::Schedule(detail) => write!(f, "Schedule is not valid: {detail}"),
            Self::NeedsPackage => {
                f.write_str("A package action needs a trigger that names a package")
            }
            Self::NotifyMessage => write!(
                f,
                "A notification needs a message of 1 to {MAX_NOTIFY_MESSAGE} characters"
            ),
            Self::Links => write!(
                f,
                "Links must be 1 to {MAX_ACTION_LINKS} addresses the destination takes"
            ),
            Self::LinksLoop => {
                f.write_str("Links added to the LinkGrabber would start this automation again")
            }
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
    validate_condition(condition)?;
    for action in actions {
        match action {
            Action::Script { name } if !is_script_name(name) => {
                return Err(DefinitionError::ScriptName);
            }
            Action::Notify { message, .. } => {
                let length = message.trim().chars().count();
                if length == 0 || length > MAX_NOTIFY_MESSAGE {
                    return Err(DefinitionError::NotifyMessage);
                }
            }
            Action::AddLinks { links, destination } => validate_links(links, *destination)?,
            _ => {}
        }
    }
    Ok(())
}

/// The trigger half of a definition (RD-1240-10): a time trigger carries a schedule that names
/// a time, read from `now` in the service's `zone`, and no action that needs a package; and
/// links into the LinkGrabber never hang off the LinkGrabber's own intake.
pub fn validate_trigger<Tz: chrono::TimeZone>(
    trigger: Trigger,
    schedule: Option<&Schedule>,
    actions: &[Action],
    now: DateTime<Utc>,
    zone: &Tz,
) -> Result<(), DefinitionError> {
    if trigger == Trigger::IntakeReceived
        && actions.iter().any(|action| {
            matches!(
                action,
                Action::AddLinks {
                    destination: LinkDestination::LinkGrabber,
                    ..
                }
            )
        })
    {
        return Err(DefinitionError::LinksLoop);
    }
    if trigger != Trigger::Schedule {
        return Ok(());
    }
    let Some(schedule) = schedule else {
        return Err(DefinitionError::Schedule(
            "a time trigger needs a schedule".to_owned(),
        ));
    };
    schedule
        .validate(now, zone)
        .map_err(|error| DefinitionError::Schedule(error.0))?;
    if actions.iter().any(Action::needs_package) {
        return Err(DefinitionError::NeedsPackage);
    }
    Ok(())
}

/// The links of an `add_links` action: absolute addresses, HTTP(S) only for the downloads,
/// which take nothing else directly.
fn validate_links(links: &[String], destination: LinkDestination) -> Result<(), DefinitionError> {
    if links.is_empty() || links.len() > MAX_ACTION_LINKS {
        return Err(DefinitionError::Links);
    }
    for link in links {
        let link = link.trim();
        if link.is_empty() || link.len() > MAX_LINK_LEN {
            return Err(DefinitionError::Links);
        }
        let Ok(parsed) = url::Url::parse(link) else {
            return Err(DefinitionError::Links);
        };
        if destination == LinkDestination::Downloads && !matches!(parsed.scheme(), "http" | "https")
        {
            return Err(DefinitionError::Links);
        }
    }
    Ok(())
}

/// The condition half of [`validate`], for a definition that is only tried out: the editor's
/// draft in a dry run (RD-1120-17), which has no name or actions to judge yet.
pub fn validate_condition(condition: &ConditionNode) -> Result<(), DefinitionError> {
    if condition.depth() > MAX_CONDITION_DEPTH {
        return Err(DefinitionError::ConditionDepth);
    }
    condition
        .validate()
        .map_err(|detail| DefinitionError::Predicate(detail.to_string()))
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
