//! `update.after_previous_set_aside`, `update.after_new_placed` and `update.before_health_check`
//! (RD-180-02, recovery matrix): the portable switch stops after an old entry went aside, after a
//! new one took its place, and after the switch was recorded but before anyone proved the new
//! version. The next start never runs a mix of both versions.
#![cfg(feature = "failpoints")]

use std::fs;
use std::path::{Path, PathBuf};

use rd_core::failpoint::FailpointGuard;
use rd_update::InstallKind;
use rd_update::install::recover::{Recovery, confirm_started, recover_at_start};
use rd_update::install::{Journal, Phase, Plan, portable};
use sha2::{Digest, Sha256};

const OLD: &str = "1.0.0";
const NEW: &str = "2.0.0";
const ENTRIES: [(&str, &str, &str); 3] = [
    ("README.md", "old readme", "new readme"),
    ("VERSION.txt", OLD, NEW),
    ("rdownloader", "old program", "new program"),
];

struct Installation {
    _root: tempfile::TempDir,
    install: PathBuf,
    data: PathBuf,
    journal: Journal,
}

fn installation() -> Installation {
    let root = tempfile::tempdir().expect("tempdir");
    let install = root.path().join("install");
    let data = install.join("data");
    fs::create_dir_all(data.join("pre-update")).expect("data");
    fs::write(data.join("rdownloader.sqlite3"), "live database").expect("database");
    fs::write(
        data.join("pre-update").join("copy.sqlite3"),
        "database before the update",
    )
    .expect("copy");
    let artifact = root.path().join("rdownloader-linux-x86_64.tar.gz");
    {
        let file = fs::File::create(&artifact).expect("archive");
        let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
            file,
            flate2::Compression::fast(),
        ));
        for (name, old, new) in ENTRIES {
            fs::write(install.join(name), old).expect("old entry");
            let mut header = tar::Header::new_gnu();
            header.set_size(new.len() as u64);
            header.set_mode(0o755);
            builder
                .append_data(&mut header, name, new.as_bytes())
                .expect("new entry");
        }
        builder.into_inner().expect("tar").finish().expect("gzip");
    }
    let bytes = fs::read(&artifact).expect("artifact");
    let mut journal = Journal::begin(Plan {
        kind: InstallKind::Portable,
        from_version: OLD.to_owned(),
        target_version: NEW.to_owned(),
        sha256: hex::encode(Sha256::digest(&bytes)),
        size: bytes.len() as u64,
        artifact,
        install_dir: install.clone(),
        executable: "rdownloader".to_owned(),
        data_dir: data.clone(),
        database: data.join("rdownloader.sqlite3"),
        database_copy: Some(data.join("pre-update").join("copy.sqlite3")),
        service_pid: 1,
        service_args: vec!["serve".to_owned()],
        service_cwd: install.clone(),
        health_timeout_secs: 90,
        previous_installer: None,
        previous_installer_sha256: None,
    });
    journal.write().expect("journal");
    portable::stage(&mut journal).expect("stage");
    Installation {
        _root: root,
        install,
        data,
        journal,
    }
}

impl Installation {
    fn contents(&self) -> Vec<String> {
        ENTRIES
            .iter()
            .map(|(name, _, _)| fs::read_to_string(self.install.join(name)).unwrap_or_default())
            .collect()
    }

    fn assert_version(&self, old: bool) {
        let expected: Vec<String> = ENTRIES
            .iter()
            .map(|(_, before, after)| (if old { before } else { after }).to_string())
            .collect();
        assert_eq!(self.contents(), expected);
    }

    fn executable(&self) -> PathBuf {
        self.install.join("rdownloader")
    }

    fn phase(&self) -> Phase {
        Journal::read(&self.data)
            .expect("journal")
            .expect("written")
            .phase
    }
}

fn crash_in_switch(point: &str) -> Installation {
    let mut installation = installation();
    let guard = FailpointGuard::once(point);
    assert!(portable::switch(&mut installation.journal).is_err());
    assert!(guard.fired(), "{point} was never reached");
    installation
}

fn database(data: &Path) -> String {
    fs::read_to_string(data.join("rdownloader.sqlite3")).expect("database")
}

#[test]
fn a_switch_stopped_after_an_entry_went_aside_is_taken_back_by_the_next_start() {
    let installation = crash_in_switch("update.after_previous_set_aside");
    assert_eq!(installation.phase(), Phase::Switching);
    // Whichever program the next start runs, the files end as the old version.
    let outcome =
        recover_at_start(&installation.data, &installation.executable(), OLD).expect("start");
    assert_eq!(outcome, Recovery::Continue);
    installation.assert_version(true);
    assert_eq!(installation.phase(), Phase::RolledBack);
    assert_eq!(database(&installation.data), "live database");
}

#[test]
fn a_switch_stopped_after_a_new_entry_took_its_place_is_taken_back_by_the_next_start() {
    let installation = crash_in_switch("update.after_new_placed");
    assert_eq!(installation.phase(), Phase::Switching);
    // The first entry is new, the rest old: a mix no start may run.
    assert_eq!(installation.contents(), ["new readme", OLD, "old program"]);
    let outcome =
        recover_at_start(&installation.data, &installation.executable(), NEW).expect("start");
    assert_eq!(outcome, Recovery::Restart(installation.executable()));
    installation.assert_version(true);
    assert!(!installation.install.join(".previous").exists());
    assert_eq!(database(&installation.data), "live database");
}

#[test]
fn a_switch_recorded_but_never_proven_is_proven_by_a_start_that_answers() {
    let installation = crash_in_switch("update.before_health_check");
    assert_eq!(installation.phase(), Phase::Switched);
    installation.assert_version(false);
    let outcome =
        recover_at_start(&installation.data, &installation.executable(), NEW).expect("start");
    assert_eq!(outcome, Recovery::Continue);
    assert!(confirm_started(&installation.data, NEW).expect("confirm"));
    assert_eq!(installation.phase(), Phase::Verified);
    installation.assert_version(false);
}

#[test]
fn a_switch_recorded_but_never_proven_is_taken_back_when_its_start_never_answered() {
    let installation = crash_in_switch("update.before_health_check");
    recover_at_start(&installation.data, &installation.executable(), NEW).expect("first");
    let outcome =
        recover_at_start(&installation.data, &installation.executable(), NEW).expect("second");
    assert_eq!(outcome, Recovery::Restart(installation.executable()));
    installation.assert_version(true);
    assert_eq!(installation.phase(), Phase::RolledBack);
    // The new version ran and may have migrated: the copy from before the update is back.
    assert_eq!(database(&installation.data), "database before the update");
}
