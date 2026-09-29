//! rclone as a plain file store: one file up, a rename, a listing, one file down, one delete
//! (RD-160-02).
//!
//! The full backup's rclone destination is built on this, and it runs rclone the way the upload
//! step does ([`crate::rclone_job`]): the same lookup (the configured path, the vendor folder,
//! `PATH`), the upload limit as `--bwlimit` in bytes, every flag before a `--` and the paths
//! after it, so a remote or a file beginning with `-` is never read as a flag. Unlike the step
//! it works on single files in one remote folder — `copyto`, `moveto`, `lsjson`, `deletefile` —
//! because an archive is one file and the folder may hold anything else.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Stdio,
};

use crate::rclone_job::bwlimit;

/// How many trailing lines of rclone's output a failure keeps.
const TAIL_LINES: usize = 20;

/// Why an rclone call did not do what it was asked.
#[derive(Debug)]
pub enum RcloneFailure {
    /// No rclone binary was found in the settings, the vendor folder or `PATH`.
    Missing,
    /// rclone reported the file or the folder as not there (exit status 3 or 4).
    NotFound,
    /// rclone could not be started.
    Spawn(String),
    /// Any other exit, with the last lines rclone wrote.
    Failed { status: Option<i32>, output: String },
    /// `lsjson` answered with something that is not its JSON.
    Unreadable(String),
}

impl std::fmt::Display for RcloneFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing => {
                formatter.write_str("rclone not found (settings, vendor folder or PATH)")
            }
            Self::NotFound => formatter.write_str("rclone found no such file or folder"),
            Self::Spawn(error) => write!(formatter, "rclone could not be started: {error}"),
            Self::Failed { status, output } => write!(
                formatter,
                "rclone exit status {}\n{output}",
                status.unwrap_or(-1)
            ),
            Self::Unreadable(error) => {
                write!(formatter, "rclone lsjson answered unreadably: {error}")
            }
        }
    }
}

impl std::error::Error for RcloneFailure {}

/// One file directly in the remote folder.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RcloneEntry {
    pub name: String,
    pub size: u64,
}

/// One folder of an rclone remote, `remote:path`.
#[derive(Clone, Debug)]
pub struct RcloneRemote {
    tool: PathBuf,
    remote: String,
    /// The upload limit in bytes per second when the remote was opened; `None` = unlimited.
    bwlimit: Option<u64>,
}

impl RcloneRemote {
    /// A remote folder served by the rclone binary at `tool`.
    #[must_use]
    pub fn new(tool: PathBuf, remote: &str, bwlimit: Option<u64>) -> Self {
        Self {
            tool,
            remote: remote.trim().to_owned(),
            bwlimit,
        }
    }

    /// Finds rclone the way the upload step does and opens `remote` with it.
    ///
    /// # Errors
    ///
    /// [`RcloneFailure::Missing`] when there is no rclone to run.
    pub fn locate(
        executable: Option<&str>,
        vendor_directory: Option<&str>,
        remote: &str,
        bwlimit: Option<u64>,
    ) -> Result<Self, RcloneFailure> {
        let tool = rd_core::locate_tool(executable, vendor_directory, "rclone")
            .ok_or(RcloneFailure::Missing)?;
        Ok(Self::new(tool.path, remote, bwlimit))
    }

    /// `remote:path` as configured.
    #[must_use]
    pub fn remote(&self) -> &str {
        &self.remote
    }

    /// The remote path of `name` in the folder. A remote that ends in its colon (`gdrive:`) is
    /// its root, where a slash would name a different place on some backends.
    #[must_use]
    pub fn path_of(&self, name: &str) -> String {
        if self.remote.ends_with(':') {
            format!("{}{name}", self.remote)
        } else {
            format!("{}/{name}", self.remote.trim_end_matches('/'))
        }
    }

    /// Copies the local file to `name` in the folder.
    ///
    /// # Errors
    ///
    /// When rclone is not there or does not succeed.
    pub async fn upload(&self, local: &Path, name: &str) -> Result<(), RcloneFailure> {
        let arguments = self.arguments(
            "copyto",
            true,
            &[local.as_os_str().to_owned(), self.path_of(name).into()],
        );
        self.run(arguments).await.map(drop)
    }

    /// Renames `from` to `to` inside the folder.
    ///
    /// # Errors
    ///
    /// When rclone is not there or does not succeed.
    pub async fn rename(&self, from: &str, to: &str) -> Result<(), RcloneFailure> {
        let arguments = self.arguments(
            "moveto",
            false,
            &[self.path_of(from).into(), self.path_of(to).into()],
        );
        self.run(arguments).await.map(drop)
    }

    /// Copies `name` from the folder to the local path.
    ///
    /// # Errors
    ///
    /// [`RcloneFailure::NotFound`] when there is no such file.
    pub async fn download(&self, name: &str, local: &Path) -> Result<(), RcloneFailure> {
        let arguments = self.arguments(
            "copyto",
            false,
            &[self.path_of(name).into(), local.as_os_str().to_owned()],
        );
        self.run(arguments).await.map(drop)
    }

    /// Deletes `name` from the folder.
    ///
    /// # Errors
    ///
    /// [`RcloneFailure::NotFound`] when there is no such file.
    pub async fn delete(&self, name: &str) -> Result<(), RcloneFailure> {
        let arguments = self.arguments("deletefile", false, &[self.path_of(name).into()]);
        self.run(arguments).await.map(drop)
    }

    /// The files directly in the folder; a folder that does not exist yet is empty.
    ///
    /// # Errors
    ///
    /// When rclone is not there, does not succeed or answers with something unreadable.
    pub async fn list(&self) -> Result<Vec<RcloneEntry>, RcloneFailure> {
        let arguments = self.arguments("lsjson", false, &[self.remote.clone().into()]);
        match self.run(arguments).await {
            Ok(stdout) => parse_lsjson(&stdout),
            Err(RcloneFailure::NotFound) => Ok(Vec::new()),
            Err(other) => Err(other),
        }
    }

    /// The arguments of one call: the verb, its flags, `--`, the paths.
    fn arguments(&self, verb: &str, upload: bool, paths: &[OsString]) -> Vec<OsString> {
        let mut arguments: Vec<OsString> = vec![verb.into()];
        if verb == "lsjson" {
            arguments.extend(["--files-only", "--no-mimetype", "--no-modtime"].map(OsString::from));
        }
        if upload && let Some(rate) = bwlimit(self.bwlimit) {
            arguments.push("--bwlimit".into());
            arguments.push(rate.into());
        }
        arguments.push("--".into());
        arguments.extend(paths.iter().cloned());
        arguments
    }

    /// Runs rclone and returns what it wrote to standard output.
    async fn run(&self, arguments: Vec<OsString>) -> Result<Vec<u8>, RcloneFailure> {
        let mut command = tokio::process::Command::new(&self.tool);
        command
            .args(arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(0x0800_0000);
        let output = command
            .output()
            .await
            .map_err(|error| RcloneFailure::Spawn(error.to_string()))?;
        match output.status.code() {
            Some(0) => Ok(output.stdout),
            // rclone's exit statuses: 3 is a directory, 4 a file that is not there.
            Some(3 | 4) => Err(RcloneFailure::NotFound),
            status => Err(RcloneFailure::Failed {
                status,
                output: tail(&output.stderr),
            }),
        }
    }
}

/// The last lines of rclone's diagnostics.
fn tail(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(TAIL_LINES)..].join("\n")
}

/// The files of an `lsjson` answer.
pub(crate) fn parse_lsjson(stdout: &[u8]) -> Result<Vec<RcloneEntry>, RcloneFailure> {
    let entries: Vec<serde_json::Value> = serde_json::from_slice(stdout)
        .map_err(|error| RcloneFailure::Unreadable(error.to_string()))?;
    Ok(entries
        .into_iter()
        .filter(|entry| {
            !entry
                .get("IsDir")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
        })
        .filter_map(|entry| {
            Some(RcloneEntry {
                name: entry.get("Name")?.as_str()?.to_owned(),
                size: entry
                    .get("Size")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0),
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{RcloneEntry, RcloneRemote, parse_lsjson};

    fn strings(arguments: &[std::ffi::OsString]) -> Vec<String> {
        arguments
            .iter()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn a_name_joins_the_folder_and_a_bare_remote_is_its_root() {
        let folder = RcloneRemote::new(PathBuf::from("rclone"), "nas:backups/", None);
        assert_eq!(folder.path_of("a.rdbackup"), "nas:backups/a.rdbackup");
        let root = RcloneRemote::new(PathBuf::from("rclone"), "gdrive:", None);
        assert_eq!(root.path_of("a.rdbackup"), "gdrive:a.rdbackup");
    }

    #[test]
    fn only_the_upload_carries_the_limit_and_every_path_follows_the_separator() {
        let remote = RcloneRemote::new(PathBuf::from("rclone"), "-odd:folder", Some(2_000_000));
        let upload = strings(&remote.arguments(
            "copyto",
            true,
            &[
                Path::new("-staged").as_os_str().to_owned(),
                remote.path_of("a.rdbackup").into(),
            ],
        ));
        assert_eq!(
            upload,
            [
                "copyto",
                "--bwlimit",
                "2000000B",
                "--",
                "-staged",
                "-odd:folder/a.rdbackup"
            ]
        );
        let delete = strings(&remote.arguments("deletefile", false, &[remote.path_of("a").into()]));
        assert_eq!(delete, ["deletefile", "--", "-odd:folder/a"]);
        let list = strings(&remote.arguments("lsjson", false, &[remote.remote().into()]));
        assert_eq!(
            list,
            [
                "lsjson",
                "--files-only",
                "--no-mimetype",
                "--no-modtime",
                "--",
                "-odd:folder"
            ]
        );
    }

    #[test]
    fn a_listing_keeps_the_files_and_refuses_what_is_not_json() {
        let listed = parse_lsjson(
            br#"[{"Path":"a.rdbackup","Name":"a.rdbackup","Size":42,"IsDir":false},
                 {"Path":"sub","Name":"sub","Size":-1,"IsDir":true}]"#,
        )
        .expect("listing");
        assert_eq!(
            listed,
            vec![RcloneEntry {
                name: "a.rdbackup".to_owned(),
                size: 42
            }]
        );
        assert!(parse_lsjson(b"Failed to lsjson").is_err());
    }
}
