//! The system checks `rdownloader doctor` prints and the diagnostic bundle carries (RD-110-02).
//!
//! Collected here rather than in `rd-diagnostics` because they read the tool store, the stream
//! runner and the proxy contract, and that crate is a leaf on purpose. `doctor` in the binary
//! and the bundle handler call the same function, so the command and the archive can never
//! disagree about what was found.

use rd_diagnostics::{Check, CheckStatus};

pub use rd_diagnostics::checks::render as render_checks;

/// The settings the checks read; a slice of the settings document, typed so the binary can
/// fill it from its own copy of the blob.
#[derive(Clone, Debug, Default)]
pub struct SystemChecksInput {
    pub vendor_directory: Option<String>,
    pub trusted_proxies: Vec<String>,
    pub external_url: Option<String>,
    pub cookie_security: rd_authn::CookieSecurity,
    /// The data directory whose permissions are checked; `None` checks nothing.
    pub data_directory: Option<std::path::PathBuf>,
    /// `refuse_unknown_archive_tool_version`: an archive tool whose version cannot be read is
    /// refused rather than run with a warning (RD-1420-03).
    pub refuse_unknown_archive_tool_version: bool,
}

const HELPERS: &str = "download helpers";
const PROXY: &str = "Reverse proxy";
const DATA: &str = "Data directory";

/// The helper binaries looked up by name, in the order `doctor` always printed them.
const TOOLS: [&str; 8] = [
    "yt-dlp",
    "ffmpeg",
    "ffprobe",
    "unrar",
    "7z",
    "rclone",
    "gallery-dl",
    "apprise",
];

/// Every check, in print order.
pub async fn system_checks(input: &SystemChecksInput) -> Vec<Check> {
    let mut checks = Vec::new();
    let vendor = input.vendor_directory.as_deref();
    for directory in rd_core::vendor_directories(vendor) {
        checks.push(Check::new(
            HELPERS,
            "vendor path",
            CheckStatus::Info,
            directory.display().to_string(),
        ));
    }
    for tool in TOOLS {
        match rd_core::locate_tool(None, vendor, tool) {
            Some(found) => checks.push(tool_check(tool, &found, input).await),
            None => checks.push(Check::new(HELPERS, tool, CheckStatus::Missing, "not found")),
        }
    }
    // streamlink additionally falls back to the portable build in vendor/streamlink/bin.
    let stream_probe = rd_core::StreamSettings {
        vendor_directory: input.vendor_directory.clone(),
        ..Default::default()
    };
    match rd_stream::locate_streamlink(&stream_probe) {
        Some(found) => checks.push(tool_check("streamlink", &found, input).await),
        None => checks.push(Check::new(
            HELPERS,
            "streamlink",
            CheckStatus::Missing,
            "not found",
        )),
    }
    if rd_core::locate_tool(None, vendor, "ffmpeg").is_some()
        && rd_core::locate_tool(None, vendor, "ffprobe").is_none()
    {
        checks.push(Check::new(
            HELPERS,
            "Note",
            CheckStatus::Warning,
            "yt-dlp needs ffprobe next to ffmpeg to merge video and audio streams.",
        ));
    }
    checks.extend(proxy_checks(input));
    if let Some(directory) = &input.data_directory {
        checks.push(data_directory_check(directory.clone()).await);
    }
    checks
}

/// A found tool: its path and source, the version it reports and what the rules make of it
/// (RD-102-03). The upgrade path is its own note, because that is the line a person reading
/// `doctor` output is looking for, and a version that could not be read has to look different
/// from one that is simply old.
async fn tool_check(tool: &str, found: &rd_core::ResolvedTool, input: &SystemChecksInput) -> Check {
    let assessment = rd_tools::compat::assess(tool, &found.path).await;
    let version = assessment.version.as_deref().unwrap_or("unreadable");
    let mut check = Check::new(
        HELPERS,
        tool,
        CheckStatus::Ok,
        format!("{} ({:?})", found.path.display(), found.source),
    )
    .note(format!("version: {version} - {}", assessment.summary()));
    if let Some(upgrade) = assessment.upgrade() {
        check.status = CheckStatus::Warning;
        check = check.note(format!("upgrade: {upgrade}"));
    } else if assessment.warns() {
        // A rule sets a minimum for this tool, and the version needed to check it could not
        // be read: for unrar and 7-Zip that minimum guards extraction, which still runs unless
        // the settings refuse such a tool (RD-1420-03).
        check.status = CheckStatus::Warning;
        let refused = ARCHIVE_TOOLS.contains(&tool) && input.refuse_unknown_archive_tool_version;
        check = check.note(if refused {
            "check: the version could not be read, and the settings refuse such a tool for \
             extraction; point the setting at the tool's own binary rather than a wrapper"
        } else {
            "check: the version could not be read, so its minimum version is not enforced; \
             point the setting at the tool's own binary rather than a wrapper"
        });
    }
    if tool == "7z" {
        // RD-1420-03 (audit L-12): unrar reads the password from its input, 7-Zip cannot.
        check = check.note(
            "note: 7-Zip gets an archive password on its command line, which other processes \
             of this machine can read while it runs; unrar reads it from its input instead",
        );
    }
    check
}

/// The two archive tools whose version floor guards extraction.
const ARCHIVE_TOOLS: [&str; 2] = ["unrar", "7z"];

/// Whether the data directory is the service account's alone (security review 2026-09-30,
/// finding 2): it holds the database, the secrets and the local control token, and whoever may
/// write it plants the journal an update's next start acts on. The path is not printed, so the
/// line can travel in a diagnostic bundle.
async fn data_directory_check(directory: std::path::PathBuf) -> Check {
    let directory = if directory.as_os_str().is_empty() {
        std::path::PathBuf::from(".")
    } else {
        directory
    };
    let verdict =
        tokio::task::spawn_blocking(move || rd_files::private_dir_exposure(&directory)).await;
    match verdict {
        Ok(Ok(None)) => Check::new(
            DATA,
            "permissions",
            CheckStatus::Ok,
            "only the service account has access",
        ),
        Ok(Ok(Some(why))) => Check::new(DATA, "permissions", CheckStatus::Warning, why)
            .note("Other accounts on this machine can read the database and the local")
            .note("control token, stop the service and plant files an update acts on."),
        Ok(Err(error)) => Check::new(
            DATA,
            "permissions",
            CheckStatus::Info,
            format!("could not be read: {error}"),
        ),
        Err(error) => Check::new(
            DATA,
            "permissions",
            CheckStatus::Info,
            format!("could not be read: {error}"),
        ),
    }
}

/// The reverse-proxy contract and anything half-configured about it.
///
/// Worth a section of its own because every mistake here produces a working service that
/// misbehaves later: a login that appears to succeed and does not, a rate limit that counts
/// everybody as one caller, or a page that loads blank. None of them look like a configuration
/// problem from the outside.
fn proxy_checks(input: &SystemChecksInput) -> Vec<Check> {
    let config = match rd_authn::ProxyConfig::parse(
        &input.trusted_proxies,
        input.external_url.as_deref(),
        input.cookie_security,
    ) {
        Ok(config) => config,
        Err(error) => {
            return vec![
                Check::new(
                    PROXY,
                    "configuration",
                    CheckStatus::Missing,
                    format!("not usable: {error}"),
                )
                .note("Until it is fixed, no forwarded header is read and the peer address")
                .note("is treated as the client."),
            ];
        }
    };
    let mut checks = vec![
        Check::new(
            PROXY,
            "external URL",
            CheckStatus::Info,
            match config.origin() {
                Some(origin) => format!("{origin}{}", config.base_path()),
                None => "not set (links and cookies assume the bind address)".to_owned(),
            },
        ),
        Check::new(
            PROXY,
            "trusted proxies",
            CheckStatus::Info,
            if config.trusted().is_empty() {
                "none - the peer address is the client".to_owned()
            } else {
                input.trusted_proxies.join(", ")
            },
        ),
        Check::new(
            PROXY,
            "session cookie",
            CheckStatus::Info,
            if config.cookie_is_secure() {
                "Secure"
            } else {
                "not Secure"
            },
        ),
    ];
    for warning in config.warnings() {
        checks.push(Check::new(
            PROXY,
            "warning",
            CheckStatus::Warning,
            warning.to_string(),
        ));
    }
    checks
}

#[cfg(test)]
mod tests {
    use rd_diagnostics::CheckStatus;

    use super::{SystemChecksInput, tool_check};

    fn unreadable(tool: &str) -> rd_core::ResolvedTool {
        rd_core::ResolvedTool {
            path: std::path::PathBuf::from("rd-doctor-test-missing").join(tool),
            source: rd_core::ToolSource::Explicit,
        }
    }

    /// An archive tool whose version cannot be read is extracted with all the same; `doctor`
    /// says so instead of reporting it as fine - and says it is refused once the settings
    /// refuse such a tool (RD-1420-03).
    #[tokio::test]
    async fn an_archive_tool_with_an_unreadable_version_is_a_warning() {
        let mut input = SystemChecksInput::default();
        let check = tool_check("unrar", &unreadable("unrar"), &input).await;
        assert_eq!(check.status, CheckStatus::Warning, "{check:?}");
        assert!(
            check
                .notes
                .iter()
                .any(|note| note.starts_with("check:") && note.contains("not enforced")),
            "{check:?}"
        );
        input.refuse_unknown_archive_tool_version = true;
        let refused = tool_check("unrar", &unreadable("unrar"), &input).await;
        assert_eq!(refused.status, CheckStatus::Warning, "{refused:?}");
        assert!(
            refused
                .notes
                .iter()
                .any(|note| note.starts_with("check:") && note.contains("refuse")),
            "{refused:?}"
        );
    }

    /// RD-1420-03 (audit L-12): 7-Zip is the tool that still gets the password on its command
    /// line, and `doctor` says so; unrar does not carry the note.
    #[tokio::test]
    async fn seven_zip_carries_the_command_line_password_note() {
        let input = SystemChecksInput::default();
        let seven = tool_check("7z", &unreadable("7z"), &input).await;
        assert!(
            seven
                .notes
                .iter()
                .any(|note| note.starts_with("note:") && note.contains("command line")),
            "{seven:?}"
        );
        let unrar = tool_check("unrar", &unreadable("unrar"), &input).await;
        assert!(
            !unrar.notes.iter().any(|note| note.contains("command line")),
            "{unrar:?}"
        );
    }
}
