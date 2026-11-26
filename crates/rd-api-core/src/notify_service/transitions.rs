//! Which transitions of a package, a captcha, a power countdown, a storage root and the whole
//! queue (RD-1290-04) become a notification, decided on the event's payload alone (RD-1260-04).
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
pub(super) fn package_failed_body(
    payload: &serde_json::Value,
    name: impl Into<rd_notify::Words>,
) -> Text {
    let count = |key: &str| payload.get(key).and_then(serde_json::Value::as_u64);
    match (count("failed_files"), count("files")) {
        (Some(1), Some(1)) => Text::new("package_failed.body_single"),
        (Some(failed), Some(files)) if failed > 0 => Text::new("package_failed.body_files")
            .with("failed", failed)
            .with("files", files),
        _ => Text::new("package_failed.body"),
    }
    .with("name", name)
}

/// The text of a `captcha.changed` event that is news: the first captcha to wait while none
/// did, and the last waiting one running out unanswered. An answer, a decline, a download that
/// stopped waiting and every further captcha of the same wait say nothing.
pub(super) fn captcha_notice(payload: &serde_json::Value) -> Option<(Text, Text)> {
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
                Text::new("captcha.waiting_title"),
                host.map_or_else(
                    || Text::new("captcha.waiting_body"),
                    |host| Text::new("captcha.waiting_body_host").with("host", host),
                ),
            ))
        }
        "expired" if pending.is_empty() => Some((
            Text::new("captcha.expired_title"),
            Text::new("captcha.expired_body"),
        )),
        _ => None,
    }
}

/// The text of a `power.changed` event that is news: a countdown starting, and one that was
/// cancelled or called off by new work. The action running is not -- the machine is going down.
pub(super) fn power_notice(
    payload: &serde_json::Value,
    occurred_at: rd_core::Timestamp,
) -> Option<(Text, Text)> {
    // The action as a title starts with it, and as the middle of a sentence names it.
    let action = |key: &str| match text(payload, key) {
        Some("shutdown") => (
            Text::new("power.shutdown"),
            Text::new("power.shutdown_inline"),
        ),
        Some("standby") => (
            Text::new("power.standby"),
            Text::new("power.standby_inline"),
        ),
        _ => (Text::new("power.action"), Text::new("power.action_inline")),
    };
    match text(payload, "state")? {
        "pending" => {
            let (name, _) = action("pending");
            let seconds = payload
                .get("runs_at")
                .and_then(|value| serde_json::from_value::<rd_core::Timestamp>(value.clone()).ok())
                .map(|runs_at| (runs_at - occurred_at).as_secs().max(0));
            let body = seconds.map_or_else(
                || Text::new("power.pending_body_soon"),
                |seconds| Text::new("power.pending_body").with("seconds", seconds.to_string()),
            );
            Some((
                Text::new("power.pending_title").with("action", name.clone()),
                body.with("action", name),
            ))
        }
        "cancelled" => {
            let (name, inline) = action("action");
            let (title, body) = if text(payload, "reason") == Some("new_work") {
                (
                    Text::new("power.called_off_title"),
                    Text::new("power.called_off_body"),
                )
            } else {
                (
                    Text::new("power.cancelled_title"),
                    Text::new("power.cancelled_body"),
                )
            };
            Some((title.with("action", name), body.with("action", inline)))
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
pub(super) fn storage_notice(payload: &serde_json::Value) -> Option<(Text, Text)> {
    if !storage_blocked(payload) {
        return None;
    }
    let Some(name) = text(payload, "name") else {
        return Some((
            Text::new("storage_blocked.title"),
            Text::new("storage_blocked.body"),
        ));
    };
    let body = text(payload, "path").map_or_else(
        || Text::new("storage_blocked.body_named"),
        |path| Text::new("storage_blocked.body_path").with("path", path),
    );
    Some((
        Text::new("storage_blocked.title_named").with("name", name),
        body.with("name", name),
    ))
}

/// The moment a `queue.state` event announces (RD-1290-04): the queue paused, resumed or ran
/// empty. The power supervisor sends one per transition; the automations' queue triggers ask the
/// same.
pub(crate) fn queue_moment(payload: &serde_json::Value) -> Option<NotificationEvent> {
    match text(payload, "state")? {
        "paused" => Some(NotificationEvent::QueuePaused),
        "resumed" => Some(NotificationEvent::QueueResumed),
        "finished" => Some(NotificationEvent::QueueFinished),
        _ => None,
    }
}

/// The notification of a `queue.state` event: the moment, its title and its text.
pub(super) fn queue_notice(payload: &serde_json::Value) -> Option<(NotificationEvent, Text, Text)> {
    let moment = queue_moment(payload)?;
    let (title, body) = match moment {
        NotificationEvent::QueuePaused => (
            Text::new("queue_paused.title"),
            match text(payload, "reason") {
                Some("automation") => Text::new("queue_paused.body_automation"),
                Some("schedule") => Text::new("queue_paused.body_schedule"),
                Some("stop_mark") => Text::new("queue_paused.body_stop_mark"),
                Some("game_mode") => Text::new("queue_paused.body_game_mode"),
                _ => Text::new("queue_paused.body_manual"),
            },
        ),
        NotificationEvent::QueueResumed => (
            Text::new("queue_resumed.title"),
            Text::new("queue_resumed.body"),
        ),
        _ => (
            Text::new("queue_finished.title"),
            Text::new("queue_finished.body"),
        ),
    };
    Some((moment, title, body))
}

#[cfg(test)]
#[path = "transitions_tests.rs"]
mod tests;
