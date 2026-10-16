//! How a failed yt-dlp run is classified: the failure class the queue retries or blocks on,
//! and the stable code the interface explains it with.

use rd_core::{Failure, FailureKind};

/// Maps yt-dlp's stderr and exit code to a failure class the queue can retry or block on.
///
/// Read from the `ERROR:` line alone and by whole words (audit 1.9.1, TR-02): matched as a
/// substring of all of stderr, "age" found "Unable to download web*page*" — a network failure
/// that then counted as a login wall and was never retried — and a warning's "404" or
/// "removed" made a passing problem permanent.
pub(crate) fn map_tool_error(stderr: &str, exit_code: Option<i32>) -> Failure {
    let text = stderr.trim();
    if crate::merge::merge_skipped(text) {
        return crate::merge::merge_failure();
    }
    let error_line = text
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| line.starts_with("ERROR:"))
        .unwrap_or_else(|| text.lines().last().unwrap_or_default());
    // Before the rest: a proxy refusing the profile's password was a network failure retried
    // every two minutes, never the proxy's (RD-1240-29).
    if let Some(failure) = rd_scheduler::proxy_auth_failed(error_line) {
        return failure;
    }
    let lower = error_line.to_ascii_lowercase();
    let says = |phrases: &[&str]| phrases.iter().any(|phrase| contains_words(&lower, phrase));
    // Redacted before it becomes a message and a param: this tail is shown in the UI and
    // stored on the download row, and yt-dlp happily echoes the signed URL it just tried.
    let tail: String = rd_core::redact_text(error_line).chars().take(300).collect();
    // Never an empty detail: "yt-dlp failed: " with nothing after it names no cause at all
    // (RD-1240-37). A run that printed nothing still has its exit code to say.
    let tail = if tail.is_empty() {
        match exit_code {
            Some(code) => format!("exit code {code}, no error output"),
            None => "ended by a signal, no error output".to_owned(),
        }
    } else {
        tail
    };
    // A site's bot check refusing a request without its fingerprint cookies (Bilibili's 412
    // without `buvid3`/`buvid4`): retrying every two minutes only provokes it further, the
    // site's cookies through an authentication profile are what helps (RD-1270-01).
    if says(&["412", "precondition failed"]) {
        return Failure::coded(
            FailureKind::AuthRequired,
            "media.ytdlp_cookies_required",
            format!("The site refused yt-dlp without its cookies (HTTP 412): {tail}"),
        )
        .with_param("detail", tail);
    }
    if let Some(failure) = signin_required(error_line, &tail) {
        return failure;
    }
    let category = if says(&[
        "unsupported url",
        "video unavailable",
        "private video",
        "not available",
        "removed",
        "404",
    ]) {
        FailureKind::Permanent
    } else if says(&["sign in", "login", "log in", "age"]) {
        FailureKind::AuthRequired
    } else if says(&["429", "too many requests"]) {
        FailureKind::RateLimited {
            retry_after_seconds: Some(600),
        }
    } else {
        FailureKind::Transient {
            retry_after_seconds: Some(120),
        }
    };
    Failure::coded(
        category,
        "media.ytdlp_failed",
        format!("yt-dlp failed: {tail}"),
    )
    .with_param("detail", tail)
}

/// Whether `phrase` occurs in `text` with no letter or digit directly before or after it.
pub(super) fn contains_words(text: &str, phrase: &str) -> bool {
    let is_word = |character: Option<char>| character.is_some_and(char::is_alphanumeric);
    text.match_indices(phrase).any(|(start, _)| {
        !is_word(text[..start].chars().next_back())
            && !is_word(text[start + phrase.len()..].chars().next())
    })
}

/// Extractors whose "Cannot parse data" means the page is only shown to a signed-in session.
///
/// Facebook answers a reel without a session with HTTP 400, and yt-dlp reports that as data it
/// cannot parse and asks for a bug report (yt-dlp #15161). Retried every two minutes it never
/// changes; the site's cookies are what helps (RD-1320-01). Another extractor's identical
/// wording stays an ordinary failure until one is reported.
const SIGNIN_EXTRACTORS: [&str; 1] = ["facebook"];

/// yt-dlp's request to report the error to it, which is wrong advice for a missing sign-in.
const REPORT_REQUEST: &str = "; please report this issue";

/// The sign-in case of RD-1320-01, or `None` for any other line.
///
/// `error_line` is the `ERROR:` line as yt-dlp printed it, `tail` the redacted detail.
fn signin_required(error_line: &str, tail: &str) -> Option<Failure> {
    let (site, message) = error_line
        .strip_prefix("ERROR:")?
        .trim_start()
        .strip_prefix('[')?
        .split_once(']')?;
    let site = site.to_ascii_lowercase();
    if !SIGNIN_EXTRACTORS.contains(&site.as_str())
        || !contains_words(&message.to_ascii_lowercase(), "cannot parse data")
    {
        return None;
    }
    let detail = tail
        .find(REPORT_REQUEST)
        .map_or(tail, |end| &tail[..end])
        .trim_end()
        .to_owned();
    Some(
        Failure::coded(
            FailureKind::AuthRequired,
            "media.ytdlp_signin_required",
            format!("{site} shows this video only to a signed-in session: {detail}"),
        )
        .with_param("site", site)
        .with_param("detail", detail),
    )
}
