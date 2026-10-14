//! Whether a definition may be stored: its name, actions and condition, and its trigger's half
//! (split out of `model.rs` to keep it under the line limit).

use chrono::{DateTime, Utc};

use crate::{
    condition::ConditionNode,
    model::{
        Action, DefinitionError, LinkDestination, MAX_ACTION_LINKS, MAX_ACTIONS,
        MAX_CONDITION_DEPTH, MAX_LINK_LEN, MAX_NOTIFY_MESSAGE, Trigger,
    },
    schedule::Schedule,
};

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
/// a time, read from `now` in the service's `zone`, and no action that needs a package -- nor does
/// a moment of the whole queue (RD-1290-04); and links into the LinkGrabber never hang off the
/// LinkGrabber's own intake.
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
    if trigger.is_queue_moment() && actions.iter().any(Action::needs_package) {
        return Err(DefinitionError::NeedsPackage);
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
