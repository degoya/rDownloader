//! The WebAssembly guest adapter, written once for every resolver plugin.
//!
//! Converting between the WIT vocabulary and [`plugin_common`]'s is the same work for every
//! hoster, and so is wiring the four exported functions to a plugin's logic. Doing it here means
//! a plugin's `guest.rs` is three lines, and — more to the point — that the two adapters cannot
//! drift apart, which is how the duplication this whole crate exists to remove began.

#![allow(unsafe_code)] // Generated canonical-ABI exports contain the only unsafe code here.

wit_bindgen::generate!({
    path: "../../crates/rd-plugin-api/wit",
    world: "resolver-plugin",
    // The bindings live here rather than in each plugin, so the export macro has to be usable
    // from another crate and needs a name that says what it exports.
    pub_export_macro: true,
    export_macro_name: "export_resolver",
});

pub use exports::rdownloader::plugin::resolver::Guest;
pub use rdownloader::plugin::types as wit;

use plugin_common::{
    Account, CaptchaAnswer, CaptchaChallenge, CaptchaSolution, ClickPoint, Failure, FailureKind,
    HttpRequest, HttpResponse, LinkCheck, LinkStatus, PluginHost, Resolved, block_on,
};
use rdownloader::plugin::{
    captcha, cookies, host,
    http::{self, RequestHeader, RequestQuery},
};

/// The WIT imports, behind the trait the shared logic is written against.
///
/// Every method is `async` and every one completes immediately: the canonical ABI blocks until
/// the host answers, so these futures are ready on the first poll. That is what lets one piece
/// of async logic serve both builds.
pub struct WitHost;

impl PluginHost for WitHost {
    async fn http(&self, request: HttpRequest) -> Result<HttpResponse, Failure> {
        let query: Vec<RequestQuery> = request
            .query
            .into_iter()
            .map(|value| RequestQuery {
                name: value.name,
                value_template: value.value,
            })
            .collect();
        let headers: Vec<RequestHeader> = request
            .headers
            .into_iter()
            .map(|value| RequestHeader {
                name: value.name,
                value_template: value.value,
            })
            .collect();
        let response = http::http_request(
            &request.method,
            &request.url,
            &query,
            &headers,
            &request.body,
        )
        .map_err(from_wit_failure)?;
        Ok(HttpResponse {
            status: response.status,
            final_url: response.final_url,
            headers: response.headers,
            body: response.body,
        })
    }

    async fn cookies(&self, account_id: &str, url: &str) -> Vec<(String, String)> {
        cookies::cookies_get(account_id, url)
    }

    async fn secret_available(&self, account_id: &str, reference: &str) -> bool {
        host::secret_available(account_id, reference)
    }

    async fn wait(&self, seconds: u32) -> Result<(), Failure> {
        host::wait(seconds).map_err(from_wit_failure)
    }

    async fn solve_captcha(&self, challenge: CaptchaChallenge) -> Result<CaptchaSolution, Failure> {
        let solution =
            captcha::solve_captcha(&to_wit_challenge(challenge)).map_err(from_wit_failure)?;
        Ok(CaptchaSolution {
            token: solution.token,
        })
    }

    async fn solve_challenge(&self, challenge: CaptchaChallenge) -> Result<CaptchaAnswer, Failure> {
        let answer =
            captcha::solve_challenge(&to_wit_challenge(challenge)).map_err(from_wit_failure)?;
        Ok(match answer {
            captcha::CaptchaAnswer::Token(token) => CaptchaAnswer::Token(token),
            captcha::CaptchaAnswer::Point(point) => CaptchaAnswer::Point(ClickPoint {
                x: point.x,
                y: point.y,
            }),
        })
    }

    async fn now_unix_seconds(&self) -> u64 {
        host::now_unix_seconds()
    }

    async fn random_bytes(&self, count: u32) -> Vec<u8> {
        host::random_bytes(count)
    }

    fn log(&self, level: &str, message: &str) {
        host::log(level, message);
    }
}

/// Drives one shared call to completion and converts whatever came back.
///
/// # Errors
///
/// Whatever the plugin's logic reported, in the WIT vocabulary.
pub fn run<T>(
    call: impl std::future::Future<Output = Result<T, Failure>>,
) -> Result<T, wit::Failure> {
    block_on(call).map_err(to_wit_failure)
}

#[must_use]
pub fn to_wit_account(account: Account) -> wit::AccountStatus {
    wit::AccountStatus {
        valid: account.valid,
        premium: account.premium,
        label: account
            .label
            .into_iter()
            .map(|part| wit::LabelPart {
                code: part.code,
                params: part.params,
                message: part.message,
            })
            .collect(),
        traffic_left: account.traffic_left,
    }
}

#[must_use]
pub fn to_wit_resolved(resolved: Resolved, client: wit::ClientIdentity) -> wit::ResolvedDownload {
    let (checksum_algorithm, checksum_value) = match resolved.checksum {
        Some((algorithm, value)) => (Some(algorithm), Some(value)),
        None => (None, None),
    };
    wit::ResolvedDownload {
        url: resolved.url,
        file_name: resolved.file_name,
        size: resolved.size,
        headers: resolved
            .headers
            .into_iter()
            .map(|header| wit::ResolvedHeader {
                name: header.name,
                value: header.value,
            })
            .collect(),
        checksum_algorithm,
        checksum_value,
        client,
    }
}

#[must_use]
pub fn to_wit_checks(results: Vec<LinkCheck>) -> Vec<wit::LinkCheckResult> {
    results
        .into_iter()
        .map(|result| wit::LinkCheckResult {
            url: result.url,
            status: match result.status {
                LinkStatus::Online => wit::LinkStatus::Online,
                LinkStatus::Offline => wit::LinkStatus::Offline,
                LinkStatus::Unknown => wit::LinkStatus::Unknown,
                LinkStatus::Cached => wit::LinkStatus::Cached,
            },
            file_name: result.file_name,
            size: result.size,
        })
        .collect()
}

fn to_wit_challenge(challenge: CaptchaChallenge) -> captcha::CaptchaChallenge {
    fn widget(value: plugin_common::WidgetChallenge) -> captcha::WidgetChallenge {
        captcha::WidgetChallenge {
            site_key: value.site_key,
            page_url: value.page_url,
            invisible: value.invisible,
        }
    }
    fn picture(value: plugin_common::ImageChallenge) -> captcha::ImageChallenge {
        captcha::ImageChallenge {
            mime: value.mime,
            data: value.data,
            prompt: value.prompt,
        }
    }
    match challenge {
        CaptchaChallenge::RecaptchaV2(value) => {
            captcha::CaptchaChallenge::RecaptchaV2(widget(value))
        }
        CaptchaChallenge::HCaptcha(value) => captcha::CaptchaChallenge::Hcaptcha(widget(value)),
        CaptchaChallenge::Turnstile(value) => captcha::CaptchaChallenge::Turnstile(widget(value)),
        CaptchaChallenge::Image(value) => captcha::CaptchaChallenge::Image(picture(value)),
        CaptchaChallenge::ClickPoint(value) => {
            captcha::CaptchaChallenge::ClickPoint(picture(value))
        }
        CaptchaChallenge::Cutcaptcha(value) => {
            captcha::CaptchaChallenge::Cutcaptcha(captcha::CutcaptchaChallenge {
                site_key: value.site_key,
                misery_key: value.misery_key,
                page_url: value.page_url,
            })
        }
    }
}

#[must_use]
pub fn to_wit_failure(failure: Failure) -> wit::Failure {
    wit::Failure {
        category: to_wit_kind(failure.kind),
        message: failure.message,
        code: failure.code,
        params: failure.params,
    }
}

fn to_wit_kind(kind: FailureKind) -> wit::FailureKind {
    match kind {
        FailureKind::Transient(seconds) => wit::FailureKind::Transient(seconds),
        FailureKind::Permanent => wit::FailureKind::Permanent,
        FailureKind::Offline => wit::FailureKind::Offline,
        FailureKind::AuthRequired => wit::FailureKind::AuthRequired,
        FailureKind::AccountInvalid => wit::FailureKind::AccountInvalid,
        FailureKind::RateLimited(seconds) => wit::FailureKind::RateLimited(seconds),
        FailureKind::NeedsCaptcha => wit::FailureKind::NeedsCaptcha,
        FailureKind::Unsupported => wit::FailureKind::Unsupported,
        FailureKind::IpBlocked(seconds) => wit::FailureKind::IpBlocked(seconds),
        FailureKind::CaptchaFailed => wit::FailureKind::CaptchaFailed,
    }
}

fn from_wit_failure(failure: wit::Failure) -> Failure {
    Failure {
        kind: match failure.category {
            wit::FailureKind::Transient(seconds) => FailureKind::Transient(seconds),
            wit::FailureKind::Permanent => FailureKind::Permanent,
            wit::FailureKind::Offline => FailureKind::Offline,
            wit::FailureKind::AuthRequired => FailureKind::AuthRequired,
            wit::FailureKind::AccountInvalid => FailureKind::AccountInvalid,
            wit::FailureKind::RateLimited(seconds) => FailureKind::RateLimited(seconds),
            wit::FailureKind::NeedsCaptcha => FailureKind::NeedsCaptcha,
            wit::FailureKind::Unsupported => FailureKind::Unsupported,
            wit::FailureKind::IpBlocked(seconds) => FailureKind::IpBlocked(seconds),
            wit::FailureKind::CaptchaFailed => FailureKind::CaptchaFailed,
        },
        message: failure.message,
        code: failure.code,
        params: failure.params,
    }
}

/// Exports a plugin whose logic module offers `matches`, `check_account`, `resolve`, `check`
/// and `hosters`.
///
/// The four exported functions are the same delegation for every hoster, so writing them out
/// per plugin would only create somewhere for the two builds to differ again.
#[macro_export]
macro_rules! resolver_plugin {
    ($logic:path) => {
        struct Component;

        impl $crate::Guest for Component {
            fn match_url(url: String) -> bool {
                use $logic as logic;
                logic::matches(&url)
            }

            fn check_account(
                account_id: String,
            ) -> Result<$crate::wit::AccountStatus, $crate::wit::Failure> {
                use $logic as logic;
                $crate::run(logic::check_account(&$crate::WitHost, &account_id))
                    .map($crate::to_wit_account)
            }

            fn resolve(
                request: $crate::wit::ResolveRequest,
            ) -> Result<$crate::wit::ResolvedDownload, $crate::wit::Failure> {
                use $logic as logic;
                let client = request.client.clone();
                let input = ::plugin_common::ResolveInput {
                    url: request.url,
                    account_id: request.client.account_id,
                };
                $crate::run(logic::resolve(&$crate::WitHost, &input))
                    .map(|resolved| $crate::to_wit_resolved(resolved, client))
            }

            fn check(
                request: $crate::wit::CheckRequest,
            ) -> Result<Vec<$crate::wit::LinkCheckResult>, $crate::wit::Failure> {
                use $logic as logic;
                let input = ::plugin_common::CheckInput {
                    urls: request.urls,
                    account_id: request.client.account_id,
                };
                $crate::run(logic::check(&$crate::WitHost, &input)).map($crate::to_wit_checks)
            }

            fn hosters(account_id: String) -> Result<Vec<String>, $crate::wit::Failure> {
                use $logic as logic;
                $crate::run(logic::hosters(&$crate::WitHost, &account_id))
            }
        }

        $crate::export_resolver!(Component with_types_in $crate);
    };
}
