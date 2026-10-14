//! The placeholders a rule's own wording may name, and which event fills which (RD-1280-02).
//!
//! One table for the service that fills them, the rule editor (`GET
//! /api/v1/notifications/placeholders`) and the user handbook: the wiki's table is
//! [`placeholder_table_markdown`], committed as `crates/rd-notify/placeholders.md` and held to it
//! by a test. No placeholder carries a secret: an account is named by its label and provider,
//! never by its user name, and a target's address is none of them.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::model::NotificationEvent;

/// One value a template may name in braces, `{package}`.
#[derive(
    Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Placeholder {
    /// The event's code, `package_completed`.
    Event,
    /// `info`, `warning` or `error`.
    Severity,
    /// When it happened, `HH:MM` in the service's time zone.
    Time,
    /// The day it happened, `YYYY-MM-DD` in the service's time zone.
    Date,
    /// The built-in title.
    Title,
    /// The built-in text.
    Message,
    /// The active profile's traffic budget, read when the delivery is queued (RD-1290-02): how
    /// much of it is used, as `{size}` writes a size; empty without an active budget.
    BudgetUsed,
    /// What is left of it.
    BudgetLeft,
    /// Its limit.
    BudgetLimit,
    /// Its period, `daily` or `monthly`: of the two, the one with less left.
    BudgetPeriod,
    Package,
    File,
    Category,
    State,
    Size,
    Path,
    Error,
    StorageTarget,
    Version,
    Plugin,
    Account,
    /// Why the queue paused (RD-1290-04): `manual`, `automation`, `schedule`, `stop_mark` or
    /// `game_mode`.
    Reason,
}

use Placeholder as P;

impl Placeholder {
    /// Every placeholder, in the order the editor and the handbook list them.
    pub const ALL: [Self; 22] = [
        P::Event,
        P::Severity,
        P::Time,
        P::Date,
        P::Title,
        P::Message,
        P::BudgetUsed,
        P::BudgetLeft,
        P::BudgetLimit,
        P::BudgetPeriod,
        P::Package,
        P::File,
        P::Category,
        P::State,
        P::Size,
        P::Path,
        P::Error,
        P::StorageTarget,
        P::Version,
        P::Plugin,
        P::Account,
        P::Reason,
    ];

    /// What every event that takes a template fills; the budget's four stay empty without an
    /// active budget.
    pub const COMMON: [Self; 10] = [
        P::Event,
        P::Severity,
        P::Time,
        P::Date,
        P::Title,
        P::Message,
        P::BudgetUsed,
        P::BudgetLeft,
        P::BudgetLimit,
        P::BudgetPeriod,
    ];

    /// The name between the braces.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            P::Event => "event",
            P::Severity => "severity",
            P::Time => "time",
            P::Date => "date",
            P::Title => "title",
            P::Message => "message",
            P::BudgetUsed => "budget_used",
            P::BudgetLeft => "budget_left",
            P::BudgetLimit => "budget_limit",
            P::BudgetPeriod => "budget_period",
            P::Package => "package",
            P::File => "file",
            P::Category => "category",
            P::State => "state",
            P::Size => "size",
            P::Path => "path",
            P::Error => "error",
            P::StorageTarget => "storage_target",
            P::Version => "version",
            P::Plugin => "plugin",
            P::Account => "account",
            P::Reason => "reason",
        }
    }

    /// The placeholder a name between braces stands for; `None` for a name the table lacks.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|placeholder| placeholder.name() == name)
    }

    /// What it holds, in English, for the handbook's table.
    #[must_use]
    pub const fn description(self) -> &'static str {
        match self {
            P::Event => "The event's code, such as package_completed",
            P::Severity => "info, warning or error",
            P::Time => "When it happened, HH:MM in the service's time zone",
            P::Date => "The day it happened, YYYY-MM-DD in the service's time zone",
            P::Title => "The built-in title",
            P::Message => "The built-in text",
            P::BudgetUsed => "Traffic of the active budget used so far, empty without one",
            P::BudgetLeft => "Traffic of the active budget left",
            P::BudgetLimit => "The active budget's limit",
            P::BudgetPeriod => "The active budget's period, daily or monthly",
            P::Package => "The package's name",
            P::File => "The file's name",
            P::Category => "The category's name",
            P::State => "The package's state, such as completed or failed",
            P::Size => "The size, such as 1.4 GiB",
            P::Path => "The folder on disk",
            P::Error => "What went wrong",
            P::StorageTarget => "The storage root or backup destination",
            P::Version => "The version",
            P::Plugin => "The plugin's name",
            P::Account => "The account's label and provider, never its user name",
            P::Reason => {
                "Why the queue paused: manual, automation, schedule, stop_mark or game_mode"
            }
        }
    }

    /// A sample value for the editor's preview. `time` and `date` are the clock's, filled by the
    /// route that hands the table out.
    #[must_use]
    pub const fn example(self) -> &'static str {
        match self {
            P::Event => "package_completed",
            P::Severity => "info",
            P::Time => "14:32",
            P::Date => "2026-10-13",
            P::Title => "Package finished: Holiday photos",
            P::Message => "Holiday photos (completed)",
            P::BudgetUsed => "12.5 GiB",
            P::BudgetLeft => "37.5 GiB",
            P::BudgetLimit => "50.0 GiB",
            P::BudgetPeriod => "monthly",
            P::Package => "Holiday photos",
            P::File => "IMG_2048.jpg",
            P::Category => "Pictures",
            P::State => "completed",
            P::Size => "1.4 GiB",
            P::Path => "/downloads/Pictures/Holiday photos",
            P::Error => "The server refused the request (403)",
            P::StorageTarget => "Downloads",
            P::Version => "1.28.0",
            P::Plugin => "Example Hoster",
            P::Account => "Main (example.com)",
            P::Reason => "schedule",
        }
    }
}

impl NotificationEvent {
    /// Whether a rule's own wording applies. A burst of starts or of added links arrives as one
    /// summary and keeps its text, and an automation's message goes through no rule.
    #[must_use]
    pub fn takes_template(self) -> bool {
        !self.coalesces() && self != Self::Automation
    }

    /// The placeholders this event fills, the common ones first; none for an event that keeps
    /// its own text ([`Self::takes_template`]).
    #[must_use]
    pub fn placeholders(self) -> Vec<Placeholder> {
        if !self.takes_template() {
            return Vec::new();
        }
        Placeholder::COMMON
            .into_iter()
            .chain(self.specific_placeholders().iter().copied())
            .collect()
    }

    /// What this event fills beyond the common ones.
    fn specific_placeholders(self) -> &'static [Placeholder] {
        match self {
            Self::PackageCompleted => &[P::Package, P::Category, P::State, P::Size, P::Path],
            Self::PackageFailed => &[P::Package, P::Category, P::State, P::Size, P::Error],
            Self::StorageBlocked => &[P::StorageTarget, P::Path],
            Self::BackupFailed => &[P::Error],
            Self::BackupVerifyFailed => &[P::StorageTarget, P::Error],
            Self::UpdateAvailable | Self::UpdateInstalled => &[P::Version],
            Self::UpdateFailed => &[P::Version, P::Error],
            Self::PluginUpdateAvailable => &[P::Plugin, P::Version],
            Self::PluginUpdateFailed => &[P::Plugin, P::Version, P::Error],
            Self::AccountExpiring => &[P::Account],
            Self::AccountInvalid => &[P::Account, P::Error],
            Self::UsenetJobHopeless => &[P::Package, P::Category],
            Self::StopMarkReached => &[P::Package, P::File, P::Category],
            Self::StreamRecorded => &[P::File, P::Package, P::Category, P::Size],
            Self::SubscriptionMatched => &[P::Category],
            Self::QueuePaused => &[P::Reason],
            Self::BudgetExhausted
            | Self::CaptchaWaiting
            | Self::PowerPending
            | Self::ServiceRestarting
            | Self::UsenetQuotaReached
            | Self::QueueResumed
            | Self::QueueFinished
            | Self::DownloadStarted
            | Self::LinksAdded
            | Self::Automation => &[],
        }
    }

    /// The code an event goes by on the wire, `package_completed`.
    #[must_use]
    pub fn code(self) -> String {
        serde_json::to_value(self)
            .ok()
            .and_then(|value| value.as_str().map(str::to_owned))
            .unwrap_or_default()
    }
}

/// The handbook's placeholder tables, generated from this module: what each placeholder holds,
/// then which event fills which beyond the common ones.
#[must_use]
pub fn placeholder_table_markdown() -> String {
    let braced = |placeholder: Placeholder| format!("`{{{}}}`", placeholder.name());
    let mut out = String::from("| Placeholder | Holds | Example |\n| --- | --- | --- |\n");
    for placeholder in Placeholder::ALL {
        out.push_str(&format!(
            "| {} | {} | {} |\n",
            braced(placeholder),
            placeholder.description(),
            placeholder.example()
        ));
    }
    let common: Vec<String> = Placeholder::COMMON.into_iter().map(braced).collect();
    out.push_str(&format!("\nEvery event fills {}.\n\n", common.join(", ")));
    out.push_str("| Event | Also fills |\n| --- | --- |\n");
    for event in NotificationEvent::all() {
        let also = if !event.takes_template() {
            "(own wording does not apply: a burst arrives as one summary)".to_owned()
        } else if event.specific_placeholders().is_empty() {
            "(nothing more)".to_owned()
        } else {
            let names: Vec<String> = event
                .specific_placeholders()
                .iter()
                .copied()
                .map(braced)
                .collect();
            names.join(", ")
        };
        out.push_str(&format!("| `{}` | {also} |\n", event.code()));
    }
    out
}
