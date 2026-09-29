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
}

const HELPERS: &str = "download helpers";
const PROXY: &str = "Reverse proxy";

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
            Some(found) => checks.push(tool_check(tool, &found).await),
            None => checks.push(Check::new(HELPERS, tool, CheckStatus::Missing, "not found")),
        }
    }
    // streamlink additionally falls back to the portable build in vendor/streamlink/bin.
    let stream_probe = rd_core::StreamSettings {
        vendor_directory: input.vendor_directory.clone(),
        ..Default::default()
    };
    match rd_stream::locate_streamlink(&stream_probe) {
        Some(found) => checks.push(tool_check("streamlink", &found).await),
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
    checks
}

/// A found tool: its path and source, the version it reports and what the rules make of it
/// (RD-102-03). The upgrade path is its own note, because that is the line a person reading
/// `doctor` output is looking for, and a version that could not be read has to look different
/// from one that is simply old.
async fn tool_check(tool: &str, found: &rd_core::ResolvedTool) -> Check {
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
    }
    check
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
