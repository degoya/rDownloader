use super::*;
use crate::install::fixture::{Fixture, NEW, OLD};
use crate::install::{UpdaterLock, portable};

#[test]
fn a_start_without_a_journal_just_starts() {
    let directory = tempfile::tempdir().expect("tempdir");
    let executable = directory.path().join("rdownloader");
    assert_eq!(
        recover_at_start(directory.path(), &executable, OLD).expect("start"),
        Recovery::Continue
    );
}

#[test]
fn an_updater_that_ended_before_changing_anything_leaves_a_failed_record() {
    let fixture = Fixture::tar();
    let outcome = recover_at_start(&fixture.data, &fixture.executable(), OLD).expect("start");
    assert_eq!(outcome, Recovery::Continue);
    let journal = fixture.stored();
    assert_eq!(journal.phase, Phase::Failed);
    assert_eq!(journal.reason.as_deref(), Some("update.interrupted"));
    fixture.assert_old();
}

#[test]
fn a_half_switch_is_taken_back_and_the_new_program_restarts_as_the_old_one() {
    let mut fixture = Fixture::tar();
    portable::stage(&mut fixture.journal).expect("stage");
    let previous = fixture.journal.previous_dir();
    std::fs::create_dir_all(&previous).expect("previous");
    let executable = fixture.journal.plan.executable.clone();
    std::fs::rename(
        fixture.install.join(&executable),
        previous.join(&executable),
    )
    .expect("aside");
    std::fs::rename(
        fixture.journal.staged_dir().join(&executable),
        fixture.install.join(&executable),
    )
    .expect("place");
    fixture
        .journal
        .advance(Phase::Switching)
        .expect("switching");

    // The new program was started by hand while the old files are half replaced.
    let outcome = recover_at_start(&fixture.data, &fixture.executable(), NEW).expect("start");
    assert_eq!(outcome, Recovery::Restart(fixture.executable()));
    fixture.assert_old();
    let journal = fixture.stored();
    assert_eq!(journal.phase, Phase::RolledBack);
    assert_eq!(journal.reason.as_deref(), Some("update.interrupted"));
    // The new version never ran: the live database stays.
    assert_eq!(
        std::fs::read_to_string(fixture.data.join("rdownloader.sqlite3")).expect("database"),
        "live database"
    );
}

#[test]
fn the_old_program_taking_back_a_half_switch_just_goes_on() {
    let mut fixture = Fixture::zip();
    portable::stage(&mut fixture.journal).expect("stage");
    fixture
        .journal
        .advance(Phase::Switching)
        .expect("switching");
    let outcome = recover_at_start(&fixture.data, &fixture.executable(), OLD).expect("start");
    assert_eq!(outcome, Recovery::Continue);
    fixture.assert_old();
}

#[test]
fn an_unproven_switch_is_proven_by_the_first_start_that_answers() {
    let mut fixture = Fixture::tar();
    portable::stage(&mut fixture.journal).expect("stage");
    portable::switch(&mut fixture.journal).expect("switch");
    let outcome = recover_at_start(&fixture.data, &fixture.executable(), NEW).expect("start");
    assert_eq!(outcome, Recovery::Continue);
    assert_eq!(fixture.stored().start_attempts, 1);
    assert!(confirm_started(&fixture.data, NEW).expect("confirm"));
    assert_eq!(fixture.stored().phase, Phase::Verified);
    fixture.assert_new();

    // The next start of the proven version removes what the update left.
    recover_at_start(&fixture.data, &fixture.executable(), NEW).expect("next start");
    assert!(!fixture.journal.previous_dir().exists());
    assert!(!fixture.journal.staged_dir().exists());
    assert!(fixture.stored().cleaned);
    assert_eq!(fixture.read("VERSION.txt").as_deref(), Some(NEW));
}

#[test]
fn an_unproven_switch_whose_first_start_never_answered_is_taken_back_with_the_database() {
    let mut fixture = Fixture::tar();
    portable::stage(&mut fixture.journal).expect("stage");
    portable::switch(&mut fixture.journal).expect("switch");
    recover_at_start(&fixture.data, &fixture.executable(), NEW).expect("first start");
    // That start ended before it answered; the next one takes the update back.
    let outcome = recover_at_start(&fixture.data, &fixture.executable(), NEW).expect("second");
    assert_eq!(outcome, Recovery::Restart(fixture.executable()));
    fixture.assert_old();
    let journal = fixture.stored();
    assert_eq!(journal.phase, Phase::RolledBack);
    assert_eq!(journal.reason.as_deref(), Some("update.not_confirmed"));
    assert_eq!(
        std::fs::read_to_string(fixture.data.join("rdownloader.sqlite3")).expect("database"),
        "database before the update"
    );
}

#[test]
fn nothing_is_touched_while_the_updater_runs() {
    let mut fixture = Fixture::tar();
    portable::stage(&mut fixture.journal).expect("stage");
    portable::switch(&mut fixture.journal).expect("switch");
    let _lock = UpdaterLock::acquire(&fixture.data)
        .expect("lock")
        .expect("free");
    assert!(crate::install::updater_running(&fixture.data));
    for _ in 0..3 {
        let outcome = recover_at_start(&fixture.data, &fixture.executable(), NEW).expect("start");
        assert_eq!(outcome, Recovery::Continue);
    }
    assert!(!confirm_started(&fixture.data, NEW).expect("confirm"));
    let journal = fixture.stored();
    assert_eq!(journal.phase, Phase::Switched);
    assert_eq!(journal.start_attempts, 0);
}

#[test]
fn another_program_sharing_the_data_leaves_the_update_alone() {
    let fixture = Fixture::tar();
    let elsewhere = fixture.data.join("elsewhere").join("rdownloader");
    let outcome = recover_at_start(&fixture.data, &elsewhere, OLD).expect("start");
    assert_eq!(outcome, Recovery::Continue);
    assert_eq!(fixture.stored().phase, Phase::Handed);
}

#[test]
fn an_installer_interrupted_mid_run_is_read_from_the_version_that_starts() {
    let mut fixture = Fixture::zip();
    fixture.journal.plan.kind = InstallKind::Msi;
    fixture
        .journal
        .advance(Phase::Switching)
        .expect("switching");
    recover_at_start(&fixture.data, &fixture.executable(), NEW).expect("start");
    let journal = fixture.stored();
    assert_eq!(journal.phase, Phase::Switched);
    assert!(confirm_started(&fixture.data, NEW).expect("confirm"));

    let mut other = Fixture::zip();
    other.journal.plan.kind = InstallKind::Msi;
    other.journal.advance(Phase::Switching).expect("switching");
    recover_at_start(&other.data, &other.executable(), OLD).expect("start");
    let journal = other.stored();
    assert_eq!(journal.phase, Phase::Failed);
    assert_eq!(journal.reason.as_deref(), Some("update.interrupted"));
}

#[test]
fn a_failed_roll_back_keeps_the_files_the_manual_recovery_needs() {
    let mut fixture = Fixture::tar();
    portable::stage(&mut fixture.journal).expect("stage");
    portable::switch(&mut fixture.journal).expect("switch");
    fixture
        .journal
        .end(Phase::Failed, "update.rollback_failed", "the test says so")
        .expect("end");
    recover_at_start(&fixture.data, &fixture.executable(), NEW).expect("start");
    assert!(fixture.journal.previous_dir().exists());
    assert!(!fixture.stored().cleaned);
}
