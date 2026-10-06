use super::{
    ApiFailure, ErrorKind, HttpError, HttpWords, SecretSlot, call, coded, diagnosed, free_limit,
    invalid_url, require_account, require_secret,
};
use crate::{
    CaptchaAnswer, CaptchaChallenge, CaptchaSolution, Failure, FailureKind, HttpRequest,
    HttpResponse, PluginHost, block_on,
};

fn http_error(status: u16) -> String {
    format!("Example HTTP status {status}")
}

const WORDS: HttpWords = HttpWords {
    unauthorized: ("example.auth_invalid", "The key was refused"),
    gone: ("example.file_gone", "The file is gone"),
    unavailable: ("example.refused", "The provider refused it"),
    rate_limited: ("example.rate_limited", "Too many requests"),
    server_error: ("example.server_error", "The provider failed"),
    rate_limited_wait: Some(60),
    server_error_wait: None,
    other: HttpError {
        code: "example.http_error",
        text: http_error,
    },
};

/// Each status class in the plugin's words, in the kind every plugin shares.
#[test]
fn each_status_class_is_named_in_the_plugins_words() {
    assert_eq!(WORDS.ensure_http_status(200, None), Ok(()));
    let cases = [
        (401, ErrorKind::AccountInvalid, "example.auth_invalid"),
        (403, ErrorKind::AccountInvalid, "example.auth_invalid"),
        (404, ErrorKind::Permanent, "example.file_gone"),
        (410, ErrorKind::Permanent, "example.file_gone"),
        (451, ErrorKind::Offline, "example.refused"),
        (
            429,
            ErrorKind::RateLimited(Some(60)),
            "example.rate_limited",
        ),
        (503, ErrorKind::Transient(None), "example.server_error"),
    ];
    for (status, kind, code) in cases {
        let refusal = WORDS.ensure_http_status(status, None).expect_err("refused");
        assert_eq!(refusal.kind, kind, "{status}");
        assert_eq!(refusal.code, code, "{status}");
        assert!(refusal.params.is_empty(), "{status}");
    }
}

/// A stated wait wins over the plugin's default; the default fills only the silence.
#[test]
fn a_stated_wait_wins_over_the_default() {
    assert_eq!(
        WORDS
            .ensure_http_status(429, Some(120))
            .expect_err("429")
            .kind,
        ErrorKind::RateLimited(Some(120))
    );
    assert_eq!(
        WORDS
            .ensure_http_status(502, Some(30))
            .expect_err("502")
            .kind,
        ErrorKind::Transient(Some(30))
    );
    // `0` is no wait at all, so the default stands in for it.
    assert_eq!(
        WORDS
            .ensure_http_status(429, Some(0))
            .expect_err("429")
            .kind,
        ErrorKind::RateLimited(Some(60))
    );
}

#[test]
fn any_other_status_is_permanent_and_names_itself() {
    let refusal = WORDS.ensure_http_status(418, None).expect_err("418");
    assert_eq!(refusal.kind, ErrorKind::Permanent);
    assert_eq!(refusal.code, "example.http_error");
    assert_eq!(refusal.message, "Example HTTP status 418");
    assert_eq!(refusal.params, vec![("status", "418".to_owned())]);
}

/// One code for every class, each in its own kind.
#[test]
fn a_uniform_mapping_keeps_each_class_kind() {
    let uniform = WORDS.other;
    assert_eq!(uniform.ensure_http_status(204, None), Ok(()));
    let limited = uniform.ensure_http_status(429, Some(40)).expect_err("429");
    assert_eq!(limited.kind, ErrorKind::RateLimited(Some(40)));
    assert_eq!(limited.code, "example.http_error");
    assert_eq!(limited.params, vec![("status", "429".to_owned())]);
    for (status, kind) in [
        (403, ErrorKind::AccountInvalid),
        (410, ErrorKind::Permanent),
        (451, ErrorKind::Offline),
        (418, ErrorKind::Permanent),
    ] {
        assert_eq!(
            uniform
                .ensure_http_status(status, None)
                .expect_err("refused")
                .kind,
            kind,
            "{status}"
        );
    }
}

/// The provider's own code travels as `api_code`, whatever type the provider spells it in.
#[test]
fn a_provider_code_travels_as_a_parameter() {
    let word = ApiFailure::with_api_code(ErrorKind::Offline, ("example.gone", "Gone"), "LINK_DOWN");
    assert_eq!(word.params, vec![("api_code", "LINK_DOWN".to_owned())]);
    let number = ApiFailure::with_api_code(ErrorKind::Offline, ("example.gone", "Gone"), 35_i64);
    assert_eq!(number.params, vec![("api_code", "35".to_owned())]);
    assert_eq!(number.param("api_code"), Some("35"));
    assert_eq!(number.param("reason"), None);
}

/// The conversion is a move: kind, code, text and every parameter, in order.
#[test]
fn the_conversion_keeps_everything_in_order() {
    let failure: Failure = ApiFailure::new(
        ErrorKind::IpBlocked(Some(30)),
        ("example.ip", "Not from this address"),
    )
    .with_param("api_code", "22")
    .with_param("reason", "vpn")
    .into();
    assert_eq!(
        failure,
        coded(
            FailureKind::IpBlocked(Some(30)),
            ("example.ip", "Not from this address")
        )
        .with_param("api_code", "22")
        .with_param("reason", "vpn")
    );
}

/// Without an account a refused credential is no refused account; every other class keeps its
/// words and kind.
#[test]
fn without_an_account_a_refusal_is_a_plain_http_error() {
    for status in [401, 403] {
        let words = WORDS
            .ensure_without_account(status, None)
            .expect_err("a refusal");
        assert_eq!(words.kind, ErrorKind::Permanent, "{status}");
        assert_eq!(words.code, "example.http_error");
        assert_eq!(words.param("status"), Some(status.to_string().as_str()));
        let error = WORDS
            .other
            .ensure_without_account(status, None)
            .expect_err("a refusal");
        assert_eq!(error, words);
    }
    assert_eq!(WORDS.ensure_without_account(200, None), Ok(()));
    let limited = WORDS
        .ensure_without_account(429, None)
        .expect_err("a refusal");
    assert_eq!(limited.kind, ErrorKind::RateLimited(Some(60)));
    assert_eq!(limited.code, "example.rate_limited");
    let gone = WORDS
        .other
        .ensure_without_account(404, None)
        .expect_err("a refusal");
    assert_eq!(gone.kind, ErrorKind::Permanent);
    assert_eq!(gone.code, "example.http_error");
}

fn no_link(diagnosis: &str) -> String {
    format!("No link: {diagnosis}")
}

fn limit(seconds: Option<u64>) -> String {
    format!("Limit {seconds:?}")
}

/// The page's diagnosis is both in the text and a parameter of its own.
#[test]
fn a_diagnosed_dead_end_carries_its_diagnosis() {
    let failure = diagnosed(
        FailureKind::Permanent,
        "example.no_free_link",
        no_link,
        "login page".to_owned(),
    );
    assert_eq!(failure.kind, FailureKind::Permanent);
    assert_eq!(failure.code.as_deref(), Some("example.no_free_link"));
    assert_eq!(failure.message, "No link: login page");
    assert_eq!(
        failure.params,
        vec![("diagnosis".to_owned(), "login page".to_owned())]
    );
}

/// A stated wait travels as `wait_seconds`; a limit without one leaves the wait to the
/// scheduler, and no limit is no failure.
#[test]
fn a_free_limit_keeps_only_a_stated_wait() {
    assert_eq!(free_limit(None, "example.limit", limit), Ok(()));
    let timed = free_limit(Some(90), "example.limit", limit).expect_err("a limit");
    assert_eq!(timed.kind, FailureKind::IpBlocked(Some(90)));
    assert_eq!(timed.message, "Limit Some(90)");
    assert_eq!(
        timed.params,
        vec![("wait_seconds".to_owned(), "90".to_owned())]
    );
    let open = free_limit(Some(0), "example.limit", limit).expect_err("a limit");
    assert_eq!(open.kind, FailureKind::IpBlocked(None));
    assert_eq!(open.code.as_deref(), Some("example.limit"));
    assert!(open.params.is_empty());
}

struct Answering(u16, Vec<(String, String)>);

impl PluginHost for Answering {
    async fn http(&self, request: HttpRequest) -> Result<HttpResponse, Failure> {
        Ok(HttpResponse {
            status: self.0,
            final_url: request.url,
            headers: self.1.clone(),
            body: b"{}".to_vec(),
        })
    }

    async fn cookies(&self, _account_id: &str, _url: &str) -> Vec<(String, String)> {
        Vec::new()
    }

    /// Every slot holds a secret but the empty one, which stands for a slot never filled.
    async fn secret_available(&self, _account_id: &str, reference: &str) -> bool {
        !reference.is_empty()
    }

    async fn wait(&self, _seconds: u32) -> Result<(), Failure> {
        Ok(())
    }

    async fn solve_captcha(
        &self,
        _challenge: CaptchaChallenge,
    ) -> Result<CaptchaSolution, Failure> {
        Err(coded(
            FailureKind::Unsupported,
            ("example.captcha", "No captcha"),
        ))
    }

    async fn solve_challenge(
        &self,
        _challenge: CaptchaChallenge,
    ) -> Result<CaptchaAnswer, Failure> {
        Err(coded(
            FailureKind::Unsupported,
            ("example.captcha", "No captcha"),
        ))
    }

    async fn now_unix_seconds(&self) -> u64 {
        0
    }

    async fn random_bytes(&self, count: u32) -> Vec<u8> {
        vec![0; count as usize]
    }

    fn log(&self, _level: &str, _message: &str) {}
}

/// `call` hands the refusal reader the status, the shared `Retry-After` and the body, and
/// converts whatever it names.
#[test]
fn call_turns_a_named_refusal_into_the_failure() {
    let host = Answering(429, vec![("Retry-After".to_owned(), "90".to_owned())]);
    let refused = block_on(call(
        &host,
        HttpRequest::get("https://example.test/"),
        |status, wait, body| {
            assert_eq!(body, b"{}");
            WORDS.ensure_http_status(status, wait).err()
        },
    ))
    .expect_err("429");
    assert_eq!(refused.kind, FailureKind::RateLimited(Some(90)));
    assert_eq!(refused.code.as_deref(), Some("example.rate_limited"));

    let answered = block_on(call(
        &Answering(200, Vec::new()),
        HttpRequest::get("https://example.test/"),
        |status, wait, _| WORDS.ensure_http_status(status, wait).err(),
    ))
    .expect("200");
    assert_eq!(answered.status, 200);
}

/// An address that does not parse is permanent, under the plugin's code, with the parser's words
/// as the `error` parameter and in the text.
#[test]
fn an_unparsable_address_names_the_parsers_words() {
    let failure = invalid_url("example.invalid_url", &"relative URL without a base");
    assert_eq!(failure.kind, ErrorKind::Permanent);
    assert_eq!(failure.code, "example.invalid_url");
    assert_eq!(
        failure.message,
        "Invalid provider URL: relative URL without a base"
    );
    assert_eq!(failure.param("error"), Some("relative URL without a base"));
}

const ACCOUNT_MISSING: (&str, &str) = ("example.account_missing", "No account");

/// No account and an empty id are both refused as a missing account; an id passes unchanged.
#[test]
fn an_empty_account_id_is_no_account() {
    assert_eq!(require_account(Some("a1"), ACCOUNT_MISSING), Ok("a1"));
    for missing in [None, Some("")] {
        let failure = require_account(missing, ACCOUNT_MISSING).expect_err("refused");
        assert_eq!(failure.kind, FailureKind::AuthRequired);
        assert_eq!(failure.code.as_deref(), Some("example.account_missing"));
    }
}

/// A secret slot that holds nothing is refused before any request, under the plugin's words.
#[test]
fn a_missing_secret_is_refused_before_a_request() {
    let host = Answering(200, Vec::new());
    let missing = ("example.key_missing", "No key");
    let stored = SecretSlot {
        reference: "key",
        missing,
    };
    assert_eq!(block_on(require_secret(&host, "a1", stored)), Ok(()));
    let empty = SecretSlot {
        reference: "",
        missing,
    };
    let failure = block_on(require_secret(&host, "a1", empty)).expect_err("refused");
    assert_eq!(failure.kind, FailureKind::AuthRequired);
    assert_eq!(failure.code.as_deref(), Some("example.key_missing"));
    assert_eq!(failure.message, "No key");
}
