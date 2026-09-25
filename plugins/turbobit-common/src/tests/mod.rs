//! The shared logic driven against a scripted host, for both brands.
//!
//! The host here implements [`PluginHost`] directly — no `rd-plugin-api`, no runtime — and
//! every future it returns is ready on the first poll, so `plugin_common::block_on` drives the
//! flows exactly as the WebAssembly guest would. Fixtures are the sanitised measurements under
//! `plugins/turbobit/tests/fixtures` and `plugins/hitfile/tests/fixtures`; each test says
//! which.

use std::cell::RefCell;
use std::collections::VecDeque;

use plugin_common::{
    CaptchaAnswer, CaptchaChallenge, CaptchaSolution, Failure, FailureKind, HttpRequest,
    HttpResponse, PluginHost, block_on,
};

use crate::brand::{Brand, Codes, IdRule};

mod account_tests;
mod api_tests;
mod check_tests;
mod free_tests;
mod link_tests;

/// Turbobit, spelled the way `plugins/turbobit` spells it.
pub(crate) const TURBOBIT: Brand = Brand {
    name: "Turbobit",
    site_host: "turbobit.net",
    app_host: "app.turbobit.net",
    match_hosts: &[
        "turbobit.net",
        "www.turbobit.net",
        "new.turbobit.net",
        "m.turbobit.net",
        "turbobit.cc",
        "turb.cc",
        "turb.pw",
        "turbo.to",
        "trbt.cc",
    ],
    id: IdRule {
        min: 10,
        max: 12,
        lowercase_only: true,
    },
    bare_id_path: false,
    html_suffix: true,
    password_reference: "turbobit_password",
    codes: TURBOBIT_CODES,
};

/// HitFile, spelled the way `plugins/hitfile` spells it.
pub(crate) const HITFILE: Brand = Brand {
    name: "HitFile",
    site_host: "hitfile.net",
    app_host: "app.hitfile.net",
    match_hosts: &[
        "hitfile.net",
        "www.hitfile.net",
        "new.hitfile.net",
        "hitfile.ru",
        "hil.to",
        "hitf.cc",
        "htfl.net",
        "hitf.to",
    ],
    id: IdRule {
        min: 4,
        max: 7,
        lowercase_only: false,
    },
    bare_id_path: true,
    html_suffix: false,
    password_reference: "hitfile_password",
    codes: HITFILE_CODES,
};

/// The Turbobit codes, spelled as `plugins/turbobit/src/messages.rs` spells them.
const TURBOBIT_CODES: Codes = Codes {
    unsupported_link: "turbobit.unsupported_link",
    invalid_link: "turbobit.invalid_link",
    folder_not_file: "turbobit.folder_not_file",
    invalid_response: "turbobit.invalid_response",
    http_error: "turbobit.http_error",
    api_error: "turbobit.api_error",
    rate_limited: "turbobit.rate_limited",
    file_unavailable: "turbobit.file_unavailable",
    premium_only: "turbobit.premium_only",
    free_limit_reached: "turbobit.free_limit_reached",
    captcha_rejected: "turbobit.captcha_rejected",
    no_direct_link: "turbobit.no_direct_link",
    invalid_url: "turbobit.invalid_url",
    account_missing: "turbobit.account_missing",
    not_signed_in: "turbobit.not_signed_in",
    login_failed: "turbobit.login_failed",
    login_captcha: "turbobit.login_captcha",
    account_banned: "turbobit.account_banned",
    premium_limit_reached: "turbobit.premium_limit_reached",
};

/// The HitFile codes, spelled as `plugins/hitfile/src/messages.rs` spells them.
const HITFILE_CODES: Codes = Codes {
    unsupported_link: "hitfile.unsupported_link",
    invalid_link: "hitfile.invalid_link",
    folder_not_file: "hitfile.folder_not_file",
    invalid_response: "hitfile.invalid_response",
    http_error: "hitfile.http_error",
    api_error: "hitfile.api_error",
    rate_limited: "hitfile.rate_limited",
    file_unavailable: "hitfile.file_unavailable",
    premium_only: "hitfile.premium_only",
    free_limit_reached: "hitfile.free_limit_reached",
    captcha_rejected: "hitfile.captcha_rejected",
    no_direct_link: "hitfile.no_direct_link",
    invalid_url: "hitfile.invalid_url",
    account_missing: "hitfile.account_missing",
    not_signed_in: "hitfile.not_signed_in",
    login_failed: "hitfile.login_failed",
    login_captcha: "hitfile.login_captcha",
    account_banned: "hitfile.account_banned",
    premium_limit_reached: "hitfile.premium_limit_reached",
};

/// A scripted host: answers in the order queued, records what was asked of it.
pub(crate) struct MockHost {
    responses: RefCell<VecDeque<HttpResponse>>,
    pub(crate) requests: RefCell<Vec<HttpRequest>>,
    pub(crate) waits: RefCell<Vec<u32>>,
    pub(crate) captchas: RefCell<Vec<CaptchaChallenge>>,
    /// The token every captcha is answered with; `None` mimics a host with no solver.
    captcha_token: Option<String>,
    has_secret: bool,
    now: u64,
}

impl MockHost {
    pub(crate) fn new(responses: Vec<HttpResponse>) -> Self {
        Self {
            responses: RefCell::new(responses.into()),
            requests: RefCell::new(Vec::new()),
            waits: RefCell::new(Vec::new()),
            captchas: RefCell::new(Vec::new()),
            captcha_token: Some("turnstile-token".to_owned()),
            has_secret: false,
            now: 1_800_000_000,
        }
    }

    pub(crate) fn without_solver(mut self) -> Self {
        self.captcha_token = None;
        self
    }

    pub(crate) fn with_password(mut self) -> Self {
        self.has_secret = true;
        self
    }

    pub(crate) fn request_count(&self) -> usize {
        self.requests.borrow().len()
    }

    pub(crate) fn request(&self, index: usize) -> HttpRequest {
        self.requests.borrow()[index].clone()
    }

    pub(crate) fn urls(&self) -> Vec<String> {
        self.requests
            .borrow()
            .iter()
            .map(|request| request.url.clone())
            .collect()
    }
}

impl PluginHost for MockHost {
    async fn http(&self, request: HttpRequest) -> Result<HttpResponse, Failure> {
        self.requests.borrow_mut().push(request);
        self.responses.borrow_mut().pop_front().ok_or_else(|| {
            Failure::coded(
                FailureKind::Permanent,
                "test.no_response",
                "the script ran out of answers",
            )
        })
    }

    async fn cookies(&self, _account_id: &str, _url: &str) -> Vec<(String, String)> {
        Vec::new()
    }

    async fn secret_available(&self, _account_id: &str, reference: &str) -> bool {
        self.has_secret && reference.ends_with("_password")
    }

    async fn wait(&self, seconds: u32) -> Result<(), Failure> {
        self.waits.borrow_mut().push(seconds);
        Ok(())
    }

    async fn solve_captcha(&self, challenge: CaptchaChallenge) -> Result<CaptchaSolution, Failure> {
        match self.solve_challenge(challenge).await? {
            CaptchaAnswer::Token(token) => Ok(CaptchaSolution { token }),
            CaptchaAnswer::Point(_) => unreachable!("no click-point challenge here"),
        }
    }

    async fn solve_challenge(&self, challenge: CaptchaChallenge) -> Result<CaptchaAnswer, Failure> {
        self.captchas.borrow_mut().push(challenge);
        match &self.captcha_token {
            Some(token) => Ok(CaptchaAnswer::Token(token.clone())),
            None => Err(Failure::coded(
                FailureKind::NeedsCaptcha,
                "captcha.no_solver",
                "No captcha solver is configured",
            )),
        }
    }

    async fn now_unix_seconds(&self) -> u64 {
        self.now
    }

    async fn random_bytes(&self, count: u32) -> Vec<u8> {
        vec![7; count as usize]
    }

    fn log(&self, _level: &str, _message: &str) {}
}

/// A JSON answer with the given status.
pub(crate) fn json(status: u16, body: &str) -> HttpResponse {
    HttpResponse {
        status,
        final_url: "https://app.example.test/api".to_owned(),
        headers: vec![("content-type".to_owned(), "application/json".to_owned())],
        body: body.as_bytes().to_vec(),
    }
}

/// An HTML answer with the given status: the SPA shell, a redirect page, an error page.
pub(crate) fn html(status: u16, body: &str) -> HttpResponse {
    HttpResponse {
        status,
        final_url: "https://app.example.test/api".to_owned(),
        headers: vec![(
            "content-type".to_owned(),
            "text/html; charset=UTF-8".to_owned(),
        )],
        body: body.as_bytes().to_vec(),
    }
}

pub(crate) fn body_of(request: &HttpRequest) -> String {
    String::from_utf8_lossy(&request.body).into_owned()
}

pub(crate) fn header_of<'a>(request: &'a HttpRequest, name: &str) -> Option<&'a str> {
    request
        .headers
        .iter()
        .find(|header| header.name.eq_ignore_ascii_case(name))
        .map(|header| header.value.as_str())
}

pub(crate) fn run<T>(future: impl std::future::Future<Output = T>) -> T {
    block_on(future)
}

pub(crate) fn code(failure: &Failure) -> &str {
    failure.code.as_deref().unwrap_or("")
}

// Turbobit fixtures, measured 2026-09-21 unless named `synthetic`.
pub(crate) mod tb {
    macro_rules! fixture {
        ($name:ident, $file:literal) => {
            pub(crate) const $name: &str =
                include_str!(concat!("../../../turbobit/tests/fixtures/", $file));
        };
    }
    fixture!(SHELL, "spa-shell-2026-09-21.html");
    fixture!(CAPTCHA, "captcha-2026-09-21.json");
    fixture!(LINKS_CHECK, "links-check-2026-09-21.json");
    fixture!(INFO_FREE, "download-info-free-2026-09-21.json");
    fixture!(INFO_DELETED, "download-info-deleted-404-2026-09-21.json");
    fixture!(INIT_DIRECT_HIT, "free-init-direct-hit-2026-09-21.json");
    fixture!(INIT_NOT_FOUND, "free-init-not-found-404-2026-09-21.json");
    fixture!(START_NOT_FOUND, "free-start-not-found-404-2026-09-21.json");
    fixture!(CAPTCHA_INVALID, "free-captcha-invalid-422-2026-09-21.json");
    fixture!(
        REDIRECT_PAGE,
        "free-captcha-without-accept-302-2026-09-21.html"
    );
    fixture!(
        USER_INFO_401,
        "user-info-unauthenticated-401-2026-09-21.json"
    );
    fixture!(INIT_OK, "free-init-ok-synthetic.json");
    fixture!(INIT_IP_BAN, "free-init-ip-ban-synthetic.json");
    fixture!(CAPTCHA_DELAY, "free-captcha-delay-synthetic.json");
    fixture!(PREPARE_OK, "free-prepare-ok-synthetic.json");
    fixture!(START_OK, "free-start-ok-synthetic.json");
    fixture!(START_PERCENT, "free-start-percent-name-synthetic.json");
    fixture!(START_FOREIGN, "free-start-foreign-host-synthetic.json");
    fixture!(START_NO_URL, "free-start-no-url-synthetic.json");
    fixture!(INFO_PREMIUM, "download-info-premium-synthetic.json");
    fixture!(
        INFO_PREMIUM_NO_URLS,
        "download-info-premium-no-urls-synthetic.json"
    );
    fixture!(USER_ACTIVE, "user-info-premium-active-synthetic.json");
    fixture!(USER_EXPIRED, "user-info-premium-expired-synthetic.json");
    fixture!(USER_BANNED, "user-info-banned-synthetic.json");
    fixture!(PREMIUM_INFO, "premium-info-synthetic.json");
    fixture!(
        LOGIN_NEED_CAPTCHA,
        "auth-login-need-captcha-422-synthetic.json"
    );
    fixture!(
        LOGIN_PASSWORD_INCORRECT,
        "auth-login-password-incorrect-422-synthetic.json"
    );
    fixture!(
        LOGIN_INVALID_CAPTCHA,
        "auth-login-invalid-captcha-422-synthetic.json"
    );
    fixture!(LOGIN_OK, "auth-login-ok-synthetic.json");
}

// HitFile fixtures, measured 2026-09-21 unless named otherwise.
pub(crate) mod hf {
    macro_rules! fixture {
        ($name:ident, $file:literal) => {
            pub(crate) const $name: &str =
                include_str!(concat!("../../../hitfile/tests/fixtures/", $file));
        };
    }
    fixture!(SHELL, "spa-shell-2026-09-21.html");
    fixture!(CAPTCHA, "captcha-2026-09-21.json");
    fixture!(LINKS_CHECK, "links-check-mixed-2026-09-21.json");
    fixture!(INFO_FREE, "download-info-free-2026-09-21.json");
    fixture!(
        INFO_PREMIUM_ONLY,
        "download-info-premium-only-2026-09-21.json"
    );
    fixture!(INFO_DELETED, "download-info-deleted-404-2026-09-21.json");
    fixture!(
        INIT_DIRECT_HIT,
        "free-init-direct-hit-premium-only-2026-09-21.json"
    );
    fixture!(START_409, "free-start-premium-only-409-2026-09-21.json");
    fixture!(
        START_400_FEASIBILITY,
        "free-start-premium-only-400-feasibility.json"
    );
    fixture!(START_NOT_FOUND, "free-start-not-found-404-2026-09-21.json");
    fixture!(CAPTCHA_INVALID, "free-captcha-invalid-422-2026-09-21.json");
    fixture!(ERROR_PAGE, "free-start-without-accept-409-2026-09-21.html");
    fixture!(INIT_OK, "free-init-ok-synthetic.json");
    fixture!(CAPTCHA_DELAY, "free-captcha-delay-synthetic.json");
    fixture!(PREPARE_OK, "free-prepare-ok-synthetic.json");
    fixture!(START_OK, "free-start-ok-synthetic.json");
}
