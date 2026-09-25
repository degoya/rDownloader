//! From the file page to the direct link.
//!
//! One `GET` of `/file/<key>` with a one-byte range — the page ignores it (measured), and a
//! layout that redirected straight to the file would then hand back one byte rather than the
//! whole file through the plugin's response cap. What comes back is read in the order JD's
//! `MediafireCom.handleDownload` reads it: the error page it was redirected to, the malware
//! advisory, the per-IP threshold, the password form, a wait, then the button, then a captcha
//! form that is answered once and once only. Nothing here retries with a different identity:
//! a threshold is reported as a wait, a refusal as a failure.

use mediafire_common::address;
use plugin_common::{
    CaptchaAnswer, CaptchaChallenge, Failure, FailureKind, HttpRequest, PluginHost, WidgetChallenge,
};

use super::api::{coded, errno_failure, http_failure};
use crate::{messages, page};

/// The direct link behind `key`'s file page.
pub(crate) async fn direct_link<H: PluginHost>(host: &H, key: &str) -> Result<String, Failure> {
    let mut response = host.http(probe(address::file_page(key))).await?;
    let mut captchas_answered = 0;
    loop {
        if let Some(errno) = address::error_number(&response.final_url) {
            return Err(errno_failure(errno));
        }
        if response.final_url.contains("/download_repair.php") {
            return Err(temporarily_unavailable(60));
        }
        if !(200..=299).contains(&response.status) {
            return Err(http_failure(response.status));
        }
        if !page::is_html(&response) {
            // The page redirected to the file itself. Accept a delivery host and nothing
            // else: the transfer would refuse another host anyway, and this way the failure
            // says what was found.
            return delivered(&response.final_url);
        }
        let html = response.text().into_owned();
        if page::has_malware_advisory(&html) {
            return Err(coded(FailureKind::Permanent, messages::MALWARE_FLAGGED));
        }
        if let Some(seconds) = page::threshold_seconds(&html) {
            return Err(threshold(seconds));
        }
        if page::has_password_form(&html) {
            return Err(coded(FailureKind::Permanent, messages::PASSWORD_REQUIRED));
        }
        if let Some(seconds) = page::retry_seconds(&html) {
            return Err(temporarily_unavailable(seconds));
        }
        if let Some(link) = page::direct_link(&html) {
            return Ok(link);
        }
        let Some(form) = page::captcha_form(&html) else {
            let diagnosis = page::diagnose(&html);
            return Err(Failure::coded(
                FailureKind::Permanent,
                messages::NO_DIRECT_LINK,
                messages::no_direct_link(&diagnosis),
            )
            .with_param("diagnosis", diagnosis));
        };
        if captchas_answered > 0 {
            return Err(coded(
                FailureKind::CaptchaFailed,
                messages::CAPTCHA_REJECTED,
            ));
        }
        captchas_answered += 1;
        let fields = answered(host, form, &response.final_url).await?;
        response = host
            .http(
                HttpRequest::post(response.final_url.clone(), page::encode_form(&fields))
                    .with_header("Content-Type", "application/x-www-form-urlencoded")
                    .with_header("Range", "bytes=0-0"),
            )
            .await?;
    }
}

/// The captcha form's fields with the answer added, or the refusal when nobody can answer.
async fn answered<H: PluginHost>(
    host: &H,
    form: page::CaptchaForm,
    page_url: &str,
) -> Result<Vec<(String, String)>, Failure> {
    let mut fields = form.fields;
    match form.kind {
        page::CaptchaKind::Recaptcha { site_key } => {
            let challenge = CaptchaChallenge::RecaptchaV2(WidgetChallenge {
                site_key,
                page_url: page_url.to_owned(),
                invisible: false,
            });
            match host.solve_challenge(challenge).await? {
                CaptchaAnswer::Token(token) => {
                    fields.push(("g-recaptcha-response".to_owned(), token));
                }
                CaptchaAnswer::Point(_) => {
                    return Err(coded(FailureKind::NeedsCaptcha, messages::CAPTCHA_REQUIRED));
                }
            }
        }
        page::CaptchaKind::Checkbox => {
            fields.push(("mf_captcha_response".to_owned(), "1".to_owned()));
        }
        page::CaptchaKind::Unknown => {
            return Err(coded(FailureKind::NeedsCaptcha, messages::CAPTCHA_REQUIRED));
        }
    }
    Ok(fields)
}

/// A non-HTML answer: the file, when it came from a delivery host.
fn delivered(final_url: &str) -> Result<String, Failure> {
    let parsed = url::Url::parse(final_url).map_err(|error| super::api::invalid_url(&error))?;
    let host = parsed.host_str().unwrap_or_default();
    if address::is_download_host(host) {
        return Ok(parsed.to_string());
    }
    let diagnosis = format!("a non-HTML answer from {host}");
    Err(Failure::coded(
        FailureKind::Permanent,
        messages::NO_DIRECT_LINK,
        messages::no_direct_link(&diagnosis),
    )
    .with_param("diagnosis", diagnosis))
}

/// The file page request: one byte of range, so a redirect to the file costs one byte.
fn probe(url: String) -> HttpRequest {
    HttpRequest::get(url).with_header("Range", "bytes=0-0")
}

fn threshold(seconds: u64) -> Failure {
    Failure::coded(
        FailureKind::IpBlocked((seconds > 0).then_some(seconds)),
        messages::DOWNLOAD_LIMIT_REACHED,
        messages::download_limit_reached(seconds),
    )
    .with_param("wait_seconds", seconds.to_string())
}

fn temporarily_unavailable(seconds: u64) -> Failure {
    Failure::coded(
        FailureKind::Transient(Some(seconds)),
        messages::TEMPORARILY_UNAVAILABLE,
        messages::temporarily_unavailable(seconds),
    )
    .with_param("wait_seconds", seconds.to_string())
}
