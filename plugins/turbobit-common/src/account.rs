//! Everything an account does: signing in, the premium download, the account check.
//!
//! All of it is written from JDownloader's `TurbobitCore` (r52763) rather than from a
//! measurement — nothing behind the login was reachable without an account — so every field is
//! read as optional and a shape that does not fit costs a figure, not the account. The session
//! that a sign-in produces lives in the host's per-account cookie jar, which this code can
//! neither read nor write; it only has to notice a `401` and sign in once more.
//!
//! The credentials never enter here either: the login body carries the host's `{{username}}`
//! and `{{secret:…}}` markers, and the host substitutes them, JSON-escaped, on the way out —
//! and only towards the one host the manifest pins the password to.

use plugin_common::{
    Account, CaptchaAnswer, CaptchaChallenge, Failure, FailureKind, Label, PluginHost, Resolved,
    WidgetChallenge,
};
use serde::Deserialize;
use serde_json::json;

use crate::api::{self, Count, Refusal};
use crate::brand::Brand;
use crate::free;
use crate::reason::{coded, invalid_response};

/// How long a premium account waits after the site handed out no mirror: JDownloader's
/// "limit of premium downloads" pause. Second-hand, like the rest of this module.
const PREMIUM_LIMIT_PAUSE_SECONDS: u64 = 30 * 60;

/// `GET user/info`, as far as it is read.
#[derive(Debug, Default, Deserialize)]
pub struct UserInfo {
    pub email: Option<String>,
    pub user: Option<UserRecord>,
    pub premium: Option<PremiumRecord>,
}

#[derive(Debug, Default, Deserialize)]
pub struct UserRecord {
    pub email: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct PremiumRecord {
    /// `active`, `banned`, or something else meaning "not premium".
    pub status: Option<String>,
    /// `yyyy-MM-dd HH:mm:ss`.
    #[serde(rename = "expiredAt")]
    pub expired_at: Option<String>,
}

/// `GET premium/info`: the traffic figures, in gigabytes, comma allowed as decimal point.
#[derive(Debug, Default, Deserialize)]
pub struct PremiumInfo {
    #[serde(rename = "dayTrafficLeft")]
    pub day_traffic_left: Option<Count>,
}

/// The download with an account: the session's own mirror when the account is premium, the
/// free flow on the same session when it is not.
///
/// # Errors
///
/// `account_missing` without a stored password, the sign-in's failures, `premium_limit_reached`
/// for a premium session that got no mirror, and everything the free flow can report.
pub async fn resolve<H: PluginHost>(
    brand: &Brand,
    host: &H,
    account_id: &str,
    id: &str,
) -> Result<Resolved, Failure> {
    ensure_password(brand, host, account_id).await?;
    // `download/info` answers a guest and a lapsed session alike, with `premium: false` and no
    // mirrors, so the session is confirmed first — the way JD checks the login state before
    // every download — rather than discovered from a download that quietly went the free way.
    ensure_session(brand, host).await?;
    let info = free::download_info(brand, host, id).await?;
    if let Some(raw) = info
        .download_urls
        .as_ref()
        .and_then(|urls| urls.iter().find(|url| !url.trim().is_empty()))
    {
        let name = info.file.as_ref().and_then(|file| file.name.clone());
        let size = info
            .file
            .as_ref()
            .and_then(|file| file.size.clone())
            .and_then(Count::into_u64);
        return free::resolved(brand, id, raw, name, size);
    }
    if info.premium == Some(true) {
        return Err(coded(
            brand,
            FailureKind::Transient(Some(PREMIUM_LIMIT_PAUSE_SECONDS)),
            brand.codes.premium_limit_reached,
            "the premium session received no download link; the daily limit may be reached",
        ));
    }
    free::resolve_from_info(brand, host, id, &info).await
}

/// What the account is worth.
///
/// `premium` is `true` only where `user/info` states `status: active` with an `expiredAt`
/// still ahead of the host's clock; a proven sign-in with nothing read about the subscription
/// says so in the label instead (RD-109-34).
///
/// # Errors
///
/// `account_missing`, the sign-in's failures, `account_banned`.
pub async fn check_account<H: PluginHost>(
    brand: &Brand,
    host: &H,
    account_id: &str,
) -> Result<Account, Failure> {
    ensure_password(brand, host, account_id).await?;
    let (user, signed_in_now) = user_info_signed_in(brand, host).await?;
    let email = user
        .email
        .clone()
        .or_else(|| user.user.as_ref().and_then(|record| record.email.clone()));
    let status = user
        .premium
        .as_ref()
        .and_then(|premium| premium.status.as_deref())
        .map(|status| status.trim().to_ascii_lowercase());
    let expired_at = user
        .premium
        .as_ref()
        .and_then(|premium| premium.expired_at.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if status.as_deref() == Some("banned") {
        let mut failure = coded(
            brand,
            FailureKind::AccountInvalid,
            brand.codes.account_banned,
            "the site reports this account as banned",
        );
        if let Some(until) = expired_at {
            failure = failure.with_param("until", until);
        }
        return Err(failure);
    }
    let now = host.now_unix_seconds().await;
    let expiry = expired_at.and_then(parse_expiry_unix);
    let premium = status.as_deref() == Some("active") && expiry.is_some_and(|expiry| expiry > now);
    let mut label = Label::new().user(email.as_deref());
    label = if signed_in_now {
        label.signed_in()
    } else {
        label.session_active()
    };
    label = match (premium, expiry) {
        (true, _) => label.premium_until(expired_at),
        (false, Some(_)) => label.premium_expired(),
        (false, None) => label.premium_unchecked(),
    };
    let traffic_left = if premium {
        let info: PremiumInfo = api::call(
            brand,
            host,
            api::get(brand, "premium/info", &brand.site_url()),
            "premium/info",
        )
        .await?;
        info.day_traffic_left.and_then(gigabytes_to_bytes)
    } else {
        None
    };
    Ok(Account {
        valid: true,
        premium,
        label: label.into(),
        traffic_left,
    })
}

/// Refuses before any request when the account holds no password.
async fn ensure_password<H: PluginHost>(
    brand: &Brand,
    host: &H,
    account_id: &str,
) -> Result<(), Failure> {
    if host
        .secret_available(account_id, brand.password_reference)
        .await
    {
        return Ok(());
    }
    Err(coded(
        brand,
        FailureKind::AuthRequired,
        brand.codes.account_missing,
        "the account holds no password",
    ))
}

/// A session that answers `user/info`, signing in when there is none.
async fn ensure_session<H: PluginHost>(brand: &Brand, host: &H) -> Result<(), Failure> {
    user_info_signed_in(brand, host).await.map(|_| ())
}

/// `user/info`, and whether this call had to sign in to get it.
async fn user_info_signed_in<H: PluginHost>(
    brand: &Brand,
    host: &H,
) -> Result<(UserInfo, bool), Failure> {
    let request = || api::get(brand, "user/info", &brand.site_url());
    match api::exchange::<UserInfo, H>(brand, host, request(), "user/info").await? {
        Ok(user) => Ok((user, false)),
        Err(Refusal::Unauthenticated) => {
            sign_in(brand, host).await?;
            let user = api::call(brand, host, request(), "user/info").await?;
            Ok((user, true))
        }
        Err(refusal) => Err(api::failure_for(brand, refusal)),
    }
}

/// `POST auth/login`, with a Turnstile round when the site asks for one.
///
/// The first attempt goes without a captcha; `needCaptcha: true` fetches the widget the login
/// page shows and answers it through the host, once. `password_incorrect` is the account's
/// fault and reported as such; a captcha still refused after being answered is not, and is
/// reported apart from it — sending someone to check a password that was never read is the
/// worst thing this path can do.
///
/// # Errors
///
/// `login_failed`, `login_captcha`, the host's captcha refusal, and the API's other refusals.
pub async fn sign_in<H: PluginHost>(brand: &Brand, host: &H) -> Result<(), Failure> {
    match login_call(brand, host, None).await? {
        Ok(()) => return Ok(()),
        Err(Refusal::NeedCaptcha) => {}
        Err(refusal) => return Err(api::failure_for(brand, refusal)),
    }
    let page = brand.login_page();
    let spec: free::CaptchaSpec =
        api::call(brand, host, api::get(brand, "captcha", &page), "captcha").await?;
    let (site_key, index) = free::turnstile(brand, &spec)?;
    let token = match host
        .solve_challenge(CaptchaChallenge::Turnstile(WidgetChallenge {
            site_key,
            page_url: page,
            invisible: false,
        }))
        .await?
    {
        CaptchaAnswer::Token(token) => token,
        CaptchaAnswer::Point(_) => return Err(invalid_response(brand, "captcha-answer")),
    };
    match login_call(brand, host, Some((&token, index))).await? {
        Ok(()) => Ok(()),
        Err(Refusal::CaptchaInvalid | Refusal::NeedCaptcha) => {
            Err(api::failure_for(brand, Refusal::NeedCaptcha))
        }
        Err(refusal) => Err(api::failure_for(brand, refusal)),
    }
}

async fn login_call<H: PluginHost>(
    brand: &Brand,
    host: &H,
    captcha: Option<(&str, u32)>,
) -> Result<Result<(), Refusal>, Failure> {
    let secret = format!("{{{{secret:{}}}}}", brand.password_reference);
    let body = match captcha {
        Some((token, index)) => json!({
            "email": "{{username}}",
            "password": secret,
            "captcha": true,
            "captchaResponse": token,
            "captchaIndex": index,
        }),
        None => json!({"email": "{{username}}", "password": secret, "captcha": false}),
    };
    let answer: Result<serde_json::Value, Refusal> = api::exchange(
        brand,
        host,
        api::post(brand, "auth/login", &body, &brand.login_page()),
        "auth/login",
    )
    .await?;
    Ok(answer.map(|_| ()))
}

/// Gigabytes as the site counts them, in bytes.
fn gigabytes_to_bytes(count: Count) -> Option<u64> {
    let gigabytes = count.into_f64()?;
    if !(0.0..=1.0e9).contains(&gigabytes) {
        return None;
    }
    Some((gigabytes * 1024.0 * 1024.0 * 1024.0) as u64)
}

/// `yyyy-MM-dd HH:mm:ss` as seconds since the Unix epoch, read as UTC; `None` for anything
/// else. The site's zone is not stated, and an hour either way does not change whether a
/// subscription that runs for weeks is active.
#[must_use]
pub fn parse_expiry_unix(value: &str) -> Option<u64> {
    let (date, time) = value.trim().split_once([' ', 'T'])?;
    let mut date = date.split('-').map(str::parse::<i64>);
    let (year, month, day) = (date.next()?.ok()?, date.next()?.ok()?, date.next()?.ok()?);
    let mut time = time.split(':').map(str::parse::<i64>);
    let (hour, minute, second) = (time.next()?.ok()?, time.next()?.ok()?, time.next()?.ok()?);
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || !(0..24).contains(&hour)
        || !(0..60).contains(&minute)
        || !(0..60).contains(&second)
    {
        return None;
    }
    let days = days_from_civil(year, month, day);
    u64::try_from(days * 86_400 + hour * 3_600 + minute * 60 + second).ok()
}

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's algorithm).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month_index = (month + 9) % 12;
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}
