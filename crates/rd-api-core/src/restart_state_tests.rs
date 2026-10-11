use std::sync::Arc;

use rd_update::restart::{Environment, RestartHow, RestartPlan, Supervisor};
use rd_update::{InstallKind, RESTART_EXIT_CODE};

use super::RestartState;
use crate::dto::RestartReason;

fn reason(code: &str, plugin: &str) -> RestartReason {
    RestartReason::plugin(code, plugin, Some("Example"), None)
}

/// A fresh state -- the one a restart leaves -- has nothing pending and no restart under way.
#[test]
fn a_start_begins_with_nothing_pending() {
    let state = RestartState::default();
    assert!(state.recorded().is_empty());
    assert!(!state.restarting());
    assert_eq!(state.exit_code(), None);
}

/// The same reason is kept once; switching a plugin back replaces the earlier reason.
#[test]
fn a_reason_is_kept_once_and_an_undone_one_is_replaced() {
    let state = RestartState::default();
    state.record(reason("plugin_disabled", "a"));
    state.record(reason("plugin_disabled", "a"));
    state.record(reason("plugin_removed", "b"));
    assert_eq!(
        state.recorded(),
        vec![
            reason("plugin_disabled", "a"),
            reason("plugin_removed", "b")
        ]
    );
    state.record(reason("plugin_enabled", "a"));
    assert_eq!(
        state.recorded(),
        vec![reason("plugin_removed", "b"), reason("plugin_enabled", "a")]
    );
}

/// One restart at a time; one that could not start is taken back.
#[test]
fn a_restart_begins_once() {
    let state = RestartState::default();
    assert!(state.begin(RestartHow::Relaunch));
    assert!(state.restarting());
    assert!(!state.begin(RestartHow::Supervisor));
    assert_eq!(
        state.exit_code(),
        None,
        "the relauncher's restart ends as any stop"
    );
    state.abandon();
    assert!(!state.restarting());
}

/// A restart left to the supervisor, or to a person, ends the service with exit code 75.
#[test]
fn a_supervised_restart_ends_with_its_exit_code() {
    for how in [RestartHow::Supervisor, RestartHow::Manual] {
        let state = RestartState::default();
        assert!(state.begin(how));
        assert_eq!(state.exit_code(), Some(RESTART_EXIT_CODE), "{how:?}");
    }
    assert_eq!(RESTART_EXIT_CODE, 75);
}

/// The way follows the environment the state was given.
#[test]
fn the_way_follows_the_environment() {
    let state = RestartState::default();
    let never: super::Relauncher =
        Arc::new(|_: &RestartPlan| -> anyhow::Result<()> { anyhow::bail!("no process in a test") });
    state.use_environment(
        Environment {
            systemd: true,
            relaunchable: true,
        },
        Arc::clone(&never),
    );
    assert_eq!(
        state.how(InstallKind::Deb),
        (RestartHow::Supervisor, Some(Supervisor::Systemd))
    );
    state.use_environment(
        Environment {
            systemd: false,
            relaunchable: true,
        },
        never,
    );
    assert_eq!(
        state.how(InstallKind::Portable),
        (RestartHow::Relaunch, None)
    );
    assert_eq!(
        state.how(InstallKind::Docker),
        (RestartHow::Supervisor, Some(Supervisor::Container))
    );
}

/// What the start found already is no reason a restart would remove; only the first baseline
/// counts.
#[test]
fn the_baseline_of_the_start_is_kept_once() {
    let state = RestartState::default();
    let broken = reason("plugin_installed", "broken");
    assert!(!state.in_baseline(&broken));
    state.remember_baseline(vec![broken.clone()]);
    state.remember_baseline(Vec::new());
    assert!(state.in_baseline(&broken));
    assert!(!state.in_baseline(&reason("plugin_installed", "other")));
}
