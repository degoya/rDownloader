//! Exhaustive enumeration of JD's `LinkSnappyCom#handleErrors(DownloadLink, Account, Map)`
//! (`svn_trunk/src/jd/plugins/hoster/LinkSnappyCom.java`, revision 52502), in its exact
//! `if`/`else if` order, with the JD line each branch lives at and the [`super::classify_message`]
//! test that asserts it:
//!
//! 1. L652 `containsIgnoreCase(errormsg, "Two-Factor Verification Required")` →
//!    `AccountUnavailableException(msg + redirect, 5 * 60 * 1000)` →
//!    `two_factor_required_maps_to_rate_limited_300`.
//! 2. L662 `Regex("(?i)No server available for this filehost, Please retry after few minutes")` →
//!    `mhm.putError(account, link, 5 * 60 * 1000, errormsg)` (link-level, not thrown) →
//!    `host_unavailable_maps_to_transient_300`.
//! 3. L665 `Regex("(?i)You have reached max download request")` → `mhm.putError(..., 5 * 60 *
//!    1000, "Too many requests...")` → `too_many_requests_maps_to_rate_limited_300`.
//! 4. L668 `Regex("(?i)You have reached max download limit of")` →
//!    `AccountUnavailableException("Limit Reached...", 1 * 60 * 1000)` →
//!    `max_download_limit_maps_to_rate_limited_60`.
//! 5. L675 `Regex("(?i)Invalid file URL format\\.")` →
//!    `PluginException(ERROR_TEMPORARILY_UNAVAILABLE, "URL format not supported...")`, no explicit
//!    wait → `invalid_link_format_maps_to_transient_default`.
//! 6. L681 `Regex("(?i)File not found")` or `Regex("(?i)File deleted on.*")` →
//!    `PluginException(ERROR_FILE_NOT_FOUND)` → `file_not_found_and_file_deleted_map_to_offline`.
//! 7. L683 `Regex("(?i)Your Account has Expired")` → `AccountUnavailableException("Account
//!    expired", 5 * 60 * 1000)` → `account_expired_maps_to_rate_limited_300`.
//! 8. L693 `isErrorDownloadPasswordRequiredOrWrong(errormsg)` (L717: exact-string, case-insensitive
//!    `"This file requires password"`, via Java's own `String.matches`, not JD's substring-search
//!    `Regex` utility) → `PluginException(ERROR_RETRY, "Wrong password entered")` →
//!    `password_required_maps_to_permanent`.
//! 9. L697 `Regex("(?i)Please upgrade to Elite membership")` →
//!    `AccountUnavailableException("Daily downloadlimit reached", 10 * 60 * 1000)` →
//!    `elite_membership_required_maps_to_rate_limited_600`.
//! 10. L700 `Regex("(?i)Incorrect Username or Password")` → `AccountInvalidException(errormsg)` →
//!     `incorrect_username_or_password_maps_to_account_invalid`.
//! 11. L702 `Regex("(?i)Account has exceeded the daily quota")` →
//!     `errorDailyLimitReached(null, account, errormsg)` → (link hardcoded `null` regardless of
//!     the enclosing call's own `link`, so always the account-level branch) →
//!     `AccountUnavailableException(msg, 5 * 60 * 1000)` →
//!     `daily_quota_exceeded_maps_to_rate_limited_300`.
//! 12. L706 final `else` → `link == null`: `AccountUnavailableException(errormsg, 10 * 60 * 1000)`;
//!     `link != null`: `PluginException(ERROR_TEMPORARILY_UNAVAILABLE, errormsg, 5 * 60 * 1000)` →
//!     `unknown_message_maps_to_rate_limited_600_without_link_and_transient_300_with_link`.
//!
//! [`super::messages::HOST_UNSUPPORTED`] (tested by `not_supported_maps_to_unsupported`) has
//! **no** JD counterpart in this enumeration — see its doc comment in `messages.rs`.

use std::collections::HashMap;

use serde_json::json;

use super::*;

#[test]
fn matches_accepts_only_http_and_https() {
    assert!(matches("http"));
    assert!(matches("https"));
    assert!(!matches("ftp"));
    assert!(!matches("magnet"));
}

#[test]
fn gen_links_json_embeds_link_and_credential_templates() {
    assert_eq!(
        gen_links_json("https://example.test/f/abc"),
        r#"{"link":"https://example.test/f/abc","type":"","username":"{{username}}","password":"{{secret:linksnappy_password}}"}"#
    );
}

#[test]
fn gen_links_json_escapes_quotes_and_backslashes_in_the_link() {
    assert_eq!(
        gen_links_json(r#"https://example.test/f/"a\b"#),
        r#"{"link":"https://example.test/f/\"a\\b","type":"","username":"{{username}}","password":"{{secret:linksnappy_password}}"}"#
    );
}

#[test]
fn parse_download_url_accepts_a_well_formed_url() {
    let parsed =
        parse_download_url("https://cdn.linksnappy.com/dl/tok/release.rar").expect("valid URL");
    assert_eq!(
        parsed.as_str(),
        "https://cdn.linksnappy.com/dl/tok/release.rar"
    );
}

#[test]
fn parse_download_url_rejects_a_malformed_url() {
    let failure = parse_download_url("not a url").expect_err("malformed URL");
    assert!(matches!(failure.kind, ErrorKind::Permanent));
    assert_eq!(failure.code, messages::INVALID_URL);
    assert!(failure.params.iter().any(|(name, _)| *name == "error"));
}

#[test]
fn error_from_envelope_is_none_for_ok_status() {
    assert!(error_from_envelope(Some("OK"), None, true).is_none());
    assert!(error_from_envelope(Some("ok"), None, true).is_none());
}

#[test]
fn error_from_envelope_is_none_when_status_and_error_are_absent() {
    assert!(error_from_envelope(None, None, true).is_none());
}

#[test]
fn error_from_envelope_uses_the_string_error_verbatim() {
    let error = json!("Incorrect Username or Password");
    let failure = error_from_envelope(Some("ERROR"), Some(&error), true).expect("failure");
    assert_eq!(failure.code, messages::BAD_CREDENTIALS.0);
}

#[test]
fn error_from_envelope_synthesizes_a_placeholder_for_a_non_string_error() {
    let error = json!({"unexpected": true});
    let failure = error_from_envelope(Some("ERROR"), Some(&error), true).expect("failure");
    assert_eq!(failure.code, messages::API_ERROR);
    assert!(failure.message.contains("unknown/ERROR/"));
}

#[test]
fn error_from_envelope_reads_error_without_a_status() {
    let error = json!("File not found");
    let failure = error_from_envelope(None, Some(&error), true).expect("failure");
    assert_eq!(failure.code, messages::FILE_OFFLINE.0);
}

#[test]
fn two_factor_required_maps_to_rate_limited_300() {
    let failure = classify_message("Two-Factor Verification Required", true);
    assert!(matches!(failure.kind, ErrorKind::RateLimited(Some(300))));
    assert_eq!(failure.code, messages::TWO_FACTOR_REQUIRED.0);
}

#[test]
fn host_unavailable_maps_to_transient_300() {
    let failure = classify_message(
        "No server available for this filehost, Please retry after few minutes",
        true,
    );
    assert!(matches!(failure.kind, ErrorKind::Transient(Some(300))));
    assert_eq!(failure.code, messages::HOST_UNAVAILABLE.0);
}

#[test]
fn too_many_requests_maps_to_rate_limited_300() {
    let failure = classify_message("You have reached max download request", true);
    assert!(matches!(failure.kind, ErrorKind::RateLimited(Some(300))));
    assert_eq!(failure.code, messages::TOO_MANY_REQUESTS.0);
}

#[test]
fn max_download_limit_maps_to_rate_limited_60() {
    let failure = classify_message("You have reached max download limit of 10GB", true);
    assert!(matches!(failure.kind, ErrorKind::RateLimited(Some(60))));
    assert_eq!(failure.code, messages::LIMIT_REACHED.0);
}

#[test]
fn invalid_link_format_maps_to_transient_default() {
    let failure = classify_message("Invalid file URL format.", true);
    assert!(matches!(failure.kind, ErrorKind::Transient(None)));
    assert_eq!(failure.code, messages::INVALID_LINK_FORMAT.0);
}

#[test]
fn file_not_found_and_file_deleted_map_to_offline() {
    for message in ["File not found", "File deleted on 2024-01-01"] {
        let failure = classify_message(message, true);
        assert!(matches!(failure.kind, ErrorKind::Offline), "{message}");
        assert_eq!(failure.code, messages::FILE_OFFLINE.0, "{message}");
    }
}

#[test]
fn account_expired_maps_to_rate_limited_300() {
    let failure = classify_message("Your Account has Expired, Please extend it", true);
    assert!(matches!(failure.kind, ErrorKind::RateLimited(Some(300))));
    assert_eq!(failure.code, messages::ACCOUNT_EXPIRED.0);
}

#[test]
fn password_required_maps_to_permanent() {
    let failure = classify_message("This file requires password", true);
    assert!(matches!(failure.kind, ErrorKind::Permanent));
    assert_eq!(failure.code, messages::PASSWORD_PROTECTED.0);
}

#[test]
fn password_required_is_an_exact_match_not_a_substring() {
    // JD checks this one via Java's own `String.matches` (whole-string), not its `Regex` utility
    // (substring search) — extra text around the phrase must fall through to the generic bucket.
    let failure = classify_message("This file requires password to open", true);
    assert_eq!(failure.code, messages::API_ERROR);
}

#[test]
fn elite_membership_required_maps_to_rate_limited_600() {
    let failure = classify_message("Please upgrade to Elite membership", true);
    assert!(matches!(failure.kind, ErrorKind::RateLimited(Some(600))));
    assert_eq!(failure.code, messages::PREMIUM_REQUIRED.0);
}

#[test]
fn incorrect_username_or_password_maps_to_account_invalid() {
    let failure = classify_message("Incorrect Username or Password", true);
    assert!(matches!(failure.kind, ErrorKind::AccountInvalid));
    assert_eq!(failure.code, messages::BAD_CREDENTIALS.0);
}

#[test]
fn daily_quota_exceeded_maps_to_rate_limited_300() {
    let failure = classify_message("Account has exceeded the daily quota", true);
    assert!(matches!(failure.kind, ErrorKind::RateLimited(Some(300))));
    assert_eq!(failure.code, messages::LIMIT_REACHED.0);
}

#[test]
fn not_supported_maps_to_unsupported() {
    let failure = classify_message("This host is not supported", true);
    assert!(matches!(failure.kind, ErrorKind::Unsupported));
    assert_eq!(failure.code, messages::HOST_UNSUPPORTED.0);
}

#[test]
fn unknown_message_maps_to_rate_limited_600_without_link_and_transient_300_with_link() {
    let with_link = classify_message("Something LinkSnappy has never said before", true);
    assert!(matches!(with_link.kind, ErrorKind::Transient(Some(300))));
    assert_eq!(with_link.code, messages::API_ERROR);
    assert!(
        with_link
            .message
            .contains("Something LinkSnappy has never said before")
    );

    let without_link = classify_message("Something LinkSnappy has never said before", false);
    assert!(matches!(
        without_link.kind,
        ErrorKind::RateLimited(Some(600))
    ));
    assert_eq!(without_link.code, messages::API_ERROR);
}

#[test]
fn is_premium_treats_lifetime_and_numeric_epochs_as_premium() {
    assert!(is_premium(Some(&json!("lifetime"))));
    assert!(is_premium(Some(&json!("2177388000"))));
    assert!(is_premium(Some(&json!(1_999_999_999_i64))));
    assert!(!is_premium(Some(&json!("expired"))));
    assert!(!is_premium(Some(&json!("EXPIRED"))));
    assert!(!is_premium(None));
}

#[test]
fn subscription_reports_lifetime_expired_and_elite() {
    assert_eq!(
        subscription(Some(&json!("lifetime"))),
        Subscription::Lifetime
    );
    assert_eq!(subscription(Some(&json!("expired"))), Subscription::Expired);
    assert_eq!(
        subscription(Some(&json!(1_999_999_999_i64))),
        Subscription::Elite
    );
    assert_eq!(subscription(None), Subscription::Unknown);
}

#[test]
fn traffic_left_treats_unlimited_string_as_none() {
    assert_eq!(traffic_left(Some(&json!("unlimited"))), None);
    assert_eq!(traffic_left(None), None);
}

#[test]
fn traffic_left_clamps_negative_values_to_zero() {
    assert_eq!(traffic_left(Some(&json!(-5))), Some(0));
    assert_eq!(traffic_left(Some(&json!(1024))), Some(1024));
}

#[test]
fn merge_hosters_flattens_domains_and_lowercase_aliases_dedupes_and_sorts() {
    let mut entries = HashMap::new();
    entries.insert(
        "Rapidgator.net".to_owned(),
        HostEntry {
            alias: vec!["RG.TO".to_owned(), "rapidgator.asia".to_owned()],
        },
    );
    entries.insert(
        "1fichier.com".to_owned(),
        HostEntry {
            alias: vec!["1fichier.com".to_owned(), " ".to_owned()],
        },
    );
    assert_eq!(
        merge_hosters(entries),
        vec![
            "1fichier.com".to_owned(),
            "rapidgator.asia".to_owned(),
            "rapidgator.net".to_owned(),
            "rg.to".to_owned(),
        ]
    );
}

#[test]
fn ensure_http_status_maps_common_statuses() {
    assert!(ensure_http_status(200).is_ok());
    let unauthorized = ensure_http_status(401).expect_err("401");
    assert!(matches!(unauthorized.kind, ErrorKind::AccountInvalid));
    assert_eq!(unauthorized.code, messages::BAD_CREDENTIALS.0);
    let not_found = ensure_http_status(404).expect_err("404");
    assert!(matches!(not_found.kind, ErrorKind::Offline));
    let caching = ensure_http_status(425).expect_err("425");
    assert!(matches!(caching.kind, ErrorKind::Transient(Some(60))));
    let rate_limited = ensure_http_status(429).expect_err("429");
    assert!(matches!(rate_limited.kind, ErrorKind::RateLimited(None)));
    let server_error = ensure_http_status(503).expect_err("503");
    assert!(matches!(server_error.kind, ErrorKind::Transient(None)));
    let other = ensure_http_status(418).expect_err("418");
    assert!(matches!(other.kind, ErrorKind::Permanent));
    assert_eq!(other.code, messages::HTTP_ERROR);
}
