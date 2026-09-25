//! The documented API, as this plugin calls it, and what its answers mean.
//!
//! `https://www.mediafire.com/api/1.5/<call>.php?response_format=json`, `GET`, no session
//! token: `file/get_info` and `folder/get_info` answer for public items without one (measured
//! 2026-09-21). The envelope is read before the status, because the API puts an error in a
//! `400` and a `404` alike and both carry the same document.

use mediafire_common::{
    address,
    api::{self, ApiError, Envelope, FileInfo},
};
use plugin_common::{Failure, FailureKind, HttpRequest, HttpResponse, PluginHost};
use serde_json::Value;

use crate::messages;

/// Most keys one `file/get_info` call carries. The documentation allows 500; JDownloader
/// sends 100, and a smaller batch keeps one bad answer from taking down a large check.
pub(crate) const CHECK_BATCH: usize = 100;

/// A failure from one of the `(code, text)` pairs in `messages`.
#[must_use]
pub(crate) fn coded(kind: FailureKind, (code, message): (&str, &str)) -> Failure {
    Failure::coded(kind, code, message)
}

/// One API call; the `response` object on success.
pub(crate) async fn call<H: PluginHost>(
    host: &H,
    name: &str,
    params: &[(&str, String)],
) -> Result<Value, Failure> {
    let mut request =
        HttpRequest::get(address::api_call(name)).with_query("response_format", "json");
    for (name, value) in params {
        request = request.with_query(name, value.clone());
    }
    let response = host.http(request).await?;
    match api::envelope(&response.body) {
        Some(Envelope::Success(value)) => Ok(value),
        Some(Envelope::Error(error)) => Err(api_failure(&error)),
        None => Err(status_failure(&response)),
    }
}

/// What the API knows about one file.
pub(crate) async fn file_info<H: PluginHost>(host: &H, key: &str) -> Result<FileInfo, Failure> {
    let response = call(host, "file/get_info", &[("quick_key", key.to_owned())]).await?;
    api::file_infos(&response)
        .into_iter()
        .find(|info| info.key == key)
        .ok_or_else(|| coded(FailureKind::Permanent, messages::INVALID_RESPONSE))
}

/// What the API knows about up to [`CHECK_BATCH`] files; keys it skipped are absent.
pub(crate) async fn file_infos<H: PluginHost>(
    host: &H,
    keys: &[&str],
) -> Result<Vec<FileInfo>, Failure> {
    let response = call(host, "file/get_info", &[("quick_key", keys.join(","))]).await?;
    Ok(api::file_infos(&response))
}

/// Whether `key` names a folder: the API reports an unknown *file* key as "missing", so a
/// bare key that is not a file is asked about as a folder before it is called invalid.
pub(crate) async fn is_folder<H: PluginHost>(host: &H, key: &str) -> bool {
    call(host, "folder/get_info", &[("folder_key", key.to_owned())])
        .await
        .is_ok_and(|response| response.get("folder_info").is_some())
}

/// An API error, in the scheduler's categories.
#[must_use]
pub(crate) fn api_failure(error: &ApiError) -> Failure {
    match error.code {
        api::ERROR_RATE_LIMIT => coded(FailureKind::RateLimited(None), messages::RATE_LIMITED),
        api::ERROR_QUICKKEY_UNKNOWN => coded(FailureKind::Offline, messages::FILE_UNAVAILABLE),
        api::ERROR_QUICKKEY_MISSING => coded(FailureKind::Permanent, messages::INVALID_LINK),
        api::ERROR_ACCESS_DENIED => coded(FailureKind::Permanent, messages::PRIVATE_FILE),
        _ => Failure::coded(
            FailureKind::Permanent,
            messages::API_ERROR,
            messages::api_error(&error.message),
        )
        .with_param("message", error.message.clone()),
    }
}

/// A status that is not an answer, or an answer that is not a document.
#[must_use]
pub(crate) fn status_failure(response: &HttpResponse) -> Failure {
    match response.status {
        200..=299 => coded(FailureKind::Permanent, messages::INVALID_RESPONSE),
        status => http_failure(status),
    }
}

/// An HTTP status the plugin cannot use, in the scheduler's categories.
#[must_use]
pub(crate) fn http_failure(status: u16) -> Failure {
    let kind = match status {
        429 => FailureKind::RateLimited(None),
        404 | 410 => FailureKind::Offline,
        500..=599 => FailureKind::Transient(None),
        _ => FailureKind::Permanent,
    };
    Failure::coded(kind, messages::HTTP_ERROR, messages::http_error(status))
        .with_param("status", status.to_string())
}

/// What an `error.php?errno=<n>` page means. The numbers are the ones JD's `MediafireCom`
/// has collected over the years (job file, section 4); none was provoked live except 320.
#[must_use]
pub(crate) fn errno_failure(errno: u32) -> Failure {
    let blocked = |reason: &str| {
        Failure::coded(
            FailureKind::Permanent,
            messages::FILE_BLOCKED,
            messages::file_blocked(reason),
        )
        .with_param("reason", reason)
    };
    match errno {
        320 => coded(FailureKind::Offline, messages::FILE_UNAVAILABLE),
        323 | 326 => blocked("dangerous_file"),
        378 | 386 => blocked("terms_violation"),
        380 | 388 => blocked("copyright_claim"),
        382 => blocked("account_suspended"),
        394 => Failure::coded(
            FailureKind::Permanent,
            messages::OWNER_LIMIT,
            messages::owner_limit("encrypted_archive_limit"),
        )
        .with_param("reason", "encrypted_archive_limit"),
        999 => coded(FailureKind::Permanent, messages::PRIVATE_FILE),
        other => Failure::coded(
            FailureKind::Permanent,
            messages::ERROR_PAGE,
            messages::error_page(other),
        )
        .with_param("errno", other.to_string()),
    }
}

/// A URL the plugin built or read that does not parse.
#[must_use]
pub(crate) fn invalid_url(error: &dyn std::fmt::Display) -> Failure {
    Failure::coded(
        FailureKind::Permanent,
        messages::INVALID_URL,
        messages::invalid_url(error),
    )
    .with_param("error", error.to_string())
}
