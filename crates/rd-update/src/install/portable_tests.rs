use super::*;
use crate::install::fixture::{Fixture, NEW, OLD, write};
use crate::install::steps;

#[test]
fn a_staged_and_switched_update_puts_the_new_files_in_place_and_the_old_ones_aside() {
    let mut fixture = Fixture::tar();
    stage(&mut fixture.journal).expect("stage");
    assert_eq!(fixture.stored().phase, Phase::Staged);
    // The archive's `data/` is never one of the entries.
    assert_eq!(
        fixture.journal.entries,
        [
            "LICENSE",
            "README.md",
            "VERSION.txt",
            "plugins",
            "rdownloader"
        ]
    );
    assert_eq!(
        fixture.journal.replaced,
        ["README.md", "VERSION.txt", "plugins", "rdownloader"]
    );
    // Staging changes nothing live.
    fixture.assert_old_in_place();

    switch(&mut fixture.journal).expect("switch");
    assert_eq!(fixture.stored().phase, Phase::Switched);
    fixture.assert_new();
    assert_eq!(
        std::fs::read_to_string(fixture.data.join("rdownloader.sqlite3")).expect("database"),
        "live database"
    );
    // The staging goes with the archive's `data/`, which the switch never moved.
    assert!(!fixture.journal.staged_dir().exists());
}

#[test]
fn a_windows_zip_switches_the_same_way() {
    let mut fixture = Fixture::zip();
    stage(&mut fixture.journal).expect("stage");
    assert!(
        fixture
            .journal
            .entries
            .contains(&"rdownloader.exe".to_owned())
    );
    switch(&mut fixture.journal).expect("switch");
    fixture.assert_new();
}

#[test]
fn a_complete_switch_is_taken_back_to_exactly_the_old_files() {
    let mut fixture = Fixture::tar();
    stage(&mut fixture.journal).expect("stage");
    switch(&mut fixture.journal).expect("switch");
    roll_back(&fixture.journal).expect("roll back");
    fixture.assert_old();
    assert!(!fixture.journal.failed_dir().exists());
}

#[test]
fn a_roll_back_before_anything_moved_changes_nothing_live() {
    let mut fixture = Fixture::zip();
    stage(&mut fixture.journal).expect("stage");
    roll_back(&fixture.journal).expect("roll back");
    fixture.assert_old();
}

#[test]
fn a_switch_run_again_finishes_one_that_stopped_halfway() {
    let mut fixture = Fixture::tar();
    stage(&mut fixture.journal).expect("stage");
    // What a stop after the first entry left: its old copy aside, its new one still staged.
    let previous = fixture.journal.previous_dir();
    std::fs::create_dir_all(&previous).expect("previous");
    std::fs::rename(
        fixture.install.join("README.md"),
        previous.join("README.md"),
    )
    .expect("aside");
    fixture
        .journal
        .advance(Phase::Switching)
        .expect("switching");
    switch(&mut fixture.journal).expect("switch again");
    fixture.assert_new();
    assert_eq!(fixture.read("README.md").as_deref(), Some("new readme"));
}

#[test]
fn a_half_done_switch_is_taken_back_from_where_it_stopped() {
    let mut fixture = Fixture::tar();
    stage(&mut fixture.journal).expect("stage");
    let previous = fixture.journal.previous_dir();
    let staged = fixture.journal.staged_dir();
    std::fs::create_dir_all(&previous).expect("previous");
    // LICENSE is new and placed, README.md replaced, VERSION.txt set aside only.
    std::fs::rename(staged.join("LICENSE"), fixture.install.join("LICENSE")).expect("license");
    std::fs::rename(
        fixture.install.join("README.md"),
        previous.join("README.md"),
    )
    .expect("aside");
    std::fs::rename(staged.join("README.md"), fixture.install.join("README.md")).expect("place");
    std::fs::rename(
        fixture.install.join("VERSION.txt"),
        previous.join("VERSION.txt"),
    )
    .expect("aside");
    roll_back(&fixture.journal).expect("roll back");
    fixture.assert_old();
}

#[test]
fn an_archive_without_the_program_is_refused_before_anything_moves() {
    let mut fixture = Fixture::tar();
    let broken = fixture
        .install
        .parent()
        .expect("root")
        .join("broken.tar.gz");
    {
        let file = std::fs::File::create(&broken).expect("archive");
        let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
            file,
            flate2::Compression::fast(),
        ));
        let mut header = tar::Header::new_gnu();
        header.set_size(3);
        header.set_mode(0o644);
        builder
            .append_data(&mut header, "README.md", &b"new"[..])
            .expect("entry");
        builder.into_inner().expect("tar").finish().expect("gzip");
    }
    fixture.use_artifact(broken);
    let error = stage(&mut fixture.journal).expect_err("no program");
    assert_eq!(
        error.downcast_ref::<InstallError>().map(|error| error.code),
        Some("update.archive_incomplete")
    );
    fixture.assert_old_in_place();
}

#[test]
fn a_file_that_is_no_archive_is_refused() {
    let mut fixture = Fixture::tar();
    let other = fixture
        .install
        .parent()
        .expect("root")
        .join("rdownloader.msi");
    write(&other, "not an archive");
    fixture.use_artifact(other);
    let error = stage(&mut fixture.journal).expect_err("unknown");
    assert_eq!(
        error.downcast_ref::<InstallError>().map(|error| error.code),
        Some("update.archive_unknown")
    );
}

#[test]
fn the_database_copy_goes_back_in_place_and_the_live_files_aside() {
    let fixture = Fixture::tar();
    assert!(steps::restore_database(&fixture.journal.plan).expect("restore"));
    assert_eq!(
        std::fs::read_to_string(fixture.data.join("rdownloader.sqlite3")).expect("database"),
        "database before the update"
    );
    assert!(!fixture.data.join("rdownloader.sqlite3-wal").exists());
    let aside = fixture.data.join("update").join("replaced-database");
    assert_eq!(
        std::fs::read_to_string(aside.join("rdownloader.sqlite3")).expect("aside"),
        "live database"
    );
    assert_eq!(
        std::fs::read_to_string(aside.join("rdownloader.sqlite3-wal")).expect("aside"),
        "live journal"
    );
    // Again, as after a stop in the middle: the same result.
    assert!(steps::restore_database(&fixture.journal.plan).expect("again"));
    assert_eq!(
        std::fs::read_to_string(fixture.data.join("rdownloader.sqlite3")).expect("database"),
        "database before the update"
    );
}

#[test]
fn the_artifact_is_checked_again_before_it_is_used() {
    let mut fixture = Fixture::tar();
    steps::verify_artifact(&fixture.journal.plan).expect("as downloaded");
    fixture.journal.plan.sha256 = "00".repeat(32);
    assert_eq!(
        steps::verify_artifact(&fixture.journal.plan)
            .expect_err("changed")
            .code,
        "update.digest_mismatch"
    );
    fixture.journal.plan.artifact = fixture.install.join("gone.tar.gz");
    assert_eq!(
        steps::verify_artifact(&fixture.journal.plan)
            .expect_err("gone")
            .code,
        "update.download_failed"
    );
}

/// Finding 6 of the 2026-09-30 review: the switch hashed one opening and unpacked the path
/// again, so an archive put in its place between the two was installed. The stage now hashes
/// what it unpacks: another archive under the checked name is refused before anything is
/// unpacked.
#[test]
fn an_archive_swapped_in_after_the_check_is_not_unpacked() {
    let mut fixture = Fixture::tar();
    steps::verify_artifact(&fixture.journal.plan).expect("as downloaded");
    let artifact = fixture.journal.plan.artifact.clone();
    {
        let file = std::fs::File::create(&artifact).expect("swap");
        let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
            file,
            flate2::Compression::fast(),
        ));
        let mut header = tar::Header::new_gnu();
        header.set_size(4);
        header.set_mode(0o755);
        builder
            .append_data(&mut header, &fixture.journal.plan.executable, &b"evil"[..])
            .expect("entry");
        builder.into_inner().expect("tar").finish().expect("gzip");
    }
    let error = stage(&mut fixture.journal).expect_err("swapped");
    assert_eq!(
        error.downcast_ref::<InstallError>().map(|error| error.code),
        Some("update.digest_mismatch")
    );
    assert!(!fixture.journal.staged_dir().exists());
    assert_eq!(fixture.stored().phase, Phase::Handed);
    let executable = fixture.journal.plan.executable.clone();
    assert_eq!(fixture.read(&executable).as_deref(), Some("old program"));
    fixture.assert_old_in_place();
}

/// The handle the stage and the installer use is the one that was hashed, at its start.
#[test]
fn the_checked_handle_reads_the_whole_checked_file() {
    use std::io::Read as _;
    let fixture = Fixture::tar();
    let mut handle = steps::open_artifact(&fixture.journal.plan).expect("checked");
    let mut bytes = Vec::new();
    handle.read_to_end(&mut bytes).expect("read");
    assert_eq!(
        bytes,
        std::fs::read(&fixture.journal.plan.artifact).expect("file")
    );
}

/// On Windows the checked file cannot be changed, renamed or removed while its handle is open.
#[cfg(windows)]
#[test]
fn the_checked_file_is_locked_while_its_handle_is_open() {
    let fixture = Fixture::zip();
    let artifact = fixture.journal.plan.artifact.clone();
    let handle = steps::open_artifact(&fixture.journal.plan).expect("checked");
    assert!(
        std::fs::OpenOptions::new()
            .write(true)
            .open(&artifact)
            .is_err()
    );
    assert!(std::fs::rename(&artifact, artifact.with_extension("moved")).is_err());
    assert!(std::fs::remove_file(&artifact).is_err());
    assert!(
        std::fs::File::open(&artifact).is_ok(),
        "reading stays possible"
    );
    drop(handle);
    std::fs::OpenOptions::new()
        .write(true)
        .open(&artifact)
        .expect("free again");
}

/// Finding 6: the kept installer of the previous version is installed on a rollback only while
/// it is the package the update that kept it checked.
#[test]
fn the_kept_installer_is_checked_before_a_rollback_uses_it() {
    let mut fixture = Fixture::zip();
    let msi = fixture._root.path().join("rdownloader-windows-x86_64.msi");
    write(&msi, "the installer of 2.0.0");
    fixture.use_artifact(msi);
    fixture.journal.plan.kind = crate::InstallKind::Msi;
    let sha256 = fixture.journal.plan.sha256.clone();
    steps::keep_installer(&fixture.journal).expect("keep");
    let kept = steps::kept_installer(&fixture.data, NEW).expect("kept");
    assert_eq!(
        steps::kept_installer_sha256(&kept).as_deref(),
        Some(sha256.as_str())
    );

    let mut plan = fixture.journal.plan.clone();
    plan.previous_installer = Some(kept.clone());
    plan.previous_installer_sha256 = steps::kept_installer_sha256(&kept);
    plan.validate().expect("valid");
    let (path, handle) = steps::previous_installer(&plan)
        .expect("checked")
        .expect("available");
    assert_eq!(path, kept);
    drop(handle);

    write(&kept, "a planted installer");
    assert_eq!(
        steps::previous_installer(&plan).expect_err("changed").code,
        "update.digest_mismatch"
    );
    plan.previous_installer_sha256 = None;
    assert!(steps::previous_installer(&plan).expect("none").is_none());
}

#[test]
fn versions_name_the_folders() {
    let fixture = Fixture::tar();
    assert!(
        fixture
            .journal
            .staged_dir()
            .ends_with(format!(".update-{NEW}"))
    );
    assert!(
        fixture
            .journal
            .failed_dir()
            .ends_with(format!(".failed-{NEW}"))
    );
    assert_eq!(fixture.journal.plan.from_version, OLD);
}

impl Fixture {
    /// The old files, without asking that the staging is gone.
    fn assert_old_in_place(&self) {
        assert_eq!(self.read("README.md").as_deref(), Some("old readme"));
        assert_eq!(self.read("VERSION.txt").as_deref(), Some(OLD));
        assert!(self.read("LICENSE").is_none());
        self.assert_data_untouched();
    }
}
