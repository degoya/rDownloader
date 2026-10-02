use super::*;
use crate::install::fixture::write;
#[cfg(unix)]
use crate::install::fixture::{locked_folder, unlock};

fn install() -> (tempfile::TempDir, PathBuf) {
    let root = tempfile::tempdir().expect("tempdir");
    let install = root.path().join("install");
    write(&install.join("rdownloader"), "program");
    (root, install)
}

#[test]
fn a_leftover_goes_through_the_trash_and_the_trash_goes_with_it() {
    let (_root, install) = install();
    let previous = install.join(".previous");
    write(&previous.join("rdownloader"), "old program");
    write(&previous.join("plugins").join("old.rdplug"), "old plugin");
    discard(&previous, &install).expect("discard");
    assert!(!previous.exists());
    assert!(!trash_dir(&install).exists());
    assert!(install.join("rdownloader").is_file(), "the program stays");
}

#[test]
fn a_leftover_that_is_not_there_creates_no_trash() {
    let (_root, install) = install();
    discard(&install.join(".previous"), &install).expect("nothing to do");
    assert!(!trash_dir(&install).exists());
    sweep(&install);
}

#[test]
fn a_sweep_removes_what_an_earlier_one_could_not() {
    let (_root, install) = install();
    let left = trash_dir(&install).join("20261002T120000.000000000Z-0");
    write(&left.join(".previous").join("rdownloader-capture"), "agent");
    sweep(&install);
    assert!(!trash_dir(&install).exists());
}

/// The live finding of 2026-10-02 on Linux terms: a leftover that cannot be removed — there a
/// running capture agent's image, here a folder whose entries may not be unlinked — is moved
/// aside instead of failing, and goes with a later sweep once it can.
#[cfg(unix)]
#[test]
fn what_cannot_be_removed_yet_stays_in_the_trash_until_a_later_sweep() {
    let (_root, install) = install();
    let previous = install.join(".previous");
    let Some(locked) = locked_folder(&previous) else {
        return;
    };
    discard(&previous, &install).expect("set aside, not failed");
    assert!(!previous.exists(), "the name is free for the next update");
    let batches: Vec<PathBuf> = fs::read_dir(trash_dir(&install))
        .expect("trash")
        .map(|entry| entry.expect("entry").path())
        .collect();
    assert_eq!(batches.len(), 1, "{batches:?}");
    let agent = batches[0]
        .join(".previous")
        .join(locked.file_name().expect("name"))
        .join("rdownloader-capture");
    assert!(agent.is_file(), "what is in use waits in the trash");

    unlock(agent.parent().expect("folder"));
    sweep(&install);
    assert!(!trash_dir(&install).exists());
}

/// The live finding itself: a program that runs from `.previous/` keeps it from being removed on
/// Windows, and the update goes on anyway. The program is a copy of this test binary waiting in
/// [`waits_for_its_test`].
#[cfg(windows)]
#[test]
fn a_program_running_from_previous_does_not_stop_the_next_update() {
    let (_root, install) = install();
    let previous = install.join(".previous");
    fs::create_dir_all(&previous).expect("previous");
    let agent = previous.join("rdownloader-capture.exe");
    fs::copy(std::env::current_exe().expect("test binary"), &agent).expect("copy");
    let mut running = std::process::Command::new(&agent)
        .args([
            "--exact",
            "install::trash::tests::waits_for_its_test",
            "--ignored",
        ])
        .env(WAIT_VARIABLE, "1")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("start the agent stand-in");
    assert!(
        fs::remove_dir_all(&previous).is_err(),
        "Windows refuses to remove a running program"
    );

    let outcome = discard(&previous, &install);
    let in_trash = fs::read_dir(trash_dir(&install))
        .map(|entries| entries.count())
        .unwrap_or_default();
    let _ = running.kill();
    let _ = running.wait();
    outcome.expect("set aside, not failed");
    assert!(!previous.exists(), "the name is free for the next update");
    assert_eq!(in_trash, 1, "the running program waited in the trash");

    // Once it has ended, a sweep removes it; the system may take a moment to let go of it.
    let started = std::time::Instant::now();
    while trash_dir(&install).exists() && started.elapsed() < std::time::Duration::from_secs(10) {
        sweep(&install);
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    assert!(!trash_dir(&install).exists());
}

#[cfg(windows)]
const WAIT_VARIABLE: &str = "RD_UPDATE_TEST_WAIT";

/// Not a test: the program [`a_program_running_from_previous_does_not_stop_the_next_update`]
/// runs from `.previous/`. Waits only when that test asks it to.
#[cfg(windows)]
#[test]
#[ignore = "started by a_program_running_from_previous_does_not_stop_the_next_update"]
fn waits_for_its_test() {
    if std::env::var_os(WAIT_VARIABLE).is_some() {
        std::thread::sleep(std::time::Duration::from_secs(60));
    }
}
