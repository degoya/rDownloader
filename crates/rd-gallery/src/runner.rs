//! Queue runner: downloads one whole gallery with `gallery-dl` into a subfolder of the
//! package destination. Progress is bytes/files seen so far; the total is unknown upfront.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::Result;
use async_trait::async_trait;
use rd_core::{DownloadFile, DownloadKind, DownloadPackage, Failure, FailureKind};
use rd_db::Database;
use rd_scheduler::{ExternalRunner, RunOutcome, ToolNetwork, ToolNetworkSource};
use rd_tools::{
    LiveSlots, ProgressThrottle, ToolLine, ToolProcess,
    process::{Stdout, prepare},
};
use tokio_util::sync::CancellationToken;

use crate::SharedGallerySettings;

/// Downloads `DownloadKind::Gallery` files.
pub struct GalleryRunner {
    database: Database,
    settings: SharedGallerySettings,
    /// Read on every dispatch pass, so a changed setting needs no restart (audit 1.9.1, TR-07).
    slots: LiveSlots<rd_core::GallerySettings>,
    /// The proxy and CA gallery-dl is started with (RD-1240-08); none without it, as in tests.
    network: Option<ToolNetworkSource>,
}

impl GalleryRunner {
    #[must_use]
    pub fn new(database: Database, settings: SharedGallerySettings) -> Self {
        let slots = LiveSlots::new(Arc::clone(&settings), |settings| {
            settings.gallery_max_parallel
        });
        Self {
            database,
            settings,
            slots,
            network: None,
        }
    }

    /// Hands gallery-dl the download's proxy and the custom CA (RD-1240-08).
    #[must_use]
    pub fn with_tool_network(mut self, network: ToolNetworkSource) -> Self {
        self.network = Some(network);
        self
    }
}

/// gallery-dl's arguments for one gallery.
///
/// The proxy goes as `--proxy` when it carries no credentials. One with credentials reaches
/// gallery-dl through its environment only, which `proxy-env` tells it to read; an argument
/// list is readable by every process on the machine. A custom CA is its `verify` file.
pub(crate) fn gallery_args(
    target: &Path,
    source: &str,
    limit_rate: Option<u64>,
    network: &ToolNetwork,
) -> Vec<OsString> {
    let mut args: Vec<OsString> = Vec::new();
    let mut push = |value: OsString| args.push(value);
    // gallery-dl fetches over its own sockets, so the limit goes to the process. It takes one
    // rate for the whole job; see the bandwidth capability matrix.
    if let Some(rate) = limit_rate {
        push(OsString::from("--limit-rate"));
        push(OsString::from(rate.to_string()));
    }
    if let Some(proxy) = network.proxy_argument() {
        push(OsString::from("--proxy"));
        push(OsString::from(proxy));
    } else if network.proxy_in_environment_only() {
        push(OsString::from("-o"));
        push(OsString::from("proxy-env=true"));
    }
    if let Some(bundle) = network.trust_bundle() {
        let mut option = OsString::from("verify=");
        option.push(bundle);
        push(OsString::from("-o"));
        push(option);
    }
    push(OsString::from("-D"));
    push(target.as_os_str().to_owned());
    push(OsString::from("--"));
    push(OsString::from(source));
    args
}

/// Folder name of the gallery below the package destination.
pub(crate) fn gallery_folder(file_name: &str) -> String {
    let stem = std::path::Path::new(file_name)
        .file_stem()
        .and_then(|value| value.to_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("gallery");
    rd_files::sanitize_file_name(stem)
}

/// The folder a gallery download stores into, claimed for it alone (RD-1240-28).
///
/// Two packages of the same name share their destination, so two galleries of the same name
/// met in one folder: gallery-dl skipped the file the other one had stored there, and the new
/// download stood as finished with nothing transferred. A download that has not run before
/// takes its folder only while nothing is in it, and otherwise the next free `name (n)` -- the
/// collision rule a file meets -- which the caller records as its name, so a later attempt
/// comes back to it. One that ran before keeps its folder: what is there is its own, and
/// gallery-dl skipping it is what adopts it.
pub(crate) fn claim_folder(destination: &Path, folder: &str, ran_before: bool) -> String {
    let target = destination.join(folder);
    let occupied = match std::fs::read_dir(&target) {
        Ok(mut entries) => entries.next().is_some(),
        Err(_) => target.exists(),
    };
    if ran_before || !occupied {
        return folder.to_owned();
    }
    rd_files::collision_free_path(destination, folder)
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .map_or_else(|| folder.to_owned(), str::to_owned)
}

/// Whether an earlier attempt of `file` ran: a retry, or progress it stored.
fn ran_before(file: &DownloadFile) -> bool {
    file.retry_count > 0 || file.committed_bytes.get() > 0
}

/// Maps a gallery-dl failure (exit status + stderr tail) onto the retry policy.
pub(crate) fn map_gallery_error(stderr: &str) -> Failure {
    let lower = stderr.to_ascii_lowercase();
    if lower.contains("unsupported url") {
        return Failure::coded(
            FailureKind::Unsupported,
            "gallery.unsupported_url",
            "gallery-dl does not support this URL",
        );
    }
    // Before the site's sign-in: a proxy's "407 Proxy Authentication Required" says
    // "authentication" too, and sent the person to gallery-dl.conf for a password the proxy
    // profile holds (RD-1240-28).
    if let Some(failure) = rd_scheduler::proxy_auth_failed(stderr) {
        return failure;
    }
    if lower.contains("authentication") || lower.contains("login required") {
        return Failure::coded(
            FailureKind::Permanent,
            "gallery.auth_required",
            "This gallery requires authentication (configure it in gallery-dl.conf)",
        );
    }
    // Re-runs are cheap: gallery-dl skips files that already exist.
    rd_tools::tool_failed("gallery.tool_failed", stderr, "gallery-dl failed")
}

/// A run its deadline ended: the tool failed and is retried, where a stop read as the
/// person's own pause and was never tried again (re-audit 1.9.1, RA-TR-03).
fn timed_out(stderr: &str) -> Failure {
    rd_tools::tool_failed(
        "gallery.tool_failed",
        stderr,
        "gallery-dl went silent past its time limit",
    )
}

#[async_trait]
impl ExternalRunner for GalleryRunner {
    fn kind(&self) -> DownloadKind {
        DownloadKind::Gallery
    }

    /// gallery-dl skips files that already exist, so a re-run adopts what an earlier one stored;
    /// nothing is resumed mid-file and nothing is verified against a digest.
    fn reuse(&self) -> rd_core::ReuseCapability {
        rd_core::ReuseCapability {
            resume_partial: false,
            recheck_partial: false,
            adopt_completed: true,
            verify_completed: false,
            applies_collision_policy: false,
        }
    }

    fn slot_capacity(&self) -> usize {
        self.slots.get()
    }

    async fn run(
        &self,
        file: &DownloadFile,
        package: &DownloadPackage,
        cancellation: CancellationToken,
        limits: rd_scheduler::RunLimits,
    ) -> Result<RunOutcome> {
        // A proxy that cannot be used is a failure, never a direct connection.
        let network = match &self.network {
            Some(source) => match source.for_file(file).await {
                Ok(network) => network,
                Err(failure) => return Ok(RunOutcome::Failed(failure)),
            },
            None => ToolNetwork::direct(),
        };
        let settings = self.settings.read().await.clone();
        // Leased before the version is assessed, and only gallery downloads stop when
        // gallery-dl is too old or listed as broken; both rules live in `prepare`.
        let tool = match prepare(
            "gallery-dl",
            rd_core::locate_tool_leased(
                settings.gallery_executable.as_deref(),
                settings.vendor_directory.as_deref(),
                "gallery-dl",
            )
            .map(|(tool, lease)| (tool.path, lease)),
            rd_tools::Capability::GalleryDownload,
        )
        .await
        {
            Ok(tool) => tool,
            Err(failure) => return Ok(RunOutcome::Failed(failure)),
        };
        let named = gallery_folder(&file.file_name);
        let folder = claim_folder(Path::new(&package.destination), &named, ran_before(file));
        if folder != named {
            self.database
                .set_download_file_name(file.id, folder.clone())
                .await?;
        }
        let target = PathBuf::from(&package.destination).join(&folder);
        tokio::fs::create_dir_all(&target).await?;
        let mut command = tokio::process::Command::new(tool.path());
        network.apply(&mut command);
        command.args(gallery_args(
            &target,
            file.source.as_str(),
            limits
                .bandwidth
                .binding_limit()
                .map(|binding| binding.bytes_per_second),
            &network,
        ));
        let mut process = ToolProcess::spawn(&mut command, "gallery-dl", Stdout::Read)?
            .with_silence_limit(rd_tools::SILENCE_LIMIT);
        // gallery-dl prints one path per stored file ("# path" for skipped ones). Sizes are
        // summed from disk; totals stay unknown, so the UI shows plain byte progress.
        let mut committed: u64 = 0;
        let mut throttle = ProgressThrottle::default();
        loop {
            let line = match process.next_line(cancellation.cancelled()).await? {
                ToolLine::Line(line) => line,
                ToolLine::End => break,
                ToolLine::Stopped => return Ok(RunOutcome::Stopped),
                // No deadline: a gallery runs as long as its files take. Only the silence
                // limit ends it, when gallery-dl has stored nothing for that long.
                ToolLine::TimedOut => {
                    return Ok(RunOutcome::Failed(timed_out(&process.stderr().await)));
                }
            };
            let path = line.trim().trim_start_matches("# ").trim();
            if path.is_empty() {
                continue;
            }
            if let Ok(meta) = tokio::fs::metadata(path).await {
                committed += meta.len();
            }
            if throttle.due() {
                let _ = self
                    .database
                    .set_download_progress(file.id, committed, None)
                    .await;
                throttle.mark();
            }
        }
        let status = process.wait().await?;
        let stderr_text = process.stderr().await;
        if !status.success() {
            return Ok(RunOutcome::Failed(
                network
                    .unsupported_proxy("gallery-dl", &stderr_text)
                    .unwrap_or_else(|| map_gallery_error(&stderr_text)),
            ));
        }
        let _ = self
            .database
            .set_download_progress(file.id, committed, Some(committed))
            .await;
        Ok(RunOutcome::Completed { final_name: folder })
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{claim_folder, gallery_args, gallery_folder, map_gallery_error, timed_out};
    use rd_core::{FailureKind, ProxyKind};
    use rd_scheduler::{ToolNetwork, ToolProxy};
    use secrecy::SecretString;

    fn proxy(password: Option<&str>) -> ToolNetwork {
        let password = password.map(|password| SecretString::from(password.to_owned()));
        ToolNetwork::with_proxy(
            ToolProxy::new(
                ProxyKind::Socks5,
                "socks5://proxy.example:1080".parse().expect("endpoint"),
                password.as_ref().map(|_| "alice"),
                password.as_ref(),
            )
            .expect("proxy"),
        )
    }

    fn args(network: &ToolNetwork) -> Vec<String> {
        gallery_args(
            Path::new("/downloads/set"),
            "https://gallery.example/set/1",
            None,
            network,
        )
        .into_iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect()
    }

    /// RD-1240-08: the download's proxy is gallery-dl's own option, and without one there is
    /// none.
    #[test]
    fn the_proxy_is_on_the_command_line_only_when_there_is_one() {
        assert_eq!(
            args(&proxy(None)),
            [
                "--proxy",
                "socks5://proxy.example:1080",
                "-D",
                "/downloads/set",
                "--",
                "https://gallery.example/set/1"
            ]
        );
        let direct = args(&ToolNetwork::direct());
        assert!(
            !direct.iter().any(|arg| arg.contains("proxy")),
            "{direct:?}"
        );
    }

    /// Credentials never reach the argument list; gallery-dl reads them from its environment.
    #[test]
    fn proxy_credentials_stay_off_the_command_line() {
        let args = args(&proxy(Some("pr0xy-secret")));
        assert!(!args.iter().any(|arg| arg.contains("pr0xy")), "{args:?}");
        assert!(!args.iter().any(|arg| arg == "--proxy"), "{args:?}");
        assert!(
            args.windows(2)
                .any(|pair| pair == ["-o".to_owned(), "proxy-env=true".to_owned()]),
            "{args:?}"
        );
    }

    #[test]
    fn a_proxy_gallery_dl_cannot_speak_fails_with_a_code() {
        let stderr = "requests.exceptions.InvalidSchema: Missing dependencies for SOCKS support.";
        let failure = proxy(None)
            .unsupported_proxy("gallery-dl", stderr)
            .expect("failure");
        assert_eq!(failure.code.as_deref(), Some("proxy.unsupported_by_tool"));
        assert!(!failure.category.is_retryable());
    }

    /// RA-TR-03: a deadline is a failure with a retry, not a stop.
    #[test]
    fn a_run_past_its_deadline_is_a_retryable_failure() {
        let failure = timed_out("");
        assert_eq!(failure.code.as_deref(), Some("gallery.tool_failed"));
        assert!(failure.category.is_retryable());
        assert!(
            failure.message.contains("time limit"),
            "{}",
            failure.message
        );
    }

    /// RD-1240-28: a second gallery of the same name met the first one's folder, gallery-dl
    /// skipped the file there, and the download finished with nothing transferred.
    #[test]
    fn a_new_gallery_never_shares_another_one_s_folder() {
        let destination = tempfile::tempdir().expect("tempdir");
        let root = destination.path();
        // Nothing there yet, or only an empty folder: the download's own name.
        assert_eq!(claim_folder(root, "picture", false), "picture");
        std::fs::create_dir(root.join("picture")).expect("folder");
        assert_eq!(claim_folder(root, "picture", false), "picture");
        // Another download's file in it: the next free name.
        std::fs::write(root.join("picture").join("direct-picture.png"), b"png").expect("file");
        assert_eq!(claim_folder(root, "picture", false), "picture (1)");
        // An earlier attempt of the same download: its own files, adopted.
        assert_eq!(claim_folder(root, "picture", true), "picture");
        // The claimed name read back as the folder, so a later attempt finds it again.
        assert_eq!(gallery_folder("picture (1)"), "picture (1)");
    }

    #[test]
    fn folder_name_is_sanitized_and_never_empty() {
        assert_eq!(gallery_folder("artworks"), "artworks");
        assert_eq!(gallery_folder("set: one?"), "set_ one_");
        assert_eq!(gallery_folder(""), "gallery");
    }

    #[test]
    fn errors_map_to_retry_policy() {
        assert!(matches!(
            map_gallery_error("error: Unsupported URL 'https://x'").category,
            FailureKind::Unsupported
        ));
        assert!(matches!(
            map_gallery_error("error: Login required").category,
            FailureKind::Permanent
        ));
        assert!(matches!(
            map_gallery_error("HTTPError 503").category,
            FailureKind::Transient { .. }
        ));
    }

    /// RD-1240-28: gallery-dl's words for a proxy that refused its password, from the live test
    /// of 1.24, are the proxy's failure, not a sign-in the gallery asks for.
    #[test]
    fn a_proxy_refusing_its_password_is_a_proxy_failure() {
        let stderr = "[downloader.http][error] ProxyError: ('Unable to connect to proxy', \
            OSError('Tunnel connection failed: 407 Proxy Authentication Required'))";
        let failure = map_gallery_error(stderr);
        assert_eq!(failure.code.as_deref(), Some("proxy.auth_failed"));
        assert!(matches!(failure.category, FailureKind::Permanent));
        assert_eq!(
            map_gallery_error("[twitter][error] AuthenticationError: login required")
                .code
                .as_deref(),
            Some("gallery.auth_required")
        );
    }
}
