//! A portable installation and its next version on disk, for the install tests.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::{Journal, Plan};
use crate::InstallKind;

pub(crate) const OLD: &str = "1.0.0";
pub(crate) const NEW: &str = "2.0.0";

/// `root/install` with the old program, its data beside it, and the new version's archive.
pub(crate) struct Fixture {
    pub _root: tempfile::TempDir,
    pub install: PathBuf,
    pub data: PathBuf,
    pub journal: Journal,
}

impl Fixture {
    /// A Linux-style installation updated from a `.tar.gz`.
    pub(crate) fn tar() -> Self {
        Self::new("rdownloader", "rdownloader-linux-x86_64.tar.gz")
    }

    /// A Windows-style installation updated from a `.zip`.
    pub(crate) fn zip() -> Self {
        Self::new("rdownloader.exe", "rdownloader-windows-x86_64.zip")
    }

    fn new(executable: &str, artifact_name: &str) -> Self {
        let root = tempfile::tempdir().expect("tempdir");
        let install = root.path().join("install");
        let data = install.join("data");
        write(&install.join(executable), "old program");
        write(&install.join("VERSION.txt"), OLD);
        write(&install.join("README.md"), "old readme");
        write(&install.join("plugins").join("old.rdplug"), "old plugin");
        write(&data.join("rdownloader.sqlite3"), "live database");
        write(&data.join("rdownloader.sqlite3-wal"), "live journal");
        write(&install.join("downloads").join("film.mkv"), "payload");
        write(
            &data.join("pre-update").join("copy.sqlite3"),
            "database before the update",
        );
        let artifact = root.path().join(artifact_name);
        let files: Vec<(String, &str)> = vec![
            (executable.to_owned(), "new program"),
            ("VERSION.txt".to_owned(), NEW),
            ("README.md".to_owned(), "new readme"),
            ("LICENSE".to_owned(), "licence"),
            ("plugins/new.rdplug".to_owned(), "new plugin"),
            // Never moved, whatever an archive carries.
            (
                "data/rdownloader.sqlite3".to_owned(),
                "a database from the archive",
            ),
        ];
        if artifact_name.ends_with(".zip") {
            zip_archive(&artifact, &files);
        } else {
            tar_archive(&artifact, &files);
        }
        let bytes = fs::read(&artifact).expect("artifact");
        let plan = Plan {
            kind: InstallKind::Portable,
            from_version: OLD.to_owned(),
            target_version: NEW.to_owned(),
            artifact,
            sha256: hex::encode(Sha256::digest(&bytes)),
            size: bytes.len() as u64,
            install_dir: install.clone(),
            executable: executable.to_owned(),
            data_dir: data.clone(),
            database: data.join("rdownloader.sqlite3"),
            database_copy: Some(data.join("pre-update").join("copy.sqlite3")),
            service_pid: 1,
            service_args: vec!["serve".to_owned()],
            service_cwd: install.clone(),
            health_timeout_secs: 90,
            previous_installer: None,
            previous_installer_sha256: None,
        };
        let mut journal = Journal::begin(plan);
        journal.write().expect("journal");
        Self {
            _root: root,
            install,
            data,
            journal,
        }
    }

    /// Makes `artifact` the plan's download, with the digest and size it has.
    pub(crate) fn use_artifact(&mut self, artifact: PathBuf) {
        let bytes = fs::read(&artifact).expect("artifact");
        self.journal.plan.sha256 = hex::encode(Sha256::digest(&bytes));
        self.journal.plan.size = bytes.len() as u64;
        self.journal.plan.artifact = artifact;
    }

    pub(crate) fn read(&self, relative: &str) -> Option<String> {
        fs::read_to_string(self.install.join(relative)).ok()
    }

    pub(crate) fn stored(&self) -> Journal {
        Journal::read(&self.data)
            .expect("journal")
            .expect("written")
    }

    pub(crate) fn executable(&self) -> PathBuf {
        self.journal.executable()
    }

    /// Everything the old version had, and nothing the new one brought.
    pub(crate) fn assert_old(&self) {
        let executable = self.journal.plan.executable.clone();
        assert_eq!(self.read(&executable).as_deref(), Some("old program"));
        assert_eq!(self.read("VERSION.txt").as_deref(), Some(OLD));
        assert_eq!(self.read("README.md").as_deref(), Some("old readme"));
        assert_eq!(
            self.read("plugins/old.rdplug").as_deref(),
            Some("old plugin")
        );
        assert!(self.read("plugins/new.rdplug").is_none());
        assert!(self.read("LICENSE").is_none());
        assert!(!self.journal.previous_dir().exists());
        assert!(!self.journal.staged_dir().exists());
        self.assert_data_untouched();
    }

    /// Everything the new version brought, the old one aside.
    pub(crate) fn assert_new(&self) {
        let executable = self.journal.plan.executable.clone();
        assert_eq!(self.read(&executable).as_deref(), Some("new program"));
        assert_eq!(self.read("VERSION.txt").as_deref(), Some(NEW));
        assert_eq!(self.read("LICENSE").as_deref(), Some("licence"));
        assert_eq!(
            self.read("plugins/new.rdplug").as_deref(),
            Some("new plugin")
        );
        assert!(self.read("plugins/old.rdplug").is_none());
        assert_eq!(
            fs::read_to_string(self.journal.previous_dir().join(&executable))
                .ok()
                .as_deref(),
            Some("old program")
        );
        self.assert_data_untouched();
    }

    pub(crate) fn assert_data_untouched(&self) {
        assert_eq!(self.read("downloads/film.mkv").as_deref(), Some("payload"));
        assert!(self.data.join("rdownloader.sqlite3").is_file());
    }
}

pub(crate) fn write(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("folder");
    }
    fs::write(path, text).expect("write");
}

/// `previous/locked/rdownloader-capture` in a folder that refuses removals; `None` (and the
/// test is moot) where permissions do not hold, as for root.
#[cfg(unix)]
pub(crate) fn locked_folder(previous: &Path) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt as _;
    let locked = previous.join("locked");
    write(&locked.join("rdownloader-capture"), "agent");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o555)).expect("lock");
    if fs::write(locked.join("probe"), "").is_ok() {
        eprintln!("permissions do not hold for this user; nothing to prove");
        unlock(&locked);
        return None;
    }
    Some(locked)
}

#[cfg(unix)]
pub(crate) fn unlock(folder: &Path) {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(folder, fs::Permissions::from_mode(0o755)).expect("unlock");
}

fn tar_archive(path: &Path, files: &[(String, &str)]) {
    let file = fs::File::create(path).expect("archive");
    let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::fast());
    let mut builder = tar::Builder::new(encoder);
    for (name, text) in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(text.len() as u64);
        header.set_mode(0o755);
        builder
            .append_data(&mut header, name, text.as_bytes())
            .expect("entry");
    }
    builder
        .into_inner()
        .expect("tar")
        .finish()
        .expect("gzip")
        .flush()
        .expect("flush");
}

fn zip_archive(path: &Path, files: &[(String, &str)]) {
    let file = fs::File::create(path).expect("archive");
    let mut writer = zip::ZipWriter::new(file);
    for (name, text) in files {
        writer
            .start_file(name.as_str(), zip::write::SimpleFileOptions::default())
            .expect("entry");
        writer.write_all(text.as_bytes()).expect("write");
    }
    writer.finish().expect("zip");
}
