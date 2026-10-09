//! The agent's own update on disk (RD-1210-03): a portable agent folder replaced by its next
//! version and taken back when the new version does not start in time, a version that is not
//! newer refused before anything moves, and the proof the new agent writes.

use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use sha2::{Digest, Sha256};

use super::*;
use crate::install::fixture::{tar_archive, write};

const OLD: &str = "1.20.0";
const NEW: &str = "1.21.0";

/// A portable agent installed alone, its configuration directory, and its next version's archive.
struct Agent {
    _root: tempfile::TempDir,
    install: PathBuf,
    data: PathBuf,
    executable: PathBuf,
    download: PathBuf,
    artifact: Artifact,
}

impl Agent {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("tempdir");
        let install = root.path().join("agent");
        let data = root.path().join("config");
        let executable = install.join(crate::agent::agent_executable());
        write(&executable, "old agent");
        write(&install.join("VERSION.txt"), OLD);
        write(&install.join("start-capture.sh"), "old launcher");
        write(&install.join("notes.txt"), "the person's own file");
        fs::create_dir_all(&data).expect("config");
        let download = root.path().join("rdownloader-capture-linux-x86_64.tar.gz");
        let files: Vec<(String, &str)> = vec![
            (crate::agent::agent_executable().to_owned(), "new agent"),
            ("VERSION.txt".to_owned(), NEW),
            ("start-capture.sh".to_owned(), "new launcher"),
            ("LICENSE".to_owned(), "licence"),
        ];
        tar_archive(&download, &files);
        let bytes = fs::read(&download).expect("archive");
        let artifact = Artifact {
            platform: "linux".to_owned(),
            arch: "x86_64".to_owned(),
            kind: "archive".to_owned(),
            url: "https://github.com/degoya/rDownloader/releases/download/v1.21.0/rdownloader-capture-linux-x86_64.tar.gz".to_owned(),
            sha256: hex::encode(Sha256::digest(&bytes)),
            size: bytes.len() as u64,
        };
        Self {
            _root: root,
            install,
            data,
            executable,
            download,
            artifact,
        }
    }

    fn journal(&self) -> Journal {
        let plan = plan(
            &self.executable,
            &self.data,
            (OLD, NEW),
            &self.download,
            &self.artifact,
            vec!["run".to_owned()],
        )
        .expect("plan");
        let mut journal = Journal::begin(plan);
        journal.write().expect("journal");
        journal
    }

    fn read(&self, name: &str) -> Option<String> {
        fs::read_to_string(self.install.join(name)).ok()
    }
}

#[test]
fn a_portable_agent_is_replaced_by_its_next_version_once_it_started() {
    let agent = Agent::new();
    let mut journal = agent.journal();
    let mut asked = 0;
    apply(
        &mut journal,
        &mut |journal: &Journal| -> Result<(), InstallError> {
            asked += 1;
            // Asked once the new files are in place.
            assert_eq!(
                fs::read_to_string(journal.executable()).ok().as_deref(),
                Some("new agent")
            );
            Ok(())
        },
    )
    .expect("apply");
    assert_eq!(asked, 1);
    assert_eq!(journal.phase, Phase::Verified);
    assert_eq!(
        agent.read(crate::agent::agent_executable()).as_deref(),
        Some("new agent")
    );
    assert_eq!(agent.read("VERSION.txt").as_deref(), Some(NEW));
    assert_eq!(agent.read("LICENSE").as_deref(), Some("licence"));
    assert_eq!(
        agent.read("notes.txt").as_deref(),
        Some("the person's own file"),
        "only the archive's own entries move"
    );
    assert_eq!(
        fs::read_to_string(
            journal
                .previous_dir()
                .join(crate::agent::agent_executable())
        )
        .ok()
        .as_deref(),
        Some("old agent"),
        "the old version waits aside until the next start"
    );
    let stored = Journal::read(&agent.data).expect("read").expect("written");
    assert_eq!(stored.phase, Phase::Verified);
}

/// The new version never proved it started: the switch is taken back, the old files are where
/// they were, and the journal says why.
#[test]
fn a_new_version_that_does_not_start_in_time_is_taken_back() {
    let agent = Agent::new();
    let mut journal = agent.journal();
    apply(
        &mut journal,
        &mut |journal: &Journal| -> Result<(), InstallError> {
            await_proof(
                &journal.plan.data_dir,
                &journal.plan.target_version,
                (Duration::from_millis(300), Duration::from_millis(50)),
                &mut || true,
            )
        },
    )
    .expect("apply");
    assert_eq!(journal.phase, Phase::RolledBack);
    assert_eq!(journal.reason.as_deref(), Some("update.health_timeout"));
    assert_eq!(
        agent.read(crate::agent::agent_executable()).as_deref(),
        Some("old agent")
    );
    assert_eq!(agent.read("VERSION.txt").as_deref(), Some(OLD));
    assert_eq!(
        agent.read("start-capture.sh").as_deref(),
        Some("old launcher")
    );
    assert!(
        agent.read("LICENSE").is_none(),
        "nothing of the new version stays"
    );
    assert!(!journal.previous_dir().exists());
    assert!(!journal.staged_dir().exists());
}

#[test]
fn a_tampered_download_changes_nothing() {
    let agent = Agent::new();
    let mut journal = agent.journal();
    fs::write(&agent.download, "not the archive the manifest describes").expect("tamper");
    apply(
        &mut journal,
        &mut |_: &Journal| -> Result<(), InstallError> {
            panic!("nothing was switched, so nothing is asked")
        },
    )
    .expect("apply");
    assert_eq!(journal.phase, Phase::Failed);
    assert_eq!(journal.reason.as_deref(), Some("update.digest_mismatch"));
    assert_eq!(
        agent.read(crate::agent::agent_executable()).as_deref(),
        Some("old agent")
    );
}

/// Never a downgrade, and not the version already running, whatever asked for it.
#[test]
fn a_version_that_is_not_newer_is_refused_before_anything_moves() {
    let agent = Agent::new();
    for to in [OLD, "1.19.9", "1.20.0-beta.3"] {
        let refused = plan(
            &agent.executable,
            &agent.data,
            (OLD, to),
            &agent.download,
            &agent.artifact,
            Vec::new(),
        )
        .map(drop);
        let error = refused.expect_err(to);
        assert_eq!(error.code, "update.not_newer", "{to}");
    }
    assert_eq!(
        agent.read(crate::agent::agent_executable()).as_deref(),
        Some("old agent")
    );
}

/// The proof counts only while an update is recorded, and only with the version waited for.
#[test]
fn the_new_agent_proves_its_start_with_its_version() {
    let agent = Agent::new();
    assert!(!started(&agent.data, NEW).expect("no journal"));
    assert_eq!(
        read_proof(&agent.data),
        None,
        "nothing written without an update"
    );
    let journal = agent.journal();
    let wait = (Duration::from_millis(200), Duration::from_millis(20));
    started(&agent.data, OLD).expect("proof");
    let error = await_proof(&agent.data, NEW, wait, &mut || true).expect_err("the old version");
    assert_eq!(error.code, "update.health_timeout");
    let error = await_proof(&agent.data, NEW, wait, &mut || false).expect_err("ended");
    assert_eq!(error.code, "update.new_version_exited");
    started(&agent.data, NEW).expect("proof");
    await_proof(&agent.data, NEW, wait, &mut || true).expect("proved");
    clear_proof(&journal.plan.data_dir);
    assert_eq!(read_proof(&agent.data), None);
}

#[test]
fn a_second_update_is_refused_while_one_is_under_way() {
    let agent = Agent::new();
    let journal = agent.journal();
    let error = hand_over(journal.plan.clone()).expect_err("one is recorded as handed");
    assert_eq!(error.code, "update.in_progress");
}
