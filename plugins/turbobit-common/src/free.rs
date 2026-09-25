//! The account-less download, in the site's own order.
//!
//! `download/info` says what the file is and whether a guest may have it at all; `free/init`
//! opens the attempt and is where the guest window shows (`directHit`); `captcha` names the
//! Turnstile and `free/captcha` takes its answer and states the countdown; `free/prepare` and
//! `free/start` close the attempt with a `downloadUrl`. The order is the SPA's, step for step,
//! and nothing is skipped on the strength of the server not checking (see the crate doc).
//!
//! The direct link is one `GET` from this IP and then gone (`trycount`, `sid`, `till`), so it
//! is neither probed here nor stored anywhere: it goes straight into the `Resolved` and the
//! transfer that starts right after is the one request it is good for. A transfer that fails,
//! stops or restarts resolves again, and a second `resolve` of the same file yields a fresh
//! link, never the previous one.

use plugin_common::{
    CaptchaAnswer, CaptchaChallenge, Failure, FailureKind, Header, PluginHost, Resolved,
    WidgetChallenge,
};
use serde::Deserialize;
use serde_json::json;
use url::Url;

use crate::api::{self, Count};
use crate::brand::Brand;
use crate::reason::{coded, invalid_response};

/// How long a guest waits after `directHit: true` before the site lets this IP start another
/// free download. **Assumed**, not measured: the window's length was not observed, and the
/// historical figure is "one file per 60 minutes". It is the hold-off handed to the scheduler
/// as an IP block, never waited out in place, so a wrong guess costs a retry, not a budget.
pub const GUEST_WINDOW_SECONDS: u64 = 3600;

/// The `file` record most answers carry.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct FileMeta {
    pub id: Option<String>,
    pub name: Option<String>,
    pub size: Option<Count>,
}

/// `download/info`: the file, the guest's terms and — with a premium session — the mirrors.
#[derive(Debug, Default, Deserialize)]
pub struct DownloadInfo {
    pub file: Option<FileMeta>,
    pub premium: Option<bool>,
    #[serde(rename = "freeDownloadSize")]
    pub free_download_size: Option<Count>,
    #[serde(rename = "premiumOnlyDownload")]
    pub premium_only_download: Option<bool>,
    #[serde(rename = "downloadUrls")]
    pub download_urls: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct FreeInit {
    #[serde(rename = "directHit")]
    direct_hit: Option<bool>,
    #[serde(rename = "ipBan")]
    ip_ban: Option<IpBan>,
}

/// JDownloader reads `ipBan { delay, now }` off `free/init`; the measurement never saw one.
#[derive(Debug, Deserialize)]
struct IpBan {
    delay: Option<Count>,
}

/// `GET captcha`: which widget the site shows, and its site key.
#[derive(Debug, Deserialize)]
pub struct CaptchaSpec {
    pub driver: Option<String>,
    pub index: Option<u32>,
    #[serde(rename = "publicKey")]
    pub public_key: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CaptchaAccepted {
    delay: Option<Count>,
}

#[derive(Debug, Deserialize)]
struct Started {
    #[serde(rename = "downloadUrl")]
    download_url: Option<String>,
    file: Option<FileMeta>,
}

/// The whole free flow for one file id.
///
/// # Errors
///
/// `file_unavailable`, `premium_only`, `free_limit_reached`, `captcha_rejected`,
/// `no_direct_link`, the host's captcha and wait refusals, and `invalid_response` for an answer
/// that is not the JSON the step needs.
pub async fn resolve<H: PluginHost>(
    brand: &Brand,
    host: &H,
    id: &str,
) -> Result<Resolved, Failure> {
    let info = download_info(brand, host, id).await?;
    resolve_from_info(brand, host, id, &info).await
}

/// `POST download/info`, as the file page sends it.
///
/// # Errors
///
/// See [`api::call`].
pub async fn download_info<H: PluginHost>(
    brand: &Brand,
    host: &H,
    id: &str,
) -> Result<DownloadInfo, Failure> {
    let body = json!({"fileId": id, "referrer": null, "site": null, "shortDomain": ""});
    api::call(
        brand,
        host,
        api::post(brand, "download/info", &body, &brand.free_page(id)),
        "download/info",
    )
    .await
}

/// The free flow after `download/info` has been read, for a caller that already has it.
///
/// # Errors
///
/// See [`resolve`].
pub async fn resolve_from_info<H: PluginHost>(
    brand: &Brand,
    host: &H,
    id: &str,
    info: &DownloadInfo,
) -> Result<Resolved, Failure> {
    // Refused before any captcha is spent: a file that never starts without an account is not
    // worth a Turnstile round, and the person is told the actual reason.
    ensure_free_allowed(brand, info)?;
    let page = brand.free_page(id);
    let init: FreeInit = api::call(
        brand,
        host,
        api::post(brand, "download/free/init", &json!({"fileId": id}), &page),
        "free/init",
    )
    .await?;
    if init.direct_hit == Some(true) {
        return Err(free_limit_reached(brand, GUEST_WINDOW_SECONDS));
    }
    if let Some(delay) = init
        .ip_ban
        .and_then(|ban| ban.delay)
        .and_then(Count::into_u64)
    {
        return Err(free_limit_reached(brand, delay));
    }
    let delay = answer_captcha(brand, host, id).await?;
    if let Ok(seconds) = u32::try_from(delay)
        && seconds > 0
    {
        host.wait(seconds).await?;
    }
    let _prepared: serde_json::Value = api::call(
        brand,
        host,
        api::post(
            brand,
            "download/free/prepare",
            &json!({"fileId": id}),
            &page,
        ),
        "free/prepare",
    )
    .await?;
    let started: Started = api::call(
        brand,
        host,
        api::post(brand, "download/free/start", &json!({"fileId": id}), &page),
        "free/start",
    )
    .await?;
    let Some(raw) = started
        .download_url
        .filter(|value| !value.trim().is_empty())
    else {
        return Err(coded(
            brand,
            FailureKind::Permanent,
            brand.codes.no_direct_link,
            "free/start answered without a download link",
        ));
    };
    let name = started
        .file
        .as_ref()
        .and_then(|file| file.name.clone())
        .or_else(|| info.file.as_ref().and_then(|file| file.name.clone()));
    let size = started
        .file
        .and_then(|file| file.size)
        .or_else(|| info.file.as_ref().and_then(|file| file.size.clone()))
        .and_then(Count::into_u64);
    resolved(brand, id, &raw, name, size)
}

/// The `Resolved` for a direct link, whichever flow earned it.
///
/// # Errors
///
/// `invalid_url` for a link that does not parse, `no_direct_link` for one that points away
/// from the site.
pub fn resolved(
    brand: &Brand,
    id: &str,
    raw: &str,
    file_name: Option<String>,
    size: Option<u64>,
) -> Result<Resolved, Failure> {
    Ok(Resolved {
        url: direct_link(brand, raw)?,
        file_name,
        size,
        // The transfer looks like the browser that earned the link.
        headers: vec![Header::new("Referer", brand.started_page(id))],
        checksum: None,
    })
}

/// Whether a guest may have this file at all, from what `download/info` said.
///
/// # Errors
///
/// `premium_only` for `premiumOnlyDownload: true` or a size above `freeDownloadSize`.
pub fn ensure_free_allowed(brand: &Brand, info: &DownloadInfo) -> Result<(), Failure> {
    let size = info
        .file
        .as_ref()
        .and_then(|file| file.size.clone())
        .and_then(Count::into_u64);
    let limit = info.free_download_size.clone().and_then(Count::into_u64);
    let too_large = matches!((size, limit), (Some(size), Some(limit)) if size > limit);
    if info.premium_only_download == Some(true) || too_large {
        return Err(api::failure_for(brand, api::Refusal::PremiumOnly));
    }
    Ok(())
}

/// The direct link as the transfer may use it.
///
/// The answer is not URL-encoded — a file name with a space arrives with the space — and
/// JDownloader's `%` -> `%25` is what keeps a name containing a percent sign from being read as
/// an escape. The host must be the site or one of its delivery subdomains: a link elsewhere is
/// refused rather than followed.
///
/// # Errors
///
/// See [`resolved`].
pub fn direct_link(brand: &Brand, raw: &str) -> Result<String, Failure> {
    let escaped = raw.trim().replace('%', "%25");
    let url = Url::parse(&escaped).map_err(|error| {
        coded(
            brand,
            FailureKind::Permanent,
            brand.codes.invalid_url,
            &format!("the download link could not be parsed: {error}"),
        )
        .with_param("error", error.to_string())
    })?;
    let owned = url.scheme() == "https" && url.host_str().is_some_and(|host| brand.owns_host(host));
    if !owned {
        return Err(coded(
            brand,
            FailureKind::Permanent,
            brand.codes.no_direct_link,
            "the download link points outside the site",
        ));
    }
    Ok(url.to_string())
}

/// One or two Turnstile rounds, ending in the countdown the site states.
async fn answer_captcha<H: PluginHost>(brand: &Brand, host: &H, id: &str) -> Result<u64, Failure> {
    let page = brand.free_page(id);
    for attempt in 0..2 {
        let spec: CaptchaSpec =
            api::call(brand, host, api::get(brand, "captcha", &page), "captcha").await?;
        let (site_key, index) = turnstile(brand, &spec)?;
        let token = match host
            .solve_challenge(CaptchaChallenge::Turnstile(WidgetChallenge {
                site_key,
                page_url: page.clone(),
                invisible: false,
            }))
            .await?
        {
            CaptchaAnswer::Token(token) => token,
            CaptchaAnswer::Point(_) => return Err(invalid_response(brand, "captcha-answer")),
        };
        let body = json!({"fileId": id, "captchaResponse": token, "captchaIndex": index});
        let accepted: Result<CaptchaAccepted, api::Refusal> = api::exchange(
            brand,
            host,
            api::post(brand, "download/free/captcha", &body, &page),
            "free/captcha",
        )
        .await?;
        match accepted {
            Ok(accepted) => return Ok(accepted.delay.and_then(Count::into_u64).unwrap_or(0)),
            // Rejected once: a fresh challenge, the way the SPA re-renders the widget.
            Err(api::Refusal::CaptchaInvalid) if attempt == 0 => {}
            Err(refusal) => return Err(api::failure_for(brand, refusal)),
        }
    }
    Err(api::failure_for(brand, api::Refusal::CaptchaInvalid))
}

/// The site key of a Turnstile spec, refusing any other widget: the SDK can answer three
/// widget kinds, but only the one the site is known to show is claimed here.
///
/// # Errors
///
/// `invalid_response` naming `driver` or `publicKey`.
pub fn turnstile(brand: &Brand, spec: &CaptchaSpec) -> Result<(String, u32), Failure> {
    if spec.driver.as_deref() != Some("turnstile") {
        return Err(invalid_response(brand, "driver"));
    }
    let key = spec
        .public_key
        .as_deref()
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .ok_or_else(|| invalid_response(brand, "publicKey"))?;
    Ok((key.to_owned(), spec.index.unwrap_or(0)))
}

fn free_limit_reached(brand: &Brand, wait_seconds: u64) -> Failure {
    coded(
        brand,
        FailureKind::IpBlocked(Some(wait_seconds)),
        brand.codes.free_limit_reached,
        &format!("this IP address may start another free download in {wait_seconds}s"),
    )
    .with_param("wait_seconds", wait_seconds.to_string())
}
