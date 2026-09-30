use super::*;
use crate::install::fixture::Fixture;

/// Changes one field of a valid journal into something it must refuse.
type Tamper = fn(&mut Journal, &Path);

#[test]
fn a_plan_the_updater_must_not_act_on_is_refused() {
    let fixture = Fixture::tar();
    let plan = fixture.journal.plan.clone();
    plan.validate().expect("the fixture's plan");

    let refused = |change: &dyn Fn(&mut Plan)| {
        let mut plan = plan.clone();
        change(&mut plan);
        plan.validate().expect_err("refused").code
    };
    assert_eq!(
        refused(&|plan| plan.kind = InstallKind::Homebrew),
        "update.plan_invalid"
    );
    assert_eq!(
        refused(&|plan| plan.target_version = "../2.0.0".to_owned()),
        "update.plan_invalid"
    );
    assert_eq!(
        refused(&|plan| plan.executable = "bin/rdownloader".to_owned()),
        "update.plan_invalid"
    );
    assert_eq!(
        refused(&|plan| plan.install_dir = PathBuf::from("relative")),
        "update.plan_invalid"
    );
}

#[test]
fn plain_versions_are_safe_folder_names() {
    for good in ["1.8.0", "1.8.0-beta.2", "1.8.0+build.5", "v1.8.0"] {
        assert!(is_plain_version(good), "{good}");
    }
    for bad in ["", "..", ".1", "1.8.0/..", r"1\8", "1.8 0", &"9".repeat(65)] {
        assert!(!is_plain_version(bad), "{bad}");
    }
}

#[test]
fn the_journal_is_written_whole_and_read_back() {
    let mut fixture = Fixture::tar();
    fixture.journal.entries = vec!["rdownloader".to_owned()];
    fixture
        .journal
        .end(Phase::RolledBack, "update.health_timeout", "no answer")
        .expect("end");
    let stored = fixture.stored();
    assert_eq!(stored.phase, Phase::RolledBack);
    assert_eq!(stored.reason.as_deref(), Some("update.health_timeout"));
    assert_eq!(stored.entries, ["rdownloader"]);
    assert!(stored.phase.is_terminal());
    let update = update_dir(&fixture.data);
    let leftovers: Vec<_> = std::fs::read_dir(&update)
        .expect("list")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".tmp"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[test]
fn an_unreadable_journal_is_an_error_not_an_empty_one() {
    let directory = tempfile::tempdir().expect("tempdir");
    assert!(Journal::read(directory.path()).expect("absent").is_none());
    let path = Journal::path(directory.path());
    std::fs::create_dir_all(path.parent().expect("parent")).expect("folder");
    std::fs::write(&path, b"{ not json").expect("write");
    assert!(Journal::read(directory.path()).is_err());
}

/// Finding 5 of the 2026-09-30 review: the start acted on whatever the journal said — entries
/// that climb out of the program folder, a database copy from anywhere, a program elsewhere.
#[test]
fn a_journal_that_reaches_outside_its_installation_is_moved_aside_unread() {
    let cases: [(&str, Tamper); 13] = [
        ("an entry that climbs out", |journal, _| {
            journal.entries = vec!["../outside".to_owned()];
        }),
        ("an absolute entry", |journal, _| {
            let absolute = if cfg!(windows) {
                r"C:\outside"
            } else {
                "/outside"
            };
            journal.entries = vec![absolute.to_owned()];
        }),
        ("a nested entry", |journal, _| {
            journal.entries = vec!["plugins/x".to_owned()];
        }),
        ("a kept folder as entry", |journal, _| {
            journal.entries = vec!["data".to_owned()];
        }),
        ("a replaced name that is no entry", |journal, _| {
            journal.replaced = vec!["README.md".to_owned()];
        }),
        ("a replaced name that climbs out", |journal, _| {
            journal.entries = vec!["..".to_owned()];
            journal.replaced = vec!["..".to_owned()];
        }),
        ("an executable in another folder", |journal, _| {
            journal.plan.executable = "../evil".to_owned();
        }),
        ("a drive-relative executable", |journal, _| {
            journal.plan.executable = "C:evil.exe".to_owned();
        }),
        ("a database copy outside pre-update", |journal, root| {
            journal.plan.database_copy = Some(root.join("planted.sqlite3"));
        }),
        (
            "a database copy that climbs out of pre-update",
            |journal, _| {
                let data = journal.plan.data_dir.clone();
                journal.plan.database_copy = Some(
                    data.join("pre-update")
                        .join("..")
                        .join("..")
                        .join("planted.sqlite3"),
                );
            },
        ),
        ("a database elsewhere", |journal, root| {
            journal.plan.database = root.join("other.sqlite3");
        }),
        ("another data directory", |journal, root| {
            journal.plan.data_dir = root.join("elsewhere");
            journal.plan.database = root.join("elsewhere").join("rdownloader.sqlite3");
            journal.plan.database_copy = None;
        }),
        ("a previous installer elsewhere", |journal, root| {
            journal.plan.previous_installer = Some(root.join("evil.msi"));
        }),
    ];
    for (case, change) in cases {
        let fixture = Fixture::tar();
        let mut journal = fixture.journal.clone();
        change(&mut journal, fixture._root.path());
        let path = Journal::path(&fixture.data);
        std::fs::write(&path, serde_json::to_vec(&journal).expect("json")).expect("write");
        assert!(journal.check(&fixture.data).is_err(), "{case}");
        assert!(
            Journal::read(&fixture.data).expect("read").is_none(),
            "{case}"
        );
        assert!(!path.exists(), "{case}");
        assert!(
            update_dir(&fixture.data).join(REJECTED_FILE).is_file(),
            "{case}"
        );
    }
    let fixture = Fixture::tar();
    fixture
        .journal
        .check(&fixture.data)
        .expect("the fixture's journal");
}

/// The same crafted journal in the middle of a switch: the start's roll-back would have moved a
/// file beside the program folder.
#[test]
fn the_start_does_not_roll_back_a_journal_that_climbs_out() {
    let fixture = Fixture::tar();
    let victim = fixture._root.path().join("victim");
    std::fs::write(&victim, "not the program's").expect("victim");
    let mut journal = fixture.journal.clone();
    journal.phase = Phase::Switching;
    journal.entries = vec!["../victim".to_owned()];
    std::fs::write(
        Journal::path(&fixture.data),
        serde_json::to_vec(&journal).expect("json"),
    )
    .expect("write");
    let outcome = recover::recover_at_start(
        &fixture.data,
        &fixture.executable(),
        crate::install::fixture::NEW,
    )
    .expect("start");
    assert_eq!(outcome, recover::Recovery::Continue);
    assert_eq!(
        std::fs::read_to_string(&victim).expect("still there"),
        "not the program's"
    );
    fixture.assert_old();
}

#[test]
fn only_one_updater_holds_the_lock_and_its_end_releases_it() {
    let directory = tempfile::tempdir().expect("tempdir");
    assert!(!updater_running(directory.path()));
    let lock = UpdaterLock::acquire(directory.path())
        .expect("open")
        .expect("free");
    assert!(updater_running(directory.path()));
    assert!(
        UpdaterLock::acquire(directory.path())
            .expect("open")
            .is_none()
    );
    drop(lock);
    assert!(!updater_running(directory.path()));
}

#[test]
fn the_preflight_asks_for_a_writable_folder_and_twice_the_artifact() {
    let directory = tempfile::tempdir().expect("tempdir");
    preflight(directory.path(), 1024).expect("room for a small one");
    assert_eq!(
        preflight(directory.path(), u64::MAX / 2)
            .expect_err("too large")
            .code,
        "update.not_enough_space"
    );
    assert_eq!(
        preflight(&directory.path().join("missing"), 1)
            .expect_err("no folder")
            .code,
        "update.install_dir_not_writable"
    );
    assert!(!directory.path().join(".update-probe").exists());
}

#[test]
fn phases_have_stable_names() {
    assert_eq!(Phase::RollingBack.as_str(), "rolling_back");
    assert_eq!(
        serde_json::to_value(Phase::RolledBack).expect("json"),
        "rolled_back"
    );
    assert!(!Phase::Switched.is_terminal());
}

#[test]
fn msiexec_runs_silently_with_a_log() {
    let args = process::msiexec_arguments(
        "/i",
        Path::new(r"C:\Users\me\AppData\Local\rDownloader\data\update\download\rdownloader.msi"),
        Path::new(r"C:\log.txt"),
        &[],
    );
    assert_eq!(args[0], "/i");
    assert!(args.contains(&"/qn".to_owned()));
    assert!(args.contains(&"/norestart".to_owned()));
    assert_eq!(args[args.len() - 2], "/l*v");
}

/// Finding 2 of the 2026-09-30 review: a bare `msiexec.exe` is looked up in the program folder
/// first on Windows.
#[test]
fn msiexec_is_started_by_its_full_path() {
    let found = process::system_program(Some(r"D:\Win".into()), "msiexec.exe");
    assert!(found.is_absolute() || !cfg!(windows), "{}", found.display());
    assert!(found.starts_with(r"D:\Win"), "{}", found.display());
    assert!(found.ends_with("msiexec.exe"));
    for unknown in [None, Some(std::ffi::OsString::new())] {
        let fallback = process::system_program(unknown, "msiexec.exe");
        assert!(
            fallback.starts_with(r"C:\Windows"),
            "{}",
            fallback.display()
        );
    }
    assert_ne!(process::msiexec_path(), Path::new("msiexec.exe"));
    assert!(process::msiexec_path().ends_with("msiexec.exe"));
}
