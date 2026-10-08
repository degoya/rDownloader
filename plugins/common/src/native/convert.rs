//! The conversions from this crate's vocabulary into the scheduler's: what every plugin's
//! `native_resolver!` hands back, written once.

use rd_plugin_api::ClientIdentity;
use rd_plugin_types::ByteCount;

use crate::types::{Account, Failure, FailureKind, LinkCheck, LinkStatus, ResolveInput, Resolved};

/// This crate's failure, as the scheduler expects it.
#[must_use]
pub fn to_native_failure(failure: Failure) -> rd_plugin_types::Failure {
    let mut native = rd_plugin_types::Failure::coded(
        to_native_kind(failure.kind),
        failure.code.as_deref().unwrap_or("plugin.failed"),
        failure.message,
    );
    for (name, value) in failure.params {
        native = native.with_param(&name, value);
    }
    native
}

fn to_native_kind(kind: FailureKind) -> rd_plugin_types::FailureKind {
    match kind {
        FailureKind::Transient(retry_after_seconds) => rd_plugin_types::FailureKind::Transient {
            retry_after_seconds,
        },
        FailureKind::Permanent => rd_plugin_types::FailureKind::Permanent,
        FailureKind::Offline => rd_plugin_types::FailureKind::Offline,
        FailureKind::AuthRequired => rd_plugin_types::FailureKind::AuthRequired,
        FailureKind::AccountInvalid => rd_plugin_types::FailureKind::AccountInvalid,
        FailureKind::RateLimited(retry_after_seconds) => {
            rd_plugin_types::FailureKind::RateLimited {
                retry_after_seconds,
            }
        }
        FailureKind::NeedsCaptcha => rd_plugin_types::FailureKind::NeedsCaptcha,
        FailureKind::Unsupported => rd_plugin_types::FailureKind::Unsupported,
        FailureKind::IpBlocked(retry_after_seconds) => rd_plugin_types::FailureKind::IpBlocked {
            retry_after_seconds,
        },
        FailureKind::CaptchaFailed => rd_plugin_types::FailureKind::CaptchaFailed,
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
) -> Result<rd_plugin_api::ResolvedDownload, rd_plugin_types::Failure> {
    let url = url::Url::parse(&resolved.url).map_err(|error| {
        rd_plugin_types::Failure::coded(
            rd_plugin_types::FailureKind::Permanent,
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
fn checksum_algorithm(value: &str) -> Option<rd_plugin_types::ChecksumAlgorithm> {
    match value.to_ascii_lowercase().as_str() {
        "md5" => Some(rd_plugin_types::ChecksumAlgorithm::Md5),
        "sha1" | "sha-1" => Some(rd_plugin_types::ChecksumAlgorithm::Sha1),
        "sha256" | "sha-256" => Some(rd_plugin_types::ChecksumAlgorithm::Sha256),
        "crc32" => Some(rd_plugin_types::ChecksumAlgorithm::Crc32),
        "dropbox_content_hash" => Some(rd_plugin_types::ChecksumAlgorithm::DropboxContentHash),
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
pub fn to_native_checks(results: Vec<LinkCheck>) -> Vec<rd_plugin_types::PluginLinkCheck> {
    results
        .into_iter()
        .filter_map(|result| {
            Some(rd_plugin_types::PluginLinkCheck {
                url: result.url.parse().ok()?,
                status: match result.status {
                    LinkStatus::Online => rd_plugin_types::LinkStatus::Online,
                    LinkStatus::Offline => rd_plugin_types::LinkStatus::Offline,
                    LinkStatus::Unknown => rd_plugin_types::LinkStatus::Unknown,
                    LinkStatus::Cached => rd_plugin_types::LinkStatus::Cached,
                },
                file_name: result.file_name,
                size: result.size.and_then(|size| ByteCount::new(size).ok()),
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
