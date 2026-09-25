//! Account-less (free) resolve coverage for the Keep2Share JSON flow.
//!
//! Drives the real resolver against queued mock responses and asserts what the plugin *did* —
//! which requests, in which order, with which bodies, which captcha and which wait — not only what
//! it returned. The flow itself and its JD provenance are documented in `crate::api::free`.

use rd_core::FailureKind;
use rd_plugin_api::{CaptchaChallenge, ClientIdentity, ResolveRequest, Resolver, ResolverHost};

use std::sync::Arc;

use super::super::Keep2ShareResolver;
use super::{FILE_ID, FILE_URL, MockHost, body_str, geturl_response, json_response};

/// The account-less request the free flow answers.
fn free_request() -> ResolveRequest {
    ResolveRequest {
        url: FILE_URL.parse().expect("URL"),
        client: ClientIdentity {
            account_id: None,
            proxy_profile_id: None,
            tls_revision: 0,
        },
    }
}

/// The `/geturl` probe's answer when a captcha is needed. JD quotes this exact payload in its
/// errorcode-30 arm (`K2SApi.java:1531-1533`).
const CAPTCHA_DEMANDED: &str = r#"{"message":"You need send request for free download with captcha fields","status":"error","code":406,"errorCode":30}"#;

/// `/requestcaptcha`'s answer: the challenge id plus the image URL.
const CAPTCHA_CHALLENGE: &str = r#"{"status":"success","code":200,"challenge":"chal-7","captcha_url":"http://k2s.cc/api/v2/captcha.html?id=chal-7"}"#;

/// `/geturl`'s answer after a valid captcha — JD's own documented sample (line 879).
const CAPTCHA_ACCEPTED: &str = r#"{"status":"success","code":200,"message":"Captcha accepted, please wait","free_download_key":"homeHash","time_wait":30}"#;

const DOWNLOAD_URL: &str = "https://fs42.k2s.cc/d/homeHash/release.rar";

/// A PNG captcha image, served with an explicit content type.
fn captcha_image() -> rd_plugin_api::HostHttpResponse {
    rd_plugin_api::HostHttpResponse {
        status: 200,
        final_url: "https://k2s.cc/api/v2/captcha.html?id=chal-7"
            .parse()
            .expect("URL"),
        headers: vec![rd_plugin_api::ResolvedHeader {
            name: "Content-Type".to_owned(),
            value: "image/png".to_owned(),
        }],
        body: PNG_BYTES.to_vec(),
    }
}

const PNG_BYTES: &[u8] = b"\x89PNG\r\n\x1a\n-pretend-this-is-a-captcha-";

fn requestcaptcha_response(body: &str) -> rd_plugin_api::HostHttpResponse {
    json_response(200, "https://k2s.cc/api/v2/requestcaptcha", body)
}

fn resolver(host: &Arc<MockHost>) -> Keep2ShareResolver {
    Keep2ShareResolver::new(Arc::clone(host) as Arc<dyn ResolverHost>)
}

#[tokio::test]
async fn the_free_flow_probes_solves_the_image_captcha_waits_and_uses_the_free_download_key() {
    let host = MockHost::free(
        vec![
            geturl_response(CAPTCHA_DEMANDED),
            requestcaptcha_response(CAPTCHA_CHALLENGE),
            captcha_image(),
            geturl_response(CAPTCHA_ACCEPTED),
            geturl_response(&format!(
                r#"{{"status":"success","code":200,"url":"{DOWNLOAD_URL}"}}"#
            )),
        ],
        Some("TYPED42"),
    );
    let resolved = resolver(&host)
        .resolve(free_request())
        .await
        .expect("free download resolves");

    assert_eq!(resolved.url.as_str(), DOWNLOAD_URL);
    // `/geturl` never carries metadata, on the free path no more than on the premium one.
    assert_eq!(resolved.file_name, None);
    assert_eq!(resolved.size, None);
    assert!(resolved.client.account_id.is_none());

    let requests = host.requests.lock().expect("mock lock");
    assert_eq!(
        requests.len(),
        5,
        "probe, requestcaptcha, image, captcha answer, free_download_key"
    );

    // 1. The unauthenticated probe: the file id and nothing else - no token, no captcha fields.
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].url.as_str(), "https://k2s.cc/api/v2/geturl");
    assert_eq!(body_str(&requests[0]), r#"{"file_id":"abcdefghijklm"}"#);

    // 2. `/requestcaptcha` takes an empty body (JD posts an empty map, not the file id).
    assert_eq!(
        requests[1].url.as_str(),
        "https://k2s.cc/api/v2/requestcaptcha"
    );
    assert_eq!(body_str(&requests[1]), "{}");

    // 3. The image is fetched with a plain GET, over https even though the API named http.
    assert_eq!(requests[2].method, "GET");
    assert_eq!(
        requests[2].url.as_str(),
        "https://k2s.cc/api/v2/captcha.html?id=chal-7"
    );

    // 4. The typed answer goes back with the challenge id; no `free_download_key` yet.
    let answer = body_str(&requests[3]);
    assert!(
        answer.contains(r#""captcha_challenge":"chal-7""#),
        "{answer}"
    );
    assert!(
        answer.contains(r#""captcha_response":"TYPED42""#),
        "{answer}"
    );
    assert!(!answer.contains("free_download_key"), "{answer}");

    // 5. The final call carries the `free_download_key` and drops the captcha fields.
    assert_eq!(
        body_str(&requests[4]),
        format!(r#"{{"file_id":"{FILE_ID}","free_download_key":"homeHash"}}"#)
    );

    // The countdown was waited out on the host's clock.
    assert_eq!(*host.waits.lock().expect("mock lock"), vec![30]);

    // The captcha reached the solver as an image, with the bytes and media type just fetched.
    let captchas = host.captchas.lock().expect("mock lock");
    assert_eq!(captchas.len(), 1);
    match &captchas[0] {
        CaptchaChallenge::Image(image) => {
            assert_eq!(image.mime, "image/png");
            assert_eq!(image.data, PNG_BYTES);
            // `/requestcaptcha` supplies no prompt text.
            assert_eq!(image.prompt, None);
        }
        other => panic!("expected an image captcha, got {other:?}"),
    }
}

/// A URL straight from the probe needs neither a captcha nor a wait.
#[tokio::test]
async fn a_probe_that_already_answers_with_a_url_short_circuits_the_flow() {
    let host = MockHost::free(
        vec![geturl_response(&format!(
            r#"{{"status":"success","code":200,"url":"{DOWNLOAD_URL}"}}"#
        ))],
        Some("unused"),
    );
    let resolved = resolver(&host)
        .resolve(free_request())
        .await
        .expect("direct URL");

    assert_eq!(resolved.url.as_str(), DOWNLOAD_URL);
    assert_eq!(host.requests.lock().expect("mock lock").len(), 1);
    assert!(host.captchas.lock().expect("mock lock").is_empty());
    assert!(host.waits.lock().expect("mock lock").is_empty());
}

/// A stated limit must end the resolve as an `IpBlocked` carrying the API's own seconds — and it
/// must do so on the probe, before a captcha has been solved and paid for.
#[tokio::test]
async fn a_stated_limit_becomes_an_ip_block_without_spending_a_captcha() {
    let host = MockHost::free(
        vec![geturl_response(
            r#"{"message":"Download not available","status":"error","code":406,"errorCode":42,"errors":[{"code":5,"timeRemaining":"2521.000000"}]}"#,
        )],
        Some("unused"),
    );
    let failure = resolver(&host)
        .resolve(free_request())
        .await
        .expect_err("a limit must fail the resolve");

    assert_eq!(
        failure.category,
        FailureKind::IpBlocked {
            retry_after_seconds: Some(2521)
        }
    );
    assert_eq!(
        failure.code.as_deref(),
        Some("keep2share.free_limit_reached")
    );
    assert_eq!(
        failure.params.get("wait_seconds").map(String::as_str),
        Some("2521")
    );
    assert_eq!(
        host.requests.lock().expect("mock lock").len(),
        1,
        "the probe alone answers a blocked IP"
    );
    assert!(
        host.captchas.lock().expect("mock lock").is_empty(),
        "no captcha may be spent on a link that is blocked anyway"
    );
    assert!(host.waits.lock().expect("mock lock").is_empty());
}

/// A traffic limit reported without an explicit duration keeps the classifier's own cooldown.
#[tokio::test]
async fn a_traffic_limit_blocks_the_hoster_too() {
    let host = MockHost::free(
        vec![geturl_response(
            r#"{"message":"Traffic limit exceed","status":"error","code":406,"errorCode":2}"#,
        )],
        Some("unused"),
    );
    let failure = resolver(&host)
        .resolve(free_request())
        .await
        .expect_err("a traffic limit must fail");

    assert_eq!(
        failure.category,
        FailureKind::IpBlocked {
            retry_after_seconds: Some(3600)
        }
    );
    assert_eq!(
        failure.code.as_deref(),
        Some("keep2share.free_limit_reached")
    );
}

/// Without a solver the flow must report the host's own captcha failure verbatim, not post the
/// form regardless.
#[tokio::test]
async fn a_captcha_without_a_solver_is_reported_verbatim() {
    let host = MockHost::free(
        vec![
            geturl_response(CAPTCHA_DEMANDED),
            requestcaptcha_response(CAPTCHA_CHALLENGE),
            captcha_image(),
        ],
        None,
    );
    let failure = resolver(&host)
        .resolve(free_request())
        .await
        .expect_err("no solver configured");

    assert_eq!(failure.category, FailureKind::NeedsCaptcha);
    assert_eq!(failure.code.as_deref(), Some("captcha.no_solver"));
    assert_eq!(
        host.requests.lock().expect("mock lock").len(),
        3,
        "no answer may be posted without a solved captcha"
    );
}

/// A rejected answer (errorcode 31) is a `CaptchaFailed`, distinct from "a captcha is required".
#[tokio::test]
async fn a_rejected_captcha_answer_is_reported_as_captcha_failed() {
    let host = MockHost::free(
        vec![
            geturl_response(CAPTCHA_DEMANDED),
            requestcaptcha_response(CAPTCHA_CHALLENGE),
            captcha_image(),
            geturl_response(
                r#"{"message":"Wrong captcha","status":"error","code":406,"errorCode":31}"#,
            ),
        ],
        Some("WRONG"),
    );
    let failure = resolver(&host)
        .resolve(free_request())
        .await
        .expect_err("a rejected answer must fail");

    assert_eq!(failure.category, FailureKind::CaptchaFailed);
    assert_eq!(failure.code.as_deref(), Some("keep2share.captcha_rejected"));
}

/// A countdown longer than JD's 180-second ceiling is a download limit, not something to wait out.
#[tokio::test]
async fn an_excessive_countdown_becomes_an_ip_block() {
    let host = MockHost::free(
        vec![
            geturl_response(CAPTCHA_DEMANDED),
            requestcaptcha_response(CAPTCHA_CHALLENGE),
            captcha_image(),
            geturl_response(
                r#"{"status":"success","code":200,"free_download_key":"homeHash","time_wait":600}"#,
            ),
        ],
        Some("TYPED42"),
    );
    let failure = resolver(&host)
        .resolve(free_request())
        .await
        .expect_err("a 10-minute countdown must not be waited out");

    assert_eq!(
        failure.category,
        FailureKind::IpBlocked {
            retry_after_seconds: Some(600)
        }
    );
    assert_eq!(
        failure.code.as_deref(),
        Some("keep2share.free_limit_reached")
    );
    assert!(
        host.waits.lock().expect("mock lock").is_empty(),
        "the countdown must not be slept through"
    );
}

/// `/requestcaptcha` answering without a usable challenge is a clear coded failure, not a panic
/// and not a request posted with an empty answer.
#[tokio::test]
async fn an_unusable_captcha_challenge_reports_a_coded_failure() {
    let host = MockHost::free(
        vec![
            geturl_response(CAPTCHA_DEMANDED),
            requestcaptcha_response(r#"{"status":"success","code":200,"challenge":"chal-7"}"#),
        ],
        Some("unused"),
    );
    let failure = resolver(&host)
        .resolve(free_request())
        .await
        .expect_err("an incomplete challenge must fail");

    assert_eq!(failure.category, FailureKind::Permanent);
    assert_eq!(
        failure.code.as_deref(),
        Some("keep2share.captcha_unavailable")
    );
    assert!(host.captchas.lock().expect("mock lock").is_empty());
}

/// An HTML error page served where the captcha image belongs must fail before a solver is asked
/// to read it.
#[tokio::test]
async fn a_captcha_url_that_serves_no_image_reports_a_coded_failure() {
    let host = MockHost::free(
        vec![
            geturl_response(CAPTCHA_DEMANDED),
            requestcaptcha_response(CAPTCHA_CHALLENGE),
            json_response(
                200,
                "https://k2s.cc/api/v2/captcha.html?id=chal-7",
                "<html>maintenance</html>",
            ),
        ],
        Some("unused"),
    );
    let failure = resolver(&host)
        .resolve(free_request())
        .await
        .expect_err("a non-image body must fail");

    assert_eq!(failure.category, FailureKind::Permanent);
    assert_eq!(
        failure.code.as_deref(),
        Some("keep2share.captcha_unavailable")
    );
    assert!(host.captchas.lock().expect("mock lock").is_empty());
}

/// A `/geturl` answer that reports neither an error nor a URL is a clear coded failure carrying
/// the API's own wording, not a silently wrong link.
#[tokio::test]
async fn a_response_without_a_url_reports_a_diagnosis() {
    let host = MockHost::free(
        vec![
            geturl_response(CAPTCHA_DEMANDED),
            requestcaptcha_response(CAPTCHA_CHALLENGE),
            captcha_image(),
            geturl_response(r#"{"status":"success","code":200,"message":"Nothing to see here"}"#),
        ],
        Some("TYPED42"),
    );
    let failure = resolver(&host)
        .resolve(free_request())
        .await
        .expect_err("no URL to hand back");

    assert_eq!(failure.category, FailureKind::Permanent);
    assert_eq!(failure.code.as_deref(), Some("keep2share.no_free_link"));
    assert_eq!(
        failure.params.get("diagnosis").map(String::as_str),
        Some("Nothing to see here")
    );
}

/// The free path must be reachable without any account at all — the `requires_account = false`
/// metadata the host dispatches on.
#[test]
fn the_resolver_no_longer_requires_an_account() {
    let host = MockHost::free(Vec::new(), None);
    assert!(!resolver(&host).metadata().requires_account);
}
