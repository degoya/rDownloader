//! Which transitions of a package, a captcha, a power countdown and a storage root become a
//! notification, decided on the event's payload alone (RD-1260-04).
//!
//! Each of the four used to be decided on something else: a package on whatever its row said by
//! the time the event was read, so a later stage or a reorder was one more "Package finished";
//! a captcha, a countdown tick and a storage release on the event kind alone, so an answer, a
//! tick and a release each read as the start. Now each emitter names its transition, and only
//! a start or an end somebody needs to hear about is notified -- once.

use super::*;

fn text<'a>(payload: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    payload.get(key).and_then(serde_json::Value::as_str)
}

/// The outcome a `package.state` event announces: a package that just became completed or
/// failed. The same state written again -- a repeated write, a package renamed while it stays
/// finished -- is no new outcome. The automations' package triggers ask the same (RD-1270-03).
pub(crate) fn package_outcome(payload: &serde_json::Value) -> Option<NotificationEvent> {
    let state = text(payload, "state")?;
    let kind = match state {
        "completed" => NotificationEvent::PackageCompleted,
        "failed" => NotificationEvent::PackageFailed,
        _ => return None,
    };
    (text(payload, "previous") != Some(state)).then_some(kind)
}

/// The body of a failed package: how many of its files failed, when it was its files.
pub(super) fn package_failed_body(payload: &serde_json::Value, name: &str) -> String {
    let count = |key: &str| payload.get(key).and_then(serde_json::Value::as_u64);
    match (count("failed_files"), count("files")) {
        (Some(1), Some(1)) => format!("{name}: its file failed."),
        (Some(failed), Some(files)) if failed > 0 => {
            format!("{name}: {failed} of {files} files failed.")
        }
        _ => format!("{name} (failed)"),
    }
}

/// The text of a `captcha.changed` event that is news: the first captcha to wait while none
/// did, and the last waiting one running out unanswered. An answer, a decline, a download that
/// stopped waiting and every further captcha of the same wait say nothing.
pub(super) fn captcha_notice(payload: &serde_json::Value) -> Option<(String, String)> {
    let pending = payload
        .get("pending")
        .and_then(serde_json::Value::as_array)?;
    match text(payload, "change")? {
        "added" if pending.len() == 1 => {
            let host = pending
                .first()
                .and_then(|entry| entry.get("host"))
                .and_then(serde_json::Value::as_str)
                .filter(|host| !host.is_empty());
            Some((
                "A captcha is waiting".to_owned(),
                host.map_or_else(
                    || "A download is waiting for a captcha to be answered.".to_owned(),
                    |host| {
                        format!("A download from {host} is waiting for a captcha to be answered.")
                    },
                ),
            ))
        }
        "expired" if pending.is_empty() => Some((
            "Captcha not answered in time".to_owned(),
            "Nobody answered the waiting captcha in time; its download stopped waiting for it."
                .to_owned(),
        )),
        _ => None,
    }
}

/// The text of a `power.changed` event that is news: a countdown starting, and one that was
/// cancelled or called off by new work. The action running is not -- the machine is going down.
pub(super) fn power_notice(
    payload: &serde_json::Value,
    occurred_at: chrono::DateTime<chrono::Utc>,
) -> Option<(String, String)> {
    let action = |key: &str| match text(payload, key) {
        Some("shutdown") => "Shutdown",
        Some("standby") => "Standby",
        _ => "Power action",
    };
    match text(payload, "state")? {
        "pending" => {
            let name = action("pending");
            let seconds = payload
                .get("runs_at")
                .and_then(|value| {
                    serde_json::from_value::<chrono::DateTime<chrono::Utc>>(value.clone()).ok()
                })
                .map(|runs_at| (runs_at - occurred_at).num_seconds().max(0));
            let when = seconds.map_or_else(
                || "soon".to_owned(),
                |seconds| format!("in {seconds} seconds"),
            );
            Some((
                format!("{name} pending"),
                format!("The queue is done. {name} runs {when} unless you cancel it."),
            ))
        }
        "cancelled" => {
            let name = action("action");
            Some(if text(payload, "reason") == Some("new_work") {
                (
                    format!("{name} called off"),
                    format!(
                        "New work arrived, so the {} does not run.",
                        name.to_lowercase()
                    ),
                )
            } else {
                (
                    format!("{name} cancelled"),
                    format!("The {} countdown was cancelled.", name.to_lowercase()),
                )
            })
        }
        _ => None,
    }
}

/// Whether a `storage.capacity` event is a storage root running out of space; a release is
/// not.
pub(super) fn storage_blocked(payload: &serde_json::Value) -> bool {
    text(payload, "change") == Some("blocked")
}

/// Title and text of "Storage root out of space", naming the root that ran out (RD-1270-18);
/// `None` for a release. An event without a name keeps the general wording.
pub(super) fn storage_notice(payload: &serde_json::Value) -> Option<(String, String)> {
    if !storage_blocked(payload) {
        return None;
    }
    let Some(name) = text(payload, "name") else {
        return Some((
            "Storage root out of space".to_owned(),
            "A storage root fell below its free-space threshold and takes no new work.".to_owned(),
        ));
    };
    let place = text(payload, "path").map_or_else(String::new, |path| format!(" ({path})"));
    Some((
        format!("Storage root out of space: {name}"),
        format!("{name}{place} fell below its free-space threshold and takes no new work."),
    ))
}

#[cfg(test)]
#[path = "transitions_tests.rs"]
mod tests;
