//! The account half — sign-in, premium download, account check — against the synthetic
//! answers that mirror what JDownloader reads, since nothing behind the login was measured.

use plugin_common::{CaptchaChallenge, FailureKind, ResolveInput};

use super::{MockHost, TURBOBIT, body_of, code, header_of, json, run, tb};
use crate::account::parse_expiry_unix;
use crate::{check_account, resolve};

const TB_LINK: &str = "https://turbobit.net/a1b2c3d4e5f6.html";

fn with_account(url: &str) -> ResolveInput {
    ResolveInput {
        url: url.to_owned(),
        account_id: Some("account-1".to_owned()),
    }
}

#[test]
fn without_a_stored_password_nothing_is_requested() {
    let host = MockHost::new(Vec::new());
    let failure = run(check_account(&TURBOBIT, &host, "account-1")).expect_err("no password");
    assert_eq!(failure.kind, FailureKind::AuthRequired);
    assert_eq!(code(&failure), "turbobit.account_missing");
    let failure = run(resolve(&TURBOBIT, &host, &with_account(TB_LINK))).expect_err("no password");
    assert_eq!(code(&failure), "turbobit.account_missing");
    assert_eq!(host.request_count(), 0);
}

#[test]
fn the_check_signs_in_when_there_is_no_session_and_reads_the_subscription() {
    let host = MockHost::new(vec![
        json(401, tb::USER_INFO_401),
        json(200, tb::LOGIN_OK),
        json(200, tb::USER_ACTIVE),
        json(200, tb::PREMIUM_INFO),
    ])
    .with_password();
    let account = run(check_account(&TURBOBIT, &host, "account-1")).expect("checked");
    assert!(account.valid);
    assert!(account.premium);
    // 12,5 GB with a comma, in bytes.
    assert_eq!(
        account.traffic_left,
        Some(12 * 1024 * 1024 * 1024 + 512 * 1024 * 1024)
    );
    let codes: Vec<&str> = account
        .label
        .iter()
        .map(|part| part.code.as_str())
        .collect();
    assert_eq!(
        codes,
        vec![
            "plugin.account.user",
            "plugin.account.signed_in",
            "plugin.account.premium_until"
        ]
    );
    assert_eq!(
        account.label[2].params,
        vec![("until".to_owned(), "2099-01-01 00:00:00".to_owned())]
    );

    assert_eq!(
        host.urls(),
        vec![
            "https://app.turbobit.net/api/user/info",
            "https://app.turbobit.net/api/auth/login",
            "https://app.turbobit.net/api/user/info",
            "https://app.turbobit.net/api/premium/info",
        ]
    );
    // The credentials are markers the host expands; nothing here ever saw them.
    let login = host.request(1);
    assert_eq!(header_of(&login, "content-type"), Some("application/json"));
    let body = body_of(&login);
    assert!(body.contains(r#""email":"{{username}}""#), "{body}");
    assert!(
        body.contains(r#""password":"{{secret:turbobit_password}}""#),
        "{body}"
    );
    assert!(body.contains(r#""captcha":false"#), "{body}");
    assert!(host.captchas.borrow().is_empty());
}

#[test]
fn a_standing_session_is_left_alone() {
    let host = MockHost::new(vec![
        json(200, tb::USER_ACTIVE),
        json(200, tb::PREMIUM_INFO),
    ])
    .with_password();
    let account = run(check_account(&TURBOBIT, &host, "account-1")).expect("checked");
    assert!(account.premium);
    assert!(
        account
            .label
            .iter()
            .any(|part| part.code == "plugin.account.session_active")
    );
    assert_eq!(host.request_count(), 2);
}

#[test]
fn a_login_that_wants_a_captcha_gets_one_turnstile_round_on_the_login_page() {
    let host = MockHost::new(vec![
        json(401, tb::USER_INFO_401),
        json(422, tb::LOGIN_NEED_CAPTCHA),
        json(200, tb::CAPTCHA),
        json(200, tb::LOGIN_OK),
        json(200, tb::USER_ACTIVE),
        json(200, tb::PREMIUM_INFO),
    ])
    .with_password();
    run(check_account(&TURBOBIT, &host, "account-1")).expect("checked");
    let captchas = host.captchas.borrow();
    assert_eq!(captchas.len(), 1);
    match &captchas[0] {
        CaptchaChallenge::Turnstile(widget) => {
            assert_eq!(widget.page_url, "https://turbobit.net/login");
            assert_eq!(widget.site_key, "0x4AAAAAACiD4nQO5axxHO3o");
        }
        other => panic!("expected Turnstile, got {other:?}"),
    }
    let retry = body_of(&host.request(3));
    assert!(retry.contains(r#""captcha":true"#), "{retry}");
    assert!(
        retry.contains(r#""captchaResponse":"turnstile-token""#),
        "{retry}"
    );
}

#[test]
fn a_wrong_password_marks_the_account_and_a_refused_captcha_does_not() {
    let host = MockHost::new(vec![
        json(401, tb::USER_INFO_401),
        json(422, tb::LOGIN_PASSWORD_INCORRECT),
    ])
    .with_password();
    let failure = run(check_account(&TURBOBIT, &host, "account-1")).expect_err("wrong password");
    assert_eq!(failure.kind, FailureKind::AccountInvalid);
    assert_eq!(code(&failure), "turbobit.login_failed");

    let host = MockHost::new(vec![
        json(401, tb::USER_INFO_401),
        json(422, tb::LOGIN_NEED_CAPTCHA),
        json(200, tb::CAPTCHA),
        json(422, tb::LOGIN_INVALID_CAPTCHA),
    ])
    .with_password();
    let failure = run(check_account(&TURBOBIT, &host, "account-1")).expect_err("captcha refused");
    assert_eq!(failure.kind, FailureKind::Transient(None));
    assert_eq!(code(&failure), "turbobit.login_captcha");
}

#[test]
fn a_banned_account_is_reported_with_its_date() {
    let host = MockHost::new(vec![json(200, tb::USER_BANNED)]).with_password();
    let failure = run(check_account(&TURBOBIT, &host, "account-1")).expect_err("banned");
    assert_eq!(failure.kind, FailureKind::AccountInvalid);
    assert_eq!(code(&failure), "turbobit.account_banned");
    assert_eq!(
        failure.params,
        vec![("until".to_owned(), "2099-01-01 00:00:00".to_owned())]
    );
}

#[test]
fn an_expired_subscription_is_not_premium_and_says_so() {
    let host = MockHost::new(vec![json(200, tb::USER_EXPIRED)]).with_password();
    let account = run(check_account(&TURBOBIT, &host, "account-1")).expect("checked");
    assert!(account.valid);
    assert!(!account.premium);
    assert!(
        account
            .label
            .iter()
            .any(|part| part.code == "plugin.account.premium_expired")
    );
    assert_eq!(account.traffic_left, None);
    assert_eq!(
        host.request_count(),
        1,
        "no premium/info for an expired account"
    );
}

#[test]
fn an_answer_without_a_subscription_is_premium_unchecked() {
    let host = MockHost::new(vec![json(200, r#"{"user":{"email":"a@b.c"}}"#)]).with_password();
    let account = run(check_account(&TURBOBIT, &host, "account-1")).expect("checked");
    assert!(!account.premium);
    assert!(
        account
            .label
            .iter()
            .any(|part| part.code == "plugin.account.premium_unchecked")
    );
}

#[test]
fn a_premium_session_takes_the_first_mirror_without_a_captcha() {
    let host = MockHost::new(vec![
        json(200, tb::USER_ACTIVE),
        json(200, tb::INFO_PREMIUM),
    ])
    .with_password();
    let resolved = run(resolve(&TURBOBIT, &host, &with_account(TB_LINK))).expect("resolved");
    assert_eq!(
        resolved.url,
        "https://turbobit.net/download/redirect/fedcba9876543210fedcba9876543210/a1b2c3d4e5f6/Sample%20File%201.pdf"
    );
    assert_eq!(resolved.file_name.as_deref(), Some("Sample File 1.pdf"));
    assert_eq!(resolved.size, Some(193_434_567));
    assert_eq!(host.request_count(), 2);
    assert!(host.captchas.borrow().is_empty());
}

#[test]
fn a_lapsed_session_is_renewed_before_the_download() {
    let host = MockHost::new(vec![
        json(401, tb::USER_INFO_401),
        json(200, tb::LOGIN_OK),
        json(200, tb::USER_ACTIVE),
        json(200, tb::INFO_PREMIUM),
    ])
    .with_password();
    run(resolve(&TURBOBIT, &host, &with_account(TB_LINK))).expect("resolved");
    assert_eq!(host.urls()[1], "https://app.turbobit.net/api/auth/login");
}

#[test]
fn a_premium_session_without_a_mirror_is_the_daily_limit() {
    let host = MockHost::new(vec![
        json(200, tb::USER_ACTIVE),
        json(200, tb::INFO_PREMIUM_NO_URLS),
    ])
    .with_password();
    let failure = run(resolve(&TURBOBIT, &host, &with_account(TB_LINK))).expect_err("limit");
    assert_eq!(failure.kind, FailureKind::Transient(Some(1800)));
    assert_eq!(code(&failure), "turbobit.premium_limit_reached");
}

#[test]
fn a_signed_in_free_account_goes_the_guest_way_on_its_session() {
    let host = MockHost::new(vec![
        json(200, tb::USER_EXPIRED),
        json(200, tb::INFO_FREE),
        json(200, tb::INIT_OK),
        json(200, tb::CAPTCHA),
        json(200, tb::CAPTCHA_DELAY),
        json(200, tb::PREPARE_OK),
        json(200, tb::START_OK),
    ])
    .with_password();
    let resolved = run(resolve(&TURBOBIT, &host, &with_account(TB_LINK))).expect("resolved");
    assert!(
        resolved
            .url
            .contains("/download/redirect/0123456789abcdef0123456789abcdef/")
    );
    assert_eq!(host.captchas.borrow().len(), 1);
}

#[test]
fn the_expiry_format_the_site_uses_is_read_as_utc() {
    assert_eq!(
        parse_expiry_unix("2099-01-01 00:00:00"),
        Some(4_070_908_800)
    );
    assert_eq!(parse_expiry_unix("2000-01-01 00:00:00"), Some(946_684_800));
    assert_eq!(parse_expiry_unix("1970-01-01 00:00:01"), Some(1));
    assert_eq!(
        parse_expiry_unix("2026-09-21 12:34:56"),
        Some(1_789_994_096)
    );
    assert_eq!(parse_expiry_unix(""), None);
    assert_eq!(parse_expiry_unix("soon"), None);
    assert_eq!(parse_expiry_unix("2026-13-01 00:00:00"), None);
}
