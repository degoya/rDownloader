use std::ffi::OsString;
use std::path::PathBuf;

use chrono::Utc;

use super::{
    Environment, RestartHow, RestartPlan, SYSTEMD_ENV, Supervisor, decide_how, systemd_from,
};
use crate::InstallKind;

fn environment(systemd: bool, relaunchable: bool) -> Environment {
    Environment {
        systemd,
        relaunchable,
    }
}

/// A container restarts through its runtime, systemd through the unit, everything else through
/// the relauncher -- and a start that cannot be repeated by hand only.
#[test]
fn the_way_follows_the_install_kind_and_the_environment() {
    for kind in [
        InstallKind::Portable,
        InstallKind::Msi,
        InstallKind::Unknown,
    ] {
        assert_eq!(
            decide_how(kind, environment(false, true)),
            (RestartHow::Relaunch, None),
            "{kind:?}"
        );
    }
    for kind in [
        InstallKind::Deb,
        InstallKind::Rpm,
        InstallKind::Aur,
        InstallKind::Portable,
    ] {
        assert_eq!(
            decide_how(kind, environment(true, true)),
            (RestartHow::Supervisor, Some(Supervisor::Systemd)),
            "{kind:?}"
        );
    }
    assert_eq!(
        decide_how(InstallKind::Docker, environment(true, true)),
        (RestartHow::Supervisor, Some(Supervisor::Container))
    );
    assert_eq!(
        decide_how(InstallKind::Docker, environment(false, false)),
        (RestartHow::Supervisor, Some(Supervisor::Container))
    );
    assert_eq!(
        decide_how(InstallKind::Deb, environment(false, false)),
        (RestartHow::Manual, None)
    );
    assert_eq!(RestartHow::Relaunch.as_str(), "self");
    assert_eq!(
        serde_json::to_value(RestartHow::Relaunch).expect("serialize"),
        serde_json::json!("self")
    );
}

/// systemd sets `INVOCATION_ID` for every unit it starts; an empty one names nothing.
#[test]
fn systemd_is_read_from_its_invocation_id() {
    let with = |value: Option<&str>| {
        systemd_from(|name| {
            assert_eq!(name, SYSTEMD_ENV);
            value.map(OsString::from)
        })
    };
    assert!(with(Some("8d6c6c2c0f6a4e3e9d6e2f0f3b1a7c55")));
    assert!(!with(Some("")));
    assert!(!with(None));
}

fn plan(root: &std::path::Path) -> RestartPlan {
    RestartPlan {
        version: "1.24.0".to_owned(),
        executable: root.join("rdownloader"),
        data_dir: root.join("data"),
        service_pid: 4242,
        service_args: vec!["serve".to_owned()],
        service_cwd: root.to_path_buf(),
        health_timeout_secs: super::DEFAULT_HEALTH_TIMEOUT_SECS,
        requested_at: Utc::now(),
    }
}

/// The plan goes to `<data>/update/restart.json` and comes back as written.
#[test]
fn the_plan_is_written_where_the_relauncher_reads_it() {
    let root = tempfile::tempdir().expect("tempdir");
    let plan = plan(root.path());
    assert_eq!(plan.validate().map_err(|error| error.detail), Ok(()));
    assert_eq!(plan.image(), "rdownloader");
    let path = plan.write().expect("write");
    assert_eq!(
        path,
        root.path().join("data").join("update").join("restart.json")
    );
    assert_eq!(RestartPlan::read(&path).expect("read"), plan);
}

/// A relative path, a version that is no plain version or no program refuse the plan.
#[test]
fn a_plan_the_relauncher_must_not_act_on_is_refused() {
    let root = tempfile::tempdir().expect("tempdir");
    let cases = [
        RestartPlan {
            service_cwd: PathBuf::from("relative"),
            ..plan(root.path())
        },
        RestartPlan {
            version: "../1.0".to_owned(),
            ..plan(root.path())
        },
        RestartPlan {
            executable: PathBuf::from("/"),
            ..plan(root.path())
        },
    ];
    for case in cases {
        let refused = case.validate().expect_err("refused");
        assert_eq!(refused.code, "restart.plan_invalid", "{}", refused.detail);
    }
}
