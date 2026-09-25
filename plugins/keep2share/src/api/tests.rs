//! Unit tests for [`super`] (`crate::api`): URL matching, request-body construction and the
//! response-shape helpers (`civil_date`/`is_premium`/`premium_until`/`traffic_left`/`FileEntry`).
//! Error-classification tests and their full IMPL-VERIFY provenance live in
//! `api/errors/tests.rs`, next to `api/errors.rs` (split out for the same 500-line-limit reason
//! this file itself was split out of `api.rs` for — see plugin-common.md).

use super::*;

fn url(value: &str) -> Url {
    value.parse().expect("URL")
}

#[test]
fn file_id_accepts_f_file_and_preview_prefixes_with_optional_info_segment() {
    let id = "abcdefghijklm"; // 13 chars, JD's minimum
    assert_eq!(
        file_id(&url(&format!("https://k2s.cc/file/{id}"))),
        Some(id)
    );
    assert_eq!(file_id(&url(&format!("https://k2s.cc/f/{id}"))), Some(id));
    assert_eq!(
        file_id(&url(&format!("https://k2s.cc/preview/{id}"))),
        Some(id)
    );
    assert_eq!(
        file_id(&url(&format!("https://k2s.cc/file/info/{id}"))),
        Some(id)
    );
    assert_eq!(
        file_id(&url(&format!("https://k2s.cc/file/{id}/movie-name.html"))),
        Some(id)
    );
    assert_eq!(
        file_id(&url(&format!("https://www.keep2share.cc/file/{id}"))),
        Some(id)
    );
}

#[test]
fn file_id_rejects_short_ids_unknown_hosts_and_unsupported_shapes() {
    assert_eq!(file_id(&url("https://k2s.cc/file/tooshort")), None);
    assert_eq!(file_id(&url("https://k2s.cc/folder/abcdefghijklm")), None);
    assert_eq!(file_id(&url("https://k2s.cc/")), None);
    assert_eq!(
        file_id(&url("https://evil.example/file/abcdefghijklm")),
        None
    );
    assert_eq!(
        file_id(&url("https://fileboom.me/file/abcdefghijklm")),
        None
    );
    assert_eq!(
        file_id(&url("https://tezfiles.com/file/abcdefghijklm")),
        None
    );
}

#[test]
fn parse_download_url_accepts_a_well_formed_url() {
    let parsed = parse_download_url("https://k2s.cc/d/tok/release.rar").expect("valid URL");
    assert_eq!(parsed.as_str(), "https://k2s.cc/d/tok/release.rar");
}

#[test]
fn parse_download_url_rejects_a_malformed_url() {
    let failure = parse_download_url("not a url").expect_err("malformed URL");
    assert!(matches!(failure.kind, ErrorKind::Permanent));
    assert_eq!(failure.code, messages::INVALID_URL);
    assert!(failure.params.iter().any(|(name, _)| *name == "error"));
}

#[test]
fn login_body_serializes_the_literal_templates_with_username_first() {
    let body = super::login_body();
    assert_eq!(
        String::from_utf8(body).expect("utf8"),
        r#"{"username":"{{username}}","password":"{{secret:keep2share_password}}"}"#
    );
}

#[test]
fn geturl_body_serializes_file_id_and_auth_token() {
    let body = super::geturl_body("abcdefghijklm", "tok-abc");
    assert_eq!(
        String::from_utf8(body).expect("utf8"),
        r#"{"file_id":"abcdefghijklm","auth_token":"tok-abc"}"#
    );
}

#[test]
fn accountinfo_body_serializes_auth_token() {
    let body = super::accountinfo_body("tok-abc");
    assert_eq!(
        String::from_utf8(body).expect("utf8"),
        r#"{"auth_token":"tok-abc"}"#
    );
}

#[test]
fn getfilesinfo_body_serializes_the_id_list() {
    let body = super::getfilesinfo_body(&["a", "b"]);
    assert_eq!(
        String::from_utf8(body).expect("utf8"),
        r#"{"ids":["a","b"]}"#
    );
}

#[test]
fn civil_date_formats_known_epoch_seconds() {
    assert_eq!(civil_date(0), "1970-01-01");
    assert_eq!(civil_date(1_798_761_600), "2027-01-01");
}

#[test]
fn is_premium_is_true_only_for_a_numeric_account_expires() {
    assert!(is_premium(Some(&Value::from(1_798_761_600))));
    assert!(!is_premium(Some(&Value::Bool(false))));
    assert!(!is_premium(None));
}

#[test]
fn premium_until_states_the_end_date_only_while_premium() {
    assert_eq!(
        premium_until(true, Some(&Value::from(1_798_761_600))).as_deref(),
        Some("2027-01-01")
    );
    assert_eq!(premium_until(false, Some(&Value::Bool(false))), None);
    assert_eq!(premium_until(true, None), None);
}

#[test]
fn traffic_left_reads_the_numeric_available_traffic() {
    assert_eq!(traffic_left(Some(&Value::from(2048))), Some(2048));
    assert_eq!(traffic_left(Some(&Value::from(-5))), Some(0));
    assert_eq!(traffic_left(Some(&Value::Bool(false))), None);
    assert_eq!(traffic_left(None), None);
}

#[test]
fn file_entry_is_online_and_matches_fuid() {
    let online: FileEntry =
        serde_json::from_str(r#"{"id":"abc","name":"a.rar","size":10}"#).expect("json");
    assert!(online.is_online());
    assert!(online.matches_fuid("abc"));
    assert!(!online.matches_fuid("other"));

    let unavailable: FileEntry =
        serde_json::from_str(r#"{"id":"abc","is_available":false}"#).expect("json");
    assert!(!unavailable.is_online());

    let deleted: FileEntry =
        serde_json::from_str(r#"{"id":"abc","isDeleted":true}"#).expect("json");
    assert!(!deleted.is_online());

    let via_requested_id: FileEntry =
        serde_json::from_str(r#"{"id":"real","requested_id":"legacy"}"#).expect("json");
    assert!(via_requested_id.matches_fuid("legacy"));
}
