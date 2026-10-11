//! How a failed yt-dlp run is classified, and what it says about why.

use rd_core::FailureKind;

use super::{contains_words, map_tool_error};

fn category(stderr: &str) -> FailureKind {
    map_tool_error(stderr, Some(1)).category
}

/// RA-TR-04: a yt-dlp that cannot be started says why, not only "spawn yt-dlp".
#[tokio::test]
async fn a_tool_that_cannot_start_reports_the_cause() {
    let missing = tempfile::tempdir().expect("temp");
    let failure = super::run_json(
        &missing.path().join("no-such-yt-dlp"),
        &"https://example.invalid/watch".parse().expect("url"),
        false,
        std::time::Duration::from_secs(10),
        &rd_scheduler::ToolNetwork::direct(),
    )
    .await;
    let Err(failure) = failure else {
        panic!("nothing to start")
    };
    assert_eq!(failure.code.as_deref(), Some("media.tool_error"));
    let cause = failure
        .message
        .split_once("spawn yt-dlp: ")
        .map(|(_, cause)| cause.trim())
        .unwrap_or_default();
    assert!(!cause.is_empty(), "the cause was lost: {}", failure.message);
}

/// TR-02: yt-dlp's own wording, as it prints it.
#[test]
fn a_network_failure_is_retried_rather_than_taken_for_a_login_wall() {
    assert_eq!(
        category(
            "ERROR: [youtube] dQw4w9WgXcQ: Unable to download webpage: <urlopen error \
             [Errno -3] Temporary failure in name resolution> (caused by \
             URLError(gaierror(-3, 'Temporary failure in name resolution')))"
        ),
        FailureKind::Transient {
            retry_after_seconds: Some(120)
        }
    );
    assert_eq!(
        category(
            "ERROR: [generic] Unable to download webpage: HTTP Error 503: Service Unavailable"
        ),
        FailureKind::Transient {
            retry_after_seconds: Some(120)
        }
    );
}

#[test]
fn a_warning_does_not_decide_the_class() {
    let stderr = "WARNING: [youtube] Video 3 of the playlist was removed (HTTP 404)\n\
                  WARNING: Falling back to generic n function search\n\
                  ERROR: [youtube] dQw4w9WgXcQ: Unable to extract initial player response; \
                  please report this issue on https://github.com/yt-dlp/yt-dlp/issues";
    assert_eq!(
        category(stderr),
        FailureKind::Transient {
            retry_after_seconds: Some(120)
        }
    );
}

#[test]
fn age_and_sign_in_walls_need_an_account() {
    for stderr in [
        "ERROR: [youtube] dQw4w9WgXcQ: Sign in to confirm your age. This video may be \
         inappropriate for some users.",
        "ERROR: [youtube] dQw4w9WgXcQ: Sign in to confirm you\u{2019}re not a bot. Use \
         --cookies-from-browser or --cookies for the authentication.",
        "ERROR: [vimeo] 123456: This video is age-restricted",
    ] {
        assert_eq!(category(stderr), FailureKind::AuthRequired, "{stderr}");
    }
}

#[test]
fn gone_videos_are_permanent() {
    for stderr in [
        "ERROR: [youtube] dQw4w9WgXcQ: Video unavailable. This video has been removed by \
         the uploader",
        "ERROR: [youtube] dQw4w9WgXcQ: Private video. Sign in if you've been granted \
         access to this video",
        "ERROR: Unsupported URL: https://example.com/page",
        "ERROR: [generic] Unable to download webpage: HTTP Error 404: Not Found",
    ] {
        assert_eq!(category(stderr), FailureKind::Permanent, "{stderr}");
    }
}

#[test]
fn too_many_requests_waits_longer() {
    assert_eq!(
        category("ERROR: unable to download video data: HTTP Error 429: Too Many Requests"),
        FailureKind::RateLimited {
            retry_after_seconds: Some(600)
        }
    );
}

#[test]
fn words_match_whole_and_only_whole() {
    assert!(contains_words("confirm your age.", "age"));
    assert!(contains_words("age-restricted", "age"));
    for text in ["webpage", "message", "image", "storage", "usage limit"] {
        assert!(!contains_words(text, "age"), "{text}");
    }
    assert!(!contains_words("error 4040", "404"));
    assert!(contains_words("http error 404: not found", "404"));
}

#[test]
fn the_detail_is_the_error_line() {
    let failure = map_tool_error(
        "WARNING: something\nERROR: [youtube] x: Video unavailable\n",
        Some(1),
    );
    assert_eq!(
        failure.params.get("detail").map(String::as_str),
        Some("ERROR: [youtube] x: Video unavailable")
    );
}

/// RD-1240-29: yt-dlp's words for a proxy that refused the profile's password, through each of
/// its request handlers, are the proxy's permanent failure, not a network failure retried in
/// two minutes.
#[test]
fn a_proxy_refusing_its_password_is_a_proxy_failure() {
    for stderr in [
        // requests (the default handler), an HTTPS page through an HTTP proxy.
        "ERROR: [youtube] dQw4w9WgXcQ: Unable to download API page: ('Unable to connect to \
         proxy', OSError('Tunnel connection failed: 407 Proxy Authentication Required')) \
         (caused by ProxyError(\"('Unable to connect to proxy', OSError('Tunnel connection \
         failed: 407 Proxy Authentication Required'))\"))",
        // urllib, the fallback without requests.
        "ERROR: [generic] Unable to download webpage: <urlopen error Tunnel connection failed: \
         407 Proxy Authentication Required> (caused by URLError(OSError('Tunnel connection \
         failed: 407 Proxy Authentication Required')))",
        // A plain http:// page, which the proxy answers itself.
        "WARNING: [generic] Falling back on generic information extractor\n\
         ERROR: [generic] Unable to download webpage: HTTP Error 407: Proxy Authentication \
         Required (caused by <HTTPError 407: Proxy Authentication Required>)",
        // curl_cffi, the impersonating handler.
        "ERROR: [generic] Unable to download webpage: Failed to perform, curl: (56) CONNECT \
         tunnel failed, response 407. See https://curl.se/libcurl/c/libcurl-errors.html first \
         for more details.",
    ] {
        let failure = map_tool_error(stderr, Some(1));
        assert_eq!(
            failure.code.as_deref(),
            Some("proxy.auth_failed"),
            "{stderr}"
        );
        assert_eq!(failure.category, FailureKind::Permanent, "{stderr}");
    }
    // A 407 in a warning on the way decides nothing; the error line does.
    assert_eq!(
        category(
            "WARNING: [youtube] Retrying fragment 3: Tunnel connection failed: 407 Proxy \
             Authentication Required\n\
             ERROR: [generic] Unable to download webpage: HTTP Error 503: Service Unavailable"
        ),
        FailureKind::Transient {
            retry_after_seconds: Some(120)
        }
    );
}

/// RD-1240-37: a run that printed nothing still says how it ended, never "yt-dlp failed: ".
#[test]
fn a_silent_failure_names_its_exit_code() {
    for (stderr, code, detail) in [
        ("", Some(1), "exit code 1, no error output"),
        ("  \n\n", Some(2), "exit code 2, no error output"),
        ("", None, "ended by a signal, no error output"),
    ] {
        let failure = map_tool_error(stderr, code);
        assert_eq!(
            failure.params.get("detail").map(String::as_str),
            Some(detail),
            "{stderr:?}"
        );
        assert_eq!(failure.message, format!("yt-dlp failed: {detail}"));
    }
}
