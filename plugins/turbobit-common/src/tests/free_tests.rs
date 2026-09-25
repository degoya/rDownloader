//! The free flow, step for step, against the measured and the labelled synthetic answers.
//!
//! Each test asserts what the flow *did* — which calls in which order, which captcha, which
//! wait — and not only what it returned, because the site's own sequence is the contract.

use plugin_common::{CaptchaChallenge, FailureKind, ResolveInput};

use super::{HITFILE, MockHost, TURBOBIT, body_of, code, header_of, hf, html, json, run, tb};
use crate::free::GUEST_WINDOW_SECONDS;
use crate::resolve;

const TB_LINK: &str = "https://turbobit.net/a1b2c3d4e5f6.html";

fn free(url: &str) -> ResolveInput {
    ResolveInput {
        url: url.to_owned(),
        account_id: None,
    }
}

/// The six answers of a complete Turbobit guest download.
fn turbobit_success() -> Vec<plugin_common::HttpResponse> {
    vec![
        json(200, tb::INFO_FREE),
        json(200, tb::INIT_OK),
        json(200, tb::CAPTCHA),
        json(200, tb::CAPTCHA_DELAY),
        json(200, tb::PREPARE_OK),
        json(200, tb::START_OK),
    ]
}

#[test]
fn the_guest_flow_runs_the_sites_sequence_and_hands_the_link_over_unfetched() {
    let host = MockHost::new(turbobit_success());
    let resolved = run(resolve(&TURBOBIT, &host, &free(TB_LINK))).expect("resolved");

    assert_eq!(
        resolved.url,
        "https://turbobit.net/download/redirect/0123456789abcdef0123456789abcdef/a1b2c3d4e5f6/Sample%20File%201.pdf"
    );
    assert_eq!(resolved.file_name.as_deref(), Some("Sample File 1.pdf"));
    assert_eq!(resolved.size, Some(193_434_567));
    assert_eq!(
        resolved
            .headers
            .iter()
            .find(|header| header.name == "Referer")
            .map(|header| header.value.as_str()),
        Some("https://turbobit.net/download/started/a1b2c3d4e5f6")
    );

    // The order the SPA itself follows, and nothing after `free/start`: the link is one-shot.
    assert_eq!(
        host.urls(),
        vec![
            "https://app.turbobit.net/api/download/info",
            "https://app.turbobit.net/api/download/free/init",
            "https://app.turbobit.net/api/captcha",
            "https://app.turbobit.net/api/download/free/captcha",
            "https://app.turbobit.net/api/download/free/prepare",
            "https://app.turbobit.net/api/download/free/start",
        ]
    );
    for index in 0..host.request_count() {
        let request = host.request(index);
        assert_eq!(
            header_of(&request, "accept"),
            Some("application/json"),
            "{index}"
        );
        assert_eq!(
            header_of(&request, "origin"),
            Some("https://turbobit.net"),
            "{index}"
        );
        if request.method == "POST" {
            assert!(
                body_of(&request).contains(r#""fileId":"a1b2c3d4e5f6""#),
                "{index}"
            );
        }
    }
    let captcha_post = body_of(&host.request(3));
    assert!(
        captcha_post.contains(r#""captchaResponse":"turnstile-token""#),
        "{captcha_post}"
    );
    assert!(
        captcha_post.contains(r#""captchaIndex":0"#),
        "{captcha_post}"
    );

    let captchas = host.captchas.borrow();
    assert_eq!(captchas.len(), 1);
    match &captchas[0] {
        CaptchaChallenge::Turnstile(widget) => {
            assert_eq!(widget.site_key, "0x4AAAAAACiD4nQO5axxHO3o");
            assert_eq!(
                widget.page_url,
                "https://turbobit.net/download/free/a1b2c3d4e5f6"
            );
        }
        other => panic!("expected Turnstile, got {other:?}"),
    }
    assert_eq!(*host.waits.borrow(), vec![60]);
}

#[test]
fn hitfile_runs_the_same_sequence_with_its_own_key_and_countdown() {
    let host = MockHost::new(vec![
        json(200, hf::INFO_FREE),
        json(200, hf::INIT_OK),
        json(200, hf::CAPTCHA),
        json(200, hf::CAPTCHA_DELAY),
        json(200, hf::PREPARE_OK),
        json(200, hf::START_OK),
    ]);
    let resolved =
        run(resolve(&HITFILE, &host, &free("https://hil.to/Gh2IjKl"))).expect("resolved");
    assert_eq!(
        resolved.url,
        "https://hitfile.net/download/redirect/0123456789abcdef0123456789abcdef/Gh2IjKl/free-sample.zip"
    );
    assert_eq!(resolved.file_name.as_deref(), Some("free-sample.zip"));
    assert!(
        host.urls()
            .iter()
            .all(|url| url.starts_with("https://app.hitfile.net/api/"))
    );
    match &host.captchas.borrow()[0] {
        CaptchaChallenge::Turnstile(widget) => {
            assert_eq!(widget.site_key, "0x4AAAAAACiD-ljaJGPKwX-V");
            assert_eq!(widget.page_url, "https://hitfile.net/download/free/Gh2IjKl");
        }
        other => panic!("expected Turnstile, got {other:?}"),
    }
    assert_eq!(*host.waits.borrow(), vec![38]);
}

/// Criterion "short-lived link": a second resolve of the same file is a second full round
/// and yields whatever the site hands out now, never the earlier address.
#[test]
fn every_resolve_earns_a_fresh_link_and_none_is_remembered() {
    let second = tb::START_OK.replace(
        "0123456789abcdef0123456789abcdef",
        "ffffffffffffffffffffffffffffffff",
    );
    let mut answers = turbobit_success();
    answers.extend(turbobit_success());
    answers[11] = json(200, &second);
    let host = MockHost::new(answers);
    let first = run(resolve(&TURBOBIT, &host, &free(TB_LINK))).expect("first");
    let again = run(resolve(&TURBOBIT, &host, &free(TB_LINK))).expect("second");
    assert_ne!(first.url, again.url);
    assert!(again.url.contains("ffffffffffffffffffffffffffffffff"));
    assert_eq!(host.request_count(), 12, "two complete rounds, no shortcut");
    assert_eq!(host.captchas.borrow().len(), 2);
}

#[test]
fn an_already_generated_redirect_link_is_resolved_afresh_by_its_id() {
    let host = MockHost::new(turbobit_success());
    let stale = "https://turbobit.net/download/redirect/ffffffffffffffffffffffffffffffff/a1b2c3d4e5f6/Sample.pdf";
    let resolved = run(resolve(&TURBOBIT, &host, &free(stale))).expect("resolved");
    assert!(!resolved.url.contains("ffffffffffffffffffffffffffffffff"));
    assert_eq!(host.request_count(), 6);
}

#[test]
fn a_deleted_file_is_offline_after_one_call() {
    for (brand, link, answer) in [
        (&TURBOBIT, TB_LINK, tb::INFO_DELETED),
        (&HITFILE, "https://hitfile.net/Mn3OpQr", hf::INFO_DELETED),
    ] {
        let host = MockHost::new(vec![json(404, answer)]);
        let failure = run(resolve(brand, &host, &free(link))).expect_err("deleted");
        assert_eq!(failure.kind, FailureKind::Offline);
        assert!(
            code(&failure).ends_with(".file_unavailable"),
            "{}",
            code(&failure)
        );
        assert_eq!(host.request_count(), 1);
        assert!(host.captchas.borrow().is_empty());
    }
}

/// HitFile's public premium-only file, measured live: refused before any captcha is spent.
#[test]
fn a_premium_only_file_is_refused_before_the_captcha() {
    let host = MockHost::new(vec![json(200, hf::INFO_PREMIUM_ONLY)]);
    let failure = run(resolve(
        &HITFILE,
        &host,
        &free("https://hitfile.net/Ab1CdEf"),
    ))
    .expect_err("premium only");
    assert_eq!(failure.kind, FailureKind::AuthRequired);
    assert_eq!(code(&failure), "hitfile.premium_only");
    assert_eq!(
        host.request_count(),
        1,
        "no free/init, no captcha for a file a guest never gets"
    );
    assert!(host.captchas.borrow().is_empty());
}

#[test]
fn a_file_above_the_guest_size_limit_is_premium_only_too() {
    let info = tb::INFO_FREE.replace(r#""freeDownloadSize":null"#, r#""freeDownloadSize":1024"#);
    let host = MockHost::new(vec![json(200, &info)]);
    let failure = run(resolve(&TURBOBIT, &host, &free(TB_LINK))).expect_err("too large");
    assert_eq!(code(&failure), "turbobit.premium_only");
}

/// `directHit: true`, measured live on both brands: this IP is inside the guest window. Not
/// waited out — handed to the scheduler as an IP block with the assumed window.
#[test]
fn the_guest_window_is_an_ip_block_with_a_retry_after_and_costs_no_captcha() {
    for (brand, link, info, init) in [
        (&TURBOBIT, TB_LINK, tb::INFO_FREE, tb::INIT_DIRECT_HIT),
        (
            &HITFILE,
            "https://hitfile.net/Gh2IjKl",
            hf::INFO_FREE,
            hf::INIT_DIRECT_HIT,
        ),
    ] {
        let host = MockHost::new(vec![json(200, info), json(200, init)]);
        let failure = run(resolve(brand, &host, &free(link))).expect_err("blocked");
        assert_eq!(
            failure.kind,
            FailureKind::IpBlocked(Some(GUEST_WINDOW_SECONDS))
        );
        assert!(code(&failure).ends_with(".free_limit_reached"));
        assert_eq!(
            failure.params,
            vec![("wait_seconds".to_owned(), GUEST_WINDOW_SECONDS.to_string())]
        );
        assert!(host.captchas.borrow().is_empty());
        assert!(host.waits.borrow().is_empty());
        assert_eq!(host.request_count(), 2);
    }
}

#[test]
fn an_ip_ban_with_a_stated_delay_is_an_ip_block_for_that_long() {
    let host = MockHost::new(vec![json(200, tb::INFO_FREE), json(200, tb::INIT_IP_BAN)]);
    let failure = run(resolve(&TURBOBIT, &host, &free(TB_LINK))).expect_err("banned");
    assert_eq!(failure.kind, FailureKind::IpBlocked(Some(1800)));
}

#[test]
fn a_rejected_captcha_is_answered_once_more_with_a_fresh_challenge() {
    let host = MockHost::new(vec![
        json(200, tb::INFO_FREE),
        json(200, tb::INIT_OK),
        json(200, tb::CAPTCHA),
        json(422, tb::CAPTCHA_INVALID),
        json(200, tb::CAPTCHA),
        json(200, tb::CAPTCHA_DELAY),
        json(200, tb::PREPARE_OK),
        json(200, tb::START_OK),
    ]);
    run(resolve(&TURBOBIT, &host, &free(TB_LINK))).expect("second answer accepted");
    assert_eq!(host.captchas.borrow().len(), 2);
    assert_eq!(host.request_count(), 8);
}

#[test]
fn a_captcha_rejected_twice_is_reported_and_the_flow_stops() {
    let host = MockHost::new(vec![
        json(200, tb::INFO_FREE),
        json(200, tb::INIT_OK),
        json(200, tb::CAPTCHA),
        json(422, tb::CAPTCHA_INVALID),
        json(200, tb::CAPTCHA),
        json(422, tb::CAPTCHA_INVALID),
    ]);
    let failure = run(resolve(&TURBOBIT, &host, &free(TB_LINK))).expect_err("rejected twice");
    assert_eq!(failure.kind, FailureKind::CaptchaFailed);
    assert_eq!(code(&failure), "turbobit.captcha_rejected");
    assert_eq!(host.captchas.borrow().len(), 2);
    assert_eq!(host.request_count(), 6, "no prepare, no start");
}

#[test]
fn without_a_solver_the_hosts_refusal_is_reported_and_nothing_is_posted() {
    let host = MockHost::new(vec![
        json(200, tb::INFO_FREE),
        json(200, tb::INIT_OK),
        json(200, tb::CAPTCHA),
    ])
    .without_solver();
    let failure = run(resolve(&TURBOBIT, &host, &free(TB_LINK))).expect_err("no solver");
    assert_eq!(code(&failure), "captcha.no_solver");
    assert_eq!(host.request_count(), 3);
}

#[test]
fn a_widget_other_than_turnstile_is_not_claimed() {
    let host = MockHost::new(vec![
        json(200, tb::INFO_FREE),
        json(200, tb::INIT_OK),
        json(
            200,
            r#"{"driver":"recaptcha2","index":0,"publicKey":"6Lc-x"}"#,
        ),
    ]);
    let failure = run(resolve(&TURBOBIT, &host, &free(TB_LINK))).expect_err("unknown widget");
    assert_eq!(code(&failure), "turbobit.invalid_response");
    assert_eq!(
        failure.params,
        vec![("field".to_owned(), "driver".to_owned())]
    );
    assert!(host.captchas.borrow().is_empty());
}

#[test]
fn no_countdown_means_no_wait() {
    let mut answers = turbobit_success();
    answers[3] = json(200, "{}");
    let host = MockHost::new(answers);
    run(resolve(&TURBOBIT, &host, &free(TB_LINK))).expect("resolved");
    assert!(host.waits.borrow().is_empty());
}

/// Criterion 1, second half: no answer of the free flow ever becomes a download of a page.
#[test]
fn a_start_without_a_link_or_with_a_foreign_one_never_yields_the_page() {
    for (brand, answer, expected, kind) in [
        (
            &TURBOBIT,
            json(200, tb::START_NO_URL),
            "turbobit.no_direct_link",
            FailureKind::Permanent,
        ),
        (
            &TURBOBIT,
            json(200, tb::START_FOREIGN),
            "turbobit.no_direct_link",
            FailureKind::Permanent,
        ),
        (
            &TURBOBIT,
            json(404, tb::START_NOT_FOUND),
            "turbobit.file_unavailable",
            FailureKind::Offline,
        ),
        (
            &TURBOBIT,
            html(200, tb::SHELL),
            "turbobit.invalid_response",
            FailureKind::Permanent,
        ),
        (
            &TURBOBIT,
            html(302, tb::REDIRECT_PAGE),
            "turbobit.invalid_response",
            FailureKind::Permanent,
        ),
    ] {
        let mut answers = turbobit_success();
        answers[5] = answer;
        let host = MockHost::new(answers);
        let failure = run(resolve(brand, &host, &free(TB_LINK))).expect_err(expected);
        assert_eq!(code(&failure), expected);
        assert_eq!(failure.kind, kind, "{expected}");
    }
    // The same for HitFile's measured `free/start` refusals, in the same slot.
    for (answer, expected, kind) in [
        (
            json(409, hf::START_409),
            "hitfile.no_direct_link",
            FailureKind::Permanent,
        ),
        (
            json(400, hf::START_400_FEASIBILITY),
            "hitfile.premium_only",
            FailureKind::AuthRequired,
        ),
        (
            html(409, hf::ERROR_PAGE),
            "hitfile.invalid_response",
            FailureKind::Permanent,
        ),
    ] {
        let host = MockHost::new(vec![
            json(200, hf::INFO_FREE),
            json(200, hf::INIT_OK),
            json(200, hf::CAPTCHA),
            json(200, hf::CAPTCHA_DELAY),
            json(200, hf::PREPARE_OK),
            answer,
        ]);
        let failure = run(resolve(
            &HITFILE,
            &host,
            &free("https://hitfile.net/Gh2IjKl"),
        ))
        .expect_err(expected);
        assert_eq!(code(&failure), expected);
        assert_eq!(failure.kind, kind, "{expected}");
    }
}

#[test]
fn the_spa_shell_in_place_of_the_first_answer_is_a_read_failure_not_a_file() {
    for (brand, link, shell) in [
        (&TURBOBIT, TB_LINK, tb::SHELL),
        (&HITFILE, "https://hitfile.net/Gh2IjKl", hf::SHELL),
    ] {
        let host = MockHost::new(vec![html(200, shell)]);
        let failure = run(resolve(brand, &host, &free(link))).expect_err("a shell is not a file");
        assert!(code(&failure).ends_with(".invalid_response"));
        assert_eq!(failure.kind, FailureKind::Permanent);
    }
}

#[test]
fn a_percent_sign_in_the_name_survives_the_link() {
    let mut answers = turbobit_success();
    answers[5] = json(200, tb::START_PERCENT);
    let host = MockHost::new(answers);
    let resolved = run(resolve(&TURBOBIT, &host, &free(TB_LINK))).expect("resolved");
    assert!(
        resolved.url.ends_with("/a1b2c3d4e5f6/100%25%20done.rar"),
        "{}",
        resolved.url
    );
    assert_eq!(resolved.file_name.as_deref(), Some("100% done.rar"));
}

#[test]
fn a_folder_an_unsupported_and_an_invalid_link_cost_no_request() {
    for (url, expected, kind) in [
        (
            "https://turbobit.net/download/folder/123",
            "turbobit.folder_not_file",
            FailureKind::Unsupported,
        ),
        (
            "https://turbobit.net/rules",
            "turbobit.unsupported_link",
            FailureKind::Unsupported,
        ),
        (
            "https://example.com/a1b2c3d4e5f6.html",
            "turbobit.unsupported_link",
            FailureKind::Unsupported,
        ),
        ("nope", "turbobit.invalid_link", FailureKind::Permanent),
    ] {
        let host = MockHost::new(Vec::new());
        let failure = run(resolve(&TURBOBIT, &host, &free(url))).expect_err(expected);
        assert_eq!(code(&failure), expected);
        assert_eq!(failure.kind, kind);
        assert_eq!(host.request_count(), 0);
    }
}

#[test]
fn a_rate_limit_and_a_server_error_keep_their_kinds() {
    let host = MockHost::new(vec![json(429, "")]);
    let failure = run(resolve(&TURBOBIT, &host, &free(TB_LINK))).expect_err("rate limited");
    assert_eq!(failure.kind, FailureKind::RateLimited(None));
    assert_eq!(code(&failure), "turbobit.rate_limited");
    let host = MockHost::new(vec![json(503, "")]);
    let failure = run(resolve(&TURBOBIT, &host, &free(TB_LINK))).expect_err("down");
    assert_eq!(failure.kind, FailureKind::Transient(None));
    assert_eq!(
        failure.params,
        vec![("status".to_owned(), "503".to_owned())]
    );
}
