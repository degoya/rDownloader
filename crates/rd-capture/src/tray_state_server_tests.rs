//! The two status lines (RD-1240-06): the agent's, and the server's with its version.

use url::Url;

use super::TrayState;
use crate::{
    activity::{Activity, QueueMenu, TOOLTIP_LIMIT},
    client::health_version,
    status::{ServerStatus, agent_label, server_label},
};

fn service() -> Url {
    Url::parse("http://127.0.0.1:8710").expect("valid URL")
}

fn version(text: &str) -> Option<String> {
    Some(text.to_owned())
}

fn transfers(detail: &str) -> Activity {
    Activity {
        running: true,
        detail: detail.to_owned(),
        queue: QueueMenu::Hidden,
    }
}

/// The first line names the agent and where it points, and no longer the server's state.
#[test]
fn the_agent_line_names_the_agent_and_its_address_only() {
    assert_eq!(
        agent_label(Some(&service())),
        format!(
            "rDownloader Capture v{} \u{2014} 127.0.0.1:8710",
            env!("CARGO_PKG_VERSION")
        )
    );
    assert_eq!(
        agent_label(None),
        format!(
            "rDownloader Capture v{} \u{2014} not configured",
            env!("CARGO_PKG_VERSION")
        )
    );
    let mut state = TrayState::new(Some(service()));
    state.on_server_status(ServerStatus::Running, version("1.24.0"));
    state.on_transfers(transfers("1 active"));
    let line = state.surface().status_line;
    assert!(!line.contains("server"), "{line}");
    assert_eq!(
        line,
        format!("{} \u{2014} 1 active", agent_label(Some(&service())))
    );
    let tooltip = state.surface().tooltip;
    assert!(
        line.chars().count() < tooltip.chars().count(),
        "the agent's line is shorter than the one line the tray had: {tooltip}"
    );
}

/// The second line carries the version while the service runs, and only then.
#[test]
fn the_server_line_names_the_version_while_the_service_runs() {
    let mut state = TrayState::new(Some(service()));
    let update = state.on_server_status(ServerStatus::Running, version("1.24.0"));
    assert_eq!(
        update.server_line.as_deref(),
        Some("Server v1.24.0 \u{2014} running")
    );
    assert_eq!(
        state.surface().server_line,
        "Server v1.24.0 \u{2014} running"
    );

    let update = state.on_server_status(ServerStatus::Unreachable, None);
    assert_eq!(
        update.server_line.as_deref(),
        Some("Server \u{2014} not reachable"),
        "a service that does not answer has no version to show"
    );
}

/// A version is never shown beside a state in which the service did not answer with it.
#[test]
fn a_version_outside_the_running_state_is_not_shown() {
    let mut state = TrayState::new(Some(service()));
    state.on_server_status(ServerStatus::Starting, version("1.24.0"));
    assert_eq!(state.surface().server_line, "Server \u{2014} starting");
    assert_eq!(
        server_label(ServerStatus::Running, None),
        "Server \u{2014} running",
        "a running service without a readable version"
    );
}

/// A new version with the same state is news (the service was updated); the same pair again
/// is not, since the health poll sends on every tick.
#[test]
fn only_a_changed_version_or_state_rewrites_the_server_line() {
    let mut state = TrayState::new(Some(service()));
    state.on_server_status(ServerStatus::Running, version("1.24.0"));
    assert!(
        state
            .on_server_status(ServerStatus::Running, version("1.24.0"))
            .is_empty()
    );
    let update = state.on_server_status(ServerStatus::Running, version("1.24.1"));
    assert_eq!(
        update.server_line.as_deref(),
        Some("Server v1.24.1 \u{2014} running")
    );
    assert_eq!(update.status_line, None);
    assert_eq!(update.tooltip, None);
}

/// The tooltip has one line only: it keeps naming the server's state, at the front, so the cut
/// to what Windows can carry never takes it away.
#[test]
fn the_tooltip_still_names_the_server_state_within_the_limit() {
    let mut state = TrayState::new(Some(service()));
    state.on_server_status(ServerStatus::Running, version("1.24.0"));
    let update = state.on_transfers(transfers(&"x".repeat(2 * TOOLTIP_LIMIT)));
    let tooltip = update.tooltip.expect("the tooltip is rewritten");
    assert_eq!(tooltip.chars().count(), TOOLTIP_LIMIT);
    assert!(tooltip.contains("server running"), "{tooltip}");
}

/// The longest server line is short: the version is capped where it is read.
#[test]
fn the_server_line_stays_short_with_the_longest_version() {
    let longest = "1".repeat(32);
    let body = format!(r#"{{"status":"ok","version":"{longest}","service":"rDownloader"}}"#);
    let read = health_version(body.as_bytes());
    assert_eq!(read.as_deref(), Some(longest.as_str()));
    let line = server_label(ServerStatus::Unreachable, read.as_deref());
    assert!(line.chars().count() <= 64, "{line}");
}

/// Only rDownloader's own health answer gives a version, and only one that looks like one.
#[test]
fn the_version_is_read_only_from_a_plausible_rdownloader_answer() {
    let read = |body: &str| health_version(body.as_bytes());
    assert_eq!(
        read(r#"{"status":"ok","version":"1.24.0","service":"rDownloader"}"#).as_deref(),
        Some("1.24.0")
    );
    assert_eq!(
        read(r#"{"version":"2.0.0-rc.1+build5","service":"rDownloader"}"#).as_deref(),
        Some("2.0.0-rc.1+build5")
    );
    for refused in [
        r#"{"status":"ok","version":"1.24.0","service":"something else"}"#,
        r#"{"status":"ok","service":"rDownloader"}"#,
        r#"{"version":"","service":"rDownloader"}"#,
        r#"{"version":"1.24.0 — evil","service":"rDownloader"}"#,
        r#"{"version":"1.24.0\nInstall now","service":"rDownloader"}"#,
        r#"{"version":7,"service":"rDownloader"}"#,
        "not json",
    ] {
        assert_eq!(read(refused), None, "{refused}");
    }
    let too_long = format!(
        r#"{{"version":"{}","service":"rDownloader"}}"#,
        "1".repeat(33)
    );
    assert_eq!(read(&too_long), None);
}
