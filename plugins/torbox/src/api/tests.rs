//! The parts that touch no host: what an address is, and what a refusal means.

use serde_json::json;

use super::{
    ErrorEnvelope, ErrorKind, UserInfo, classify_error, ensure_http_status, failure_from,
    is_safe_id, matches, read_download_address, read_ticket, retry_after_seconds,
};
use crate::messages;

const TORRENT: &str = "https://api.torbox.app/v1/api/torrents/requestdl?torrent_id=4711&file_id=3";

fn envelope(body: serde_json::Value) -> ErrorEnvelope {
    serde_json::from_value(body).expect("an envelope")
}

/// One address shape and nothing else, although TorBox will fetch almost anything.
#[test]
fn only_a_torbox_download_address_is_claimed() {
    let ticket = read_ticket(TORRENT).expect("a torrent ticket");
    assert_eq!(ticket.kind.id_field, "torrent_id");
    assert_eq!(ticket.job_id, "4711");
    assert_eq!(ticket.file_id, "3");
    assert_eq!(
        read_ticket("https://api.torbox.app/v1/api/usenet/requestdl?usenet_id=1&file_id=0")
            .expect("a usenet ticket")
            .kind
            .list_path,
        "/usenet/mylist"
    );
    assert_eq!(
        read_ticket("https://api.torbox.app/v1/api/webdl/requestdl?web_id=1&file_id=0")
            .expect("a webdl ticket")
            .kind
            .list_path,
        "/webdl/mylist"
    );
    for foreign in [
        // A hoster link is somebody else's; TorBox can only fetch it as a job.
        "https://rapidgator.net/file/abc",
        // The right host, the wrong endpoint.
        "https://api.torbox.app/v1/api/torrents/mylist?id=1",
        // The right endpoint on the wrong host.
        "https://api.torbox.app.evil.invalid/v1/api/torrents/requestdl?torrent_id=1&file_id=0",
        // Plain HTTP to the API host.
        "http://api.torbox.app/v1/api/torrents/requestdl?torrent_id=1&file_id=0",
        // The parameters of another kind.
        "https://api.torbox.app/v1/api/torrents/requestdl?web_id=1&file_id=0",
        // Missing the file.
        "https://api.torbox.app/v1/api/torrents/requestdl?torrent_id=1",
        "magnet:?xt=urn:btih:da39a3ee5e6b4b0d3255bfef95601890afd80709",
        "not an address at all",
    ] {
        assert!(!matches(foreign), "{foreign} must not be claimed");
    }
}

/// The address comes from a candidate row a person can edit, and what goes out carries the
/// account's key.
#[test]
fn an_identifier_out_of_an_address_is_checked_before_it_goes_back_out() {
    assert!(is_safe_id("4711"));
    assert!(is_safe_id("abc-DEF_1"));
    for refused in ["", "../x", "a&token=stolen", "a b", &"x".repeat(65)] {
        assert!(!is_safe_id(refused), "{refused}");
    }
    assert!(
        read_ticket(
            "https://api.torbox.app/v1/api/torrents/requestdl?torrent_id=4711%26token%3Dx&file_id=3"
        )
        .is_none(),
        "an identifier smuggling a second parameter is refused"
    );
    // Anything the address carried beyond the two identifiers is simply not read.
    let ticket = read_ticket(&format!("{TORRENT}&token=leaked&zip_link=true")).expect("a ticket");
    assert_eq!(ticket.job_id, "4711");
    assert_eq!(ticket.file_id, "3");
}

/// A resolved address goes straight to the transfer engine, so it is checked, not trusted.
#[test]
fn the_minted_address_is_read_out_of_the_answer_and_checked() {
    let body = br#"{"success":true,"detail":"ok","data":"https://store-1.torbox.app/dl/abc"}"#;
    assert_eq!(
        read_download_address(body).as_deref(),
        Some("https://store-1.torbox.app/dl/abc")
    );
    for refused in [
        br#"{"success":true,"data":null}"#.as_slice(),
        br#"{"success":true,"data":""}"#.as_slice(),
        br#"{"success":true,"data":"javascript:alert(1)"}"#.as_slice(),
        br#"{"success":true,"data":"file:///etc/passwd"}"#.as_slice(),
        b"not json".as_slice(),
    ] {
        assert_eq!(
            read_download_address(refused),
            None,
            "{}",
            String::from_utf8_lossy(refused)
        );
    }
}

/// A refusal inside a 200 is still a refusal, and the word travels while the sentence does not.
#[test]
fn the_envelope_decides_before_the_status_does() {
    let refusal = failure_from(
        200,
        None,
        &envelope(json!({"success": false, "error": "BAD_TOKEN", "detail": "bad key abc123"})),
    )
    .expect("a refusal");
    assert_eq!(refusal.kind, ErrorKind::AccountInvalid);
    assert_eq!(refusal.code, messages::AUTH_INVALID.0);
    assert!(!refusal.message.contains("abc123"));
    assert!(failure_from(200, None, &envelope(json!({"success": true}))).is_none());
    let limited = failure_from(429, Some(30), &envelope(json!({}))).expect("a refusal");
    assert_eq!(limited.kind, ErrorKind::RateLimited(Some(30)));
    assert_eq!(retry_after_seconds(Some("30")), Some(30));
    assert_eq!(retry_after_seconds(Some("tomorrow")), None);
}

#[test]
fn each_word_and_each_status_lands_where_a_caller_can_act_on_it() {
    for (word, kind) in [
        ("NO_AUTH", ErrorKind::AccountInvalid),
        ("ITEM_NOT_FOUND", ErrorKind::Offline),
        ("LINK_OFFLINE", ErrorKind::Offline),
        ("PLAN_RESTRICTED_FEATURE", ErrorKind::Unsupported),
        ("MONTHLY_LIMIT", ErrorKind::RateLimited(Some(3600))),
        ("DOWNLOAD_SERVER_ERROR", ErrorKind::Transient(Some(300))),
    ] {
        assert_eq!(classify_error(word, None).kind, kind, "{word}");
    }
    let unknown = classify_error("SOMETHING_NEW", None);
    assert_eq!(unknown.code, messages::API_ERROR.0);
    assert!(unknown.message.contains("SOMETHING_NEW"));
    assert!(ensure_http_status(200, None).is_ok());
    assert_eq!(
        ensure_http_status(404, None).expect_err("a refusal").kind,
        ErrorKind::Offline
    );
}

/// A cancelled subscription still has the plan until it runs out, and showing a free account
/// for the last month somebody paid for is wrong in the direction that matters.
#[test]
fn a_paid_plan_is_read_from_the_plan_number_and_the_subscription_together() {
    let paid: UserInfo =
        serde_json::from_value(json!({"plan": 2, "is_subscribed": false})).expect("a user");
    assert!(paid.is_premium());
    let subscribed: UserInfo =
        serde_json::from_value(json!({"plan": 0, "is_subscribed": true})).expect("a user");
    assert!(subscribed.is_premium());
    let free: UserInfo = serde_json::from_value(json!({"plan": 0})).expect("a user");
    assert!(!free.is_premium());
    let named: UserInfo =
        serde_json::from_value(json!({"email": "  nobody@example.invalid  "})).expect("a user");
    assert_eq!(named.display_name(), Some("nobody@example.invalid"));
    let blank: UserInfo = serde_json::from_value(json!({"email": "   "})).expect("a user");
    assert_eq!(blank.display_name(), None);
}
