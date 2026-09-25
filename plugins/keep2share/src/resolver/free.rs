//! The account-less free flow: an image captcha and a server-side countdown, both mediated by
//! the host.
//!
//! The first `/geturl` is deliberately a probe. Keep2Share answers it either with a captcha
//! demand — the one error this flow answers rather than reports — or with a limit, and finding
//! out which *before* a captcha has been paid for is the whole point of asking first.

use plugin_common::{CaptchaChallenge, Failure, HttpRequest, ImageChallenge, PluginHost, Resolved};
use url::Url;

use super::{CallError, api_call_raw, convert_failure};
use crate::api::{
    self,
    free::{FreeUrlResult, WaitStep},
};

/// Runs the account-less free flow and turns its result into a transfer.
pub(super) async fn resolve<H: PluginHost>(host: &H, file_id: &str) -> Result<Resolved, Failure> {
    let probe = free_geturl(host, api::free::geturl_probe_body(file_id)).await;
    let result = match probe {
        Ok(result) => result,
        // "You need send request for free download with captcha fields" — the only error the
        // flow answers rather than reports.
        Err(CallError::Api(failure)) if api::free::needs_captcha(&failure) => {
            solve_and_submit(host, file_id).await?
        }
        Err(error) => return Err(free_error(error)),
    };
    let url = wait_out_countdown(host, file_id, result).await?;
    Ok(Resolved {
        url: url.to_string(),
        // `/geturl` carries neither a filename nor a size on any path; the caller learns those
        // from a separate `check()` call, exactly as on the premium path.
        file_name: None,
        size: None,
        headers: Vec::new(),
        checksum: None,
    })
}

/// `POST /requestcaptcha` -> fetch the image -> hand it to the host's solver -> `POST /geturl`
/// with the challenge id and the typed answer.
async fn solve_and_submit<H: PluginHost>(
    host: &H,
    file_id: &str,
) -> Result<FreeUrlResult, Failure> {
    let challenge: api::free::RequestCaptchaResult = api_call_raw(
        host,
        api::free::REQUESTCAPTCHA_PATH,
        api::free::requestcaptcha_body(),
    )
    .await
    .map_err(free_error)?;
    let (challenge_id, image_url) =
        api::free::captcha_request(&challenge).map_err(convert_failure)?;
    let image = fetch_captcha_image(host, &image_url).await?;
    let solution = host.solve_captcha(image).await?;
    free_geturl(
        host,
        api::free::geturl_captcha_body(file_id, &challenge_id, &solution.token),
    )
    .await
    .map_err(free_error)
}

/// Fetches the captcha image and wraps its bytes in the challenge the host solves. The media
/// type comes from the response, or from the bytes themselves when the server does not say.
async fn fetch_captcha_image<H: PluginHost>(
    host: &H,
    image_url: &Url,
) -> Result<CaptchaChallenge, Failure> {
    let response = host.http(HttpRequest::get(image_url.to_string())).await?;
    api::ensure_http_status(response.status).map_err(convert_failure)?;
    let mime = api::free::image_mime(response.header("content-type"), &response.body)
        .map_err(convert_failure)?;
    Ok(CaptchaChallenge::Image(ImageChallenge {
        mime,
        data: response.body,
        // `/requestcaptcha` never sends prompt text.
        prompt: None,
    }))
}

/// Sits out the countdown the API asks for and posts `/geturl` again with the
/// `free_download_key`, until it answers with a URL. A countdown that is too long, or a sixth
/// round in a row, is an IP block rather than something to keep waiting on.
async fn wait_out_countdown<H: PluginHost>(
    host: &H,
    file_id: &str,
    first: FreeUrlResult,
) -> Result<Url, Failure> {
    let mut result = first;
    for round in 0..=api::free::MAX_WAIT_ROUNDS {
        let (Some(key), Some(seconds)) = (result.free_download_key.clone(), result.wait_seconds())
        else {
            break;
        };
        match api::free::wait_step(seconds, round) {
            WaitStep::Blocked(seconds) => {
                return Err(convert_failure(api::free::wait_limit_failure(seconds)));
            }
            WaitStep::Wait(seconds) => host.wait(seconds).await?,
        }
        result = free_geturl(host, api::free::geturl_free_key_body(file_id, &key))
            .await
            .map_err(free_error)?;
    }
    let raw_url = result
        .url
        .as_deref()
        .ok_or_else(|| convert_failure(api::free::no_free_link_failure(&result.diagnosis())))?;
    api::parse_download_url(raw_url).map_err(convert_failure)
}

async fn free_geturl<H: PluginHost>(host: &H, body: Vec<u8>) -> Result<FreeUrlResult, CallError> {
    api_call_raw(host, api::free::GETURL_PATH, body).await
}

/// Reports an API error the way the free path must: a stated download, traffic or wait limit
/// becomes an `IpBlocked`, everything else keeps its own classification.
fn free_error(error: CallError) -> Failure {
    match error {
        CallError::Host(failure) => failure,
        CallError::Api(failure) => convert_failure(api::free::free_failure(failure)),
    }
}
