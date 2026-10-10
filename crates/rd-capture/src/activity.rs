//! What the tray says about transfers, derived from the service's figures.
//!
//! Kept apart from `tray` so it compiles and is tested on every host, including the Linux
//! machines where the tray itself does not exist.
//!
//! The transfer rate is read, not derived. It used to be computed here from the change in
//! committed bytes between two polls, unsmoothed, while the web UI computed a smoothed one of
//! its own — two answers to the same question, and a third was only a matter of time. The
//! service keeps the rate now (RD-104-02) and both read the same figure. Nothing else about
//! this changes: the summary still carries counts and byte totals only, which is all a capture
//! token may see — no names, no paths.
//!
//! The queue's pause comes with the same figures (RD-1100-06): when a timed pause ends, how many
//! files are paused, and whether this agent was paired with the right to pause and start. Which
//! menu entries that makes, and which of them can be chosen, is [`QueueMenu`], decided here for
//! the same reason as the line.

use chrono::{DateTime, TimeZone, Utc};
use tokio_util::sync::CancellationToken;

use crate::{client::CaptureClient, config};

/// The figures `GET /api/v1/capture/summary` answers with.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Deserialize)]
pub(crate) struct Summary {
    pub active: u32,
    pub queued: u32,
    pub failed: u32,
    #[serde(with = "byte_string")]
    pub committed_bytes: u64,
    #[serde(with = "byte_string")]
    pub total_bytes: u64,
    /// The queue's smoothed rate, as the service measured it.
    #[serde(default)]
    pub bytes_per_second: u64,
    /// How long the queue still needs at that rate, as the service estimated it, and `None`
    /// when no honest figure exists: an entry still to be fetched whose size is unknown, a
    /// rate of zero, a paused transfer (RD-104-02).
    ///
    /// `default` on top of the `Option` because the two absences are different things. A
    /// service older than RD-108-01 sends no field at all, and the agent has to render the
    /// line without a remaining time rather than throw the whole answer away; a current one
    /// sends `null` whenever it has nothing honest to say.
    #[serde(default)]
    pub eta_seconds: Option<u64>,
    /// Files paused, by a person or by a pause of the whole queue.
    #[serde(default)]
    pub paused: u32,
    /// When the timed pause of the whole queue ends, while one holds.
    #[serde(default)]
    pub paused_until: Option<DateTime<Utc>>,
    /// Whether this agent may pause and resume the queue: it was paired with `capture:queue`.
    /// `default` because a service without the field offers no such thing.
    #[serde(default)]
    pub queue_control: bool,
    /// Accounts whose used-up traffic holds downloads back (RD-1190-14); a count, because a
    /// capture token sees no account names.
    #[serde(default)]
    pub traffic_held_accounts: u32,
    /// The soonest next traffic check of those accounts.
    #[serde(default)]
    pub traffic_next_check: Option<DateTime<Utc>>,
}

/// The queue entries the tray menu offers (RD-1100-06, RD-1101-06).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum QueueMenu {
    /// None: the service has not said yet what this agent may do.
    #[default]
    Hidden,
    /// Every entry greyed out, with "Pair again to control the queue" below them: the agent was
    /// paired without queue control. Hiding the entries left a person who paired without the box,
    /// or before 1.10, with no way to learn that the tray can do this at all (RD-1101-06).
    Locked,
    /// The entries, each enabled exactly while it would do something.
    Offered(QueueEntries),
}

/// Which queue entries can be chosen, for an agent paired with queue control.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct QueueEntries {
    /// "Start all": a timed pause holds, or files are paused.
    pub start: bool,
    /// "Pause all": files are downloading or waiting, and no timed pause holds.
    pub pause: bool,
    /// "Pause for 30 minutes" and "Pause for 1 hour": no timed pause holds yet. Offered on an
    /// empty queue too, where a timed pause still keeps new links waiting.
    pub timed_pause: bool,
}

/// What the tray or a shortcut asks of the queue (RD-1100-06, RD-1180-03). The agent makes the
/// request, because it holds the token and the tray never reads the keyring (`main::run`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum QueueRequest {
    /// Pause everything; for `minutes`, or until resumed when `None`.
    Pause { minutes: Option<u32> },
    /// "Start all": end a timed pause, or queue the paused files again.
    Resume,
    /// "Add all from LinkGrabber", started or `paused` (RD-1240-07); answered with a desktop
    /// notification rather than a log line, since nothing else shows what it did. Only the tray
    /// asks for it, and there is no tray on Linux.
    #[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
    AddLinkGrabber { paused: bool },
}

/// Carries out one queue request with the agent's token: from the tray's transfer poll, or from
/// the shortcut listener of a run without a tray.
pub(crate) async fn carry_out(client: &CaptureClient, request: QueueRequest) -> anyhow::Result<()> {
    match request {
        QueueRequest::Pause { minutes } => client.pause_queue(minutes).await,
        QueueRequest::Resume => client.resume_queue().await,
        QueueRequest::AddLinkGrabber { paused } => {
            crate::linkgrabber::add_all(client, paused).await;
            Ok(())
        }
    }
}

/// Byte counts cross the API as strings, because they do not fit a JSON number safely.
mod byte_string {
    use serde::{Deserialize, Deserializer, de::Error};

    /// A string that is not a number is a broken contract, and it is reported as one.
    ///
    /// It used to be `unwrap_or(0)`. A changed format -- `"1.2e9"`, a thousands separator, an
    /// empty field -- then turned into zero, and the tray showed `0%` or "no transfers" while
    /// downloads were running: a broken API contract that looks exactly like a quiet system, and
    /// nothing anywhere said the display was guessing. The caller is `watch_activity`, which
    /// logs the failure and asks again on the next tick, so reporting it costs nothing but makes
    /// it visible (RD-109-10).
    pub(super) fn deserialize<'de, D: Deserializer<'de>>(input: D) -> Result<u64, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Either {
            Text(String),
            Number(u64),
        }
        Ok(match Either::deserialize(input)? {
            Either::Text(value) => value.parse().map_err(|error| {
                D::Error::custom(format!("byte count {value:?} is not a number: {error}"))
            })?,
            Either::Number(value) => value,
        })
    }
}

/// What the tray shows: whether anything is running, the line under the icon, and which queue
/// entries the menu offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Activity {
    pub running: bool,
    pub detail: String,
    pub queue: QueueMenu,
}

/// Renders the tray's transfer line: `3 active · 47% · 12.4 MB/s`.
///
/// Untranslated on purpose. The agent carries no message catalogue (RD-092-05), and the rest of
/// the menu is English for the same reason.
pub(crate) fn describe(summary: Summary) -> Activity {
    let running = summary.active > 0;
    let queue = queue_menu(&summary);
    if !running
        && summary.queued == 0
        && summary.failed == 0
        && summary.paused == 0
        && summary.paused_until.is_none()
        && summary.traffic_held_accounts == 0
    {
        return Activity {
            running: false,
            detail: "no transfers".to_owned(),
            queue,
        };
    }
    let mut parts = Vec::new();
    // First, because it is the reason for everything after it.
    if let Some(until) = summary.paused_until {
        parts.push(format!(
            "paused until {}",
            pause_end(until, &chrono::Local::now())
        ));
    }
    // Next to it, for the same reason: what holds downloads back that nobody paused.
    if summary.traffic_held_accounts > 0 {
        parts.push(traffic_hold(summary, &chrono::Local::now()));
    }
    if summary.active > 0 {
        parts.push(format!("{} active", summary.active));
    }
    if summary.queued > 0 {
        parts.push(format!("{} queued", summary.queued));
    }
    if summary.paused > 0 {
        parts.push(format!("{} paused", summary.paused));
    }
    if summary.failed > 0 {
        parts.push(format!("{} failed", summary.failed));
    }
    if let Some(percent) = progress_percent(summary) {
        parts.push(format!("{percent}%"));
    }
    // Only while something is actually running: a rate next to an idle queue reads as motion,
    // and a remaining time beside one reads as motion twice over. Both stand or fall together.
    if running && summary.bytes_per_second > 0 {
        parts.push(format_rate(summary.bytes_per_second));
        if let Some(seconds) = summary.eta_seconds {
            parts.push(format!("{} left", format_duration(seconds)));
        }
    }
    Activity {
        running,
        detail: parts.join(" · "),
        queue,
    }
}

/// Which queue entries the menu offers for these figures.
///
/// The labels are the web interface's global control, "Start all" and "Pause all"; unlike the
/// web's single toggle the tray lists both, so each is enabled on its own. "Start all" starts
/// what a pause stopped -- failed files stay failed, restarting them is the web interface's
/// decision (RD-1100-06). The summary counts downloading and queued files only, so "Pause all"
/// stays greyed while nothing but resolving or verifying runs; the timed pauses still reach those.
fn queue_menu(summary: &Summary) -> QueueMenu {
    if !summary.queue_control {
        return QueueMenu::Locked;
    }
    let timed = summary.paused_until.is_some();
    QueueMenu::Offered(QueueEntries {
        start: timed || summary.paused > 0,
        pause: !timed && (summary.active > 0 || summary.queued > 0),
        timed_pause: !timed,
    })
}

/// The end of a pause as the clock on the wall says it: `18:30` today, `Oct 5 18:30` on another
/// day. `now` carries the zone, so a test can name one.
fn pause_end<Tz: TimeZone>(until: DateTime<Utc>, now: &DateTime<Tz>) -> String
where
    Tz::Offset: std::fmt::Display,
{
    let local = until.with_timezone(&now.timezone());
    if local.date_naive() == now.date_naive() {
        local.format("%H:%M").to_string()
    } else {
        local.format("%b %-d %H:%M").to_string()
    }
}

/// "account traffic used up, next check 18:30" — "2 accounts'" when more than one is held.
fn traffic_hold<Tz: TimeZone>(summary: Summary, now: &DateTime<Tz>) -> String
where
    Tz::Offset: std::fmt::Display,
{
    let whose = if summary.traffic_held_accounts == 1 {
        "account".to_owned()
    } else {
        format!("{} accounts'", summary.traffic_held_accounts)
    };
    match summary.traffic_next_check {
        Some(at) => format!("{whose} traffic used up, next check {}", pause_end(at, now)),
        None => format!("{whose} traffic used up"),
    }
}

/// Overall progress across everything unfinished, when the sizes are known.
fn progress_percent(summary: Summary) -> Option<u32> {
    if summary.total_bytes == 0 {
        return None;
    }
    let ratio = summary.committed_bytes.min(summary.total_bytes) as f64
        / summary.total_bytes as f64
        * 100.0;
    Some(ratio.round() as u32)
}

/// One decimal up to the gigabyte, which is what fits in a menu line.
fn format_rate(bytes_per_second: u64) -> String {
    const UNITS: [&str; 4] = ["B/s", "KB/s", "MB/s", "GB/s"];
    let mut value = bytes_per_second as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", value.round() as u64, UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// A remaining time for a menu line: at most two units, no leading zeros, days at the top.
///
/// Not the web interface's `formatDuration`, which writes a clock (`1:12:30`). A clock in a
/// tray line sits next to a rate and a percentage with nothing to say which of them it is;
/// `1h 12m` says it in the string itself and is shorter as soon as days are involved.
fn format_duration(seconds: u64) -> String {
    const MINUTE: u64 = 60;
    const HOUR: u64 = 60 * MINUTE;
    const DAY: u64 = 24 * HOUR;
    if seconds >= DAY {
        two_units(seconds / DAY, 'd', seconds % DAY / HOUR, 'h')
    } else if seconds >= HOUR {
        two_units(seconds / HOUR, 'h', seconds % HOUR / MINUTE, 'm')
    } else if seconds >= MINUTE {
        two_units(seconds / MINUTE, 'm', seconds % MINUTE, 's')
    } else {
        format!("{seconds}s")
    }
}

/// `2d 4h`, and a bare `2d` when the smaller unit is zero: a menu line has no room for a
/// segment that says nothing.
fn two_units(major: u64, major_unit: char, minor: u64, minor_unit: char) -> String {
    if minor == 0 {
        format!("{major}{major_unit}")
    } else {
        format!("{major}{major_unit} {minor}{minor_unit}")
    }
}

/// The tray's one line: the server status, and the transfers when there are any.
///
/// Composed here rather than in `tray` so its length can be measured on every host, including
/// the Linux machines where the tray does not exist.
#[cfg(any(windows, target_os = "macos", test))]
pub(crate) fn status_line(status: &str, detail: &str) -> String {
    if detail.is_empty() {
        return status.to_owned();
    }
    format!("{status} \u{2014} {detail}")
}

/// What a Windows tray tooltip holds: a 128-`WCHAR` buffer, the terminating NUL included.
#[cfg(any(windows, target_os = "macos", test))]
pub(crate) const TOOLTIP_LIMIT: usize = 127;

/// The status line, cut to what the platform can carry in a tooltip.
///
/// Only the tooltip is cut; the menu's status item has no such buffer behind it. The cut is a
/// guarantee rather than a precaution: the hostname, the three counts, the rate and now the
/// remaining time are each unbounded, and the longest line they can form runs well past the
/// limit. Characters, not bytes - the buffer counts UTF-16 code units, and every character
/// this line can contain is one of them.
#[cfg(any(windows, target_os = "macos", test))]
pub(crate) fn tooltip(line: &str) -> String {
    if line.chars().count() <= TOOLTIP_LIMIT {
        return line.to_owned();
    }
    let kept: String = line.chars().take(TOOLTIP_LIMIT - 1).collect();
    format!("{kept}\u{2026}")
}

/// Polls the service's figures and reports what the tray should show.
///
/// The interval is `config::STATUS_POLL_INTERVAL`, the same constant the tray's health poll
/// reads, so the two states move together and the icon never contradicts the status line. The
/// rate comes with the summary; nothing here measures across a poll interval any more, so an
/// outage cannot turn into a leap.
///
/// The tray's queue requests are made here too (RD-1100-06), and the summary is read again right
/// after one, so the menu and the status line show its outcome without waiting for the next
/// tick. A refused request is a log line: the summary that follows says what the agent may do,
/// and a tray that may not pause greys its entries out (RD-1101-06).
pub(crate) async fn watch_activity(
    client: CaptureClient,
    cancellation: CancellationToken,
    sink: crate::ActivitySink,
    mut requests: tokio::sync::mpsc::UnboundedReceiver<QueueRequest>,
) {
    loop {
        match client.summary().await {
            Ok(summary) => sink(describe(summary)),
            // `warn`, not `debug`: since RD-109-10 a byte count the service sends in a shape
            // this build cannot read fails here instead of quietly becoming zero, and a broken
            // API contract is worth a line somebody sees. The tray keeps its last figures.
            Err(error) => tracing::warn!(%error, "could not read the transfer summary"),
        }
        tokio::select! {
            () = cancellation.cancelled() => return,
            () = tokio::time::sleep(config::STATUS_POLL_INTERVAL) => {}
            Some(request) = requests.recv() => {
                if let Err(error) = carry_out(&client, request).await {
                    tracing::warn!(%error, ?request, "the tray's queue request was not carried out");
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "activity_tests.rs"]
mod tests;
