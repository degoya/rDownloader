//! The conversions from this crate's vocabulary into the scheduler's: what every plugin's
//! `native_resolver!` hands back, written once.

use rd_core::ByteCount;
use rd_plugin_api::ClientIdentity;

use crate::types::{Account, Failure, FailureKind, LinkCheck, LinkStatus, ResolveInput, Resolved};

/// This crate's failure, as the scheduler expects it.
#[must_use]
pub fn to_native_failure(failure: Failure) -> rd_core::Failure {
    let mut native = rd_core::Failure::coded(
        to_native_kind(failure.kind),
        failure.code.as_deref().unwrap_or("plugin.failed"),
        failure.message,
    );
    for (name, value) in failure.params {
        native = native.with_param(&name, value);
    }
    native
}

fn to_native_kind(kind: FailureKind) -> rd_core::FailureKind {
    match kind {
        FailureKind::Transient(retry_after_seconds) => rd_core::FailureKind::Transient {
            retry_after_seconds,
        },
        FailureKind::Permanent => rd_core::FailureKind::Permanent,
        FailureKind::Offline => rd_core::FailureKind::Offline,
        FailureKind::AuthRequired => rd_core::FailureKind::AuthRequired,
        FailureKind::AccountInvalid => rd_core::FailureKind::AccountInvalid,
        FailureKind::RateLimited(retry_after_seconds) => rd_core::FailureKind::RateLimited {
            retry_after_seconds,
        },
        FailureKind::NeedsCaptcha => rd_core::FailureKind::NeedsCaptcha,
        FailureKind::Unsupported => rd_core::FailureKind::Unsupported,
        FailureKind::IpBlocked(retry_after_seconds) => rd_core::FailureKind::IpBlocked {
            retry_after_seconds,
        },
        FailureKind::CaptchaFailed => rd_core::FailureKind::CaptchaFailed,
    }
}

/// One resolve request, as the shared logic takes it.
#[must_use]
pub fn to_resolve_input(request: &rd_plugin_api::ResolveRequest) -> ResolveInput {
    ResolveInput {
        url: request.url.to_string(),
        account_id: request.client.account_id.map(|id| id.to_string()),
    }
}

/// A resolved download, as the scheduler expects it.
///
/// # Errors
///
/// When the plugin produced a URL that does not parse; the host would refuse it anyway, and a
/// clear failure beats an unwrap in a plugin adapter.
pub fn to_native_resolved(
    resolved: Resolved,
    client: ClientIdentity,
) -> Result<rd_plugin_api::ResolvedDownload, rd_core::Failure> {
    let url = url::Url::parse(&resolved.url).map_err(|error| {
        rd_core::Failure::coded(
            rd_core::FailureKind::Permanent,
            "plugin.invalid_url",
            error.to_string(),
        )
    })?;
    Ok(rd_plugin_api::ResolvedDownload {
        url,
        file_name: resolved.file_name,
        size: resolved.size.and_then(|size| ByteCount::new(size).ok()),
        headers: resolved
            .headers
            .into_iter()
            .map(|header| rd_plugin_api::ResolvedHeader {
                name: header.name,
                value: header.value,
            })
            .collect(),
        checksum: resolved.checksum.and_then(|(algorithm, value)| {
            Some(rd_plugin_api::ResolvedChecksum {
                algorithm: checksum_algorithm(&algorithm)?,
                value,
            })
        }),
        client,
    })
}

/// The algorithms a plugin may name, spelled as the WIT contract spells them.
fn checksum_algorithm(value: &str) -> Option<rd_core::ChecksumAlgorithm> {
    match value.to_ascii_lowercase().as_str() {
        "md5" => Some(rd_core::ChecksumAlgorithm::Md5),
        "sha1" | "sha-1" => Some(rd_core::ChecksumAlgorithm::Sha1),
        "sha256" | "sha-256" => Some(rd_core::ChecksumAlgorithm::Sha256),
        "crc32" => Some(rd_core::ChecksumAlgorithm::Crc32),
        "dropbox_content_hash" => Some(rd_core::ChecksumAlgorithm::DropboxContentHash),
        _ => None,
    }
}

/// A link check batch, as the shared logic takes it.
#[must_use]
pub fn to_check_input(request: &rd_plugin_api::CheckRequest) -> crate::types::CheckInput {
    crate::types::CheckInput {
        urls: request.urls.iter().map(ToString::to_string).collect(),
        account_id: request.client.account_id.map(|id| id.to_string()),
    }
}

/// One link check result, as the scheduler expects it. An unparsable URL is dropped rather
/// than failing the whole batch: it was the plugin's answer about a link nobody can fetch.
#[must_use]
pub fn to_native_checks(results: Vec<LinkCheck>) -> Vec<rd_core::LinkCheckResult> {
    results
        .into_iter()
        .filter_map(|result| {
            Some(rd_core::LinkCheckResult {
                url: result.url.parse().ok()?,
                status: match result.status {
                    LinkStatus::Online => rd_core::LinkStatus::Online,
                    LinkStatus::Offline => rd_core::LinkStatus::Offline,
                    LinkStatus::Unknown => rd_core::LinkStatus::Unknown,
                    LinkStatus::Cached => rd_core::LinkStatus::Cached,
                },
                file_name: result.file_name,
                size: result.size.and_then(|size| ByteCount::new(size).ok()),
                media: None,
            })
        })
        .collect()
}

/// An account status, as the scheduler expects it.
#[must_use]
pub fn to_native_account(account: Account) -> rd_plugin_api::AccountStatus {
    rd_plugin_api::AccountStatus {
        valid: account.valid,
        premium: account.premium,
        label: account
            .label
            .into_iter()
            .map(|part| rd_plugin_api::LabelPart {
                code: part.code,
                params: part.params.into_iter().collect(),
                message: part.message,
            })
            .collect(),
        traffic_left: account
            .traffic_left
            .and_then(|traffic| ByteCount::new(traffic).ok()),
    }
}
