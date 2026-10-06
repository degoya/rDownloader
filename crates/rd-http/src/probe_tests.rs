/// RD-109-36: the rule exists only where two numbers disagree. The case that started it is
/// a 405 MB release answered with a 1150-byte refusal page.
#[test]
fn an_offer_far_under_the_announcement_is_a_contradiction() {
    assert!(super::contradicts_announced_size(425_134_653, 1150));
    assert!(super::contradicts_announced_size(425_134_653, 0));
}

/// A hoster prints a rounded size; the exact length may differ by well under a percent.
#[test]
fn rounding_in_the_announcement_is_tolerated() {
    assert!(!super::contradicts_announced_size(425_134_653, 425_132_032));
    assert!(!super::contradicts_announced_size(
        425_134_653,
        425_134_653 + 4_000_000
    ));
}

/// The guard must never fire on a legitimately small file. Below the floor the two numbers
/// are treated as agreeing whatever they are -- "it is small" is not evidence of anything.
#[test]
fn a_small_file_can_never_trip_the_rule() {
    assert!(!super::contradicts_announced_size(1, 1));
    assert!(!super::contradicts_announced_size(1150, 0));
    assert!(!super::contradicts_announced_size(500, 4_000));
    // Still caught once the announcement is large enough to mean something.
    assert!(super::contradicts_announced_size(1_000_000, 1150));
}

/// An oversized offer is as wrong as an undersized one: it is not the announced file either.
#[test]
fn an_offer_far_over_the_announcement_is_a_contradiction() {
    assert!(super::contradicts_announced_size(1_000_000, 900_000_000));
}
use axum::{Router, routing::get};

use super::{
    ProbeResult, is_weak_etag, parse_content_range_start, parse_content_range_total, probe,
    strip_markup,
};

#[test]
fn a_weak_etag_is_not_treated_as_a_validator() {
    // The bug this guards: a weak tag stays the same while the body legitimately
    // changes, so resuming on it continues into different bytes.
    assert!(is_weak_etag("W/\"abc\""));
    assert!(is_weak_etag("w/\"abc\""));
    assert!(!is_weak_etag("\"abc\""));
    assert!(!is_weak_etag("abc"));
}

fn sniffed(content_type: Option<&str>, disposition: Option<&str>, total: Option<u64>) -> bool {
    ProbeResult {
        final_url: "https://example.test/file".parse().expect("URL"),
        total_bytes: total,
        accepts_ranges: true,
        etag: None,
        last_modified: None,
        content_disposition: disposition.map(str::to_owned),
        content_type: content_type.map(str::to_owned),
    }
    .looks_downloadable()
}

#[test]
fn parses_content_range() {
    assert_eq!(parse_content_range_total("bytes 0-0/4096"), Some(4096));
    assert_eq!(parse_content_range_total("bytes */*"), None);
}

#[test]
fn parses_the_first_byte_a_content_range_describes() {
    assert_eq!(parse_content_range_start("bytes 0-0/4096"), Some(0));
    assert_eq!(
        parse_content_range_start("bytes 1024-2047/8192"),
        Some(1024)
    );
    assert_eq!(parse_content_range_start("Bytes 512-/8192"), Some(512));
    // An unsatisfied range names no first byte, and a unit that is not bytes says
    // nothing about offsets at all.
    assert_eq!(parse_content_range_start("bytes */8192"), None);
    assert_eq!(parse_content_range_start("items 0-9/50"), None);
    assert_eq!(parse_content_range_start("nonsense"), None);
}

#[test]
fn a_hoster_landing_page_is_not_downloadable_content() {
    assert!(!sniffed(
        Some("text/html; charset=utf-8"),
        None,
        Some(30_000)
    ));
    // Markup stays rejected regardless of size: a big page is still a page.
    assert!(!sniffed(Some("text/html"), None, Some(8 * 1024 * 1024)));
    assert!(!sniffed(Some("application/json"), None, Some(512)));
}

#[test]
fn real_payloads_and_attachments_stay_downloadable() {
    assert!(sniffed(Some("application/octet-stream"), None, Some(4096)));
    assert!(sniffed(Some("video/mp4"), None, None));
    assert!(sniffed(None, None, None));
    // An explicit attachment wins even when the type looks like markup.
    assert!(sniffed(
        Some("text/html"),
        Some("attachment; filename=\"archive.rar\""),
        Some(10)
    ));
    // A large text payload is a file, not an error page.
    assert!(sniffed(Some("text/plain"), None, Some(4 * 1024 * 1024)));
}

#[test]
fn markup_is_condensed_into_a_readable_message() {
    let page = "<html><body><h1>Wait</h1>\n<p>You have to wait 30 minutes</p></body></html>";
    assert_eq!(strip_markup(page), "Wait You have to wait 30 minutes");
}

#[tokio::test]
async fn head_probe_reports_the_advertised_content_length() {
    const PAYLOAD: &[u8] = b"cdn-payload-with-a-real-length";
    let app = Router::new().route("/file.bin", get(|| async { PAYLOAD }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fixture");
    let address = listener.local_addr().expect("fixture address");
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve fixture");
    });

    let url = format!("http://{address}/file.bin").parse().expect("URL");
    let result = probe(&reqwest::Client::new(), url).await.expect("probe");
    assert_eq!(result.total_bytes, Some(PAYLOAD.len() as u64));
}
