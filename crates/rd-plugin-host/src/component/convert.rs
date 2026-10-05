//! Conversions between the WIT types and the domain types -- challenges, downloads,
//! identities, checksums and failures -- and the failures a component error becomes.
//!
//! Split out of `component.rs` (PLUG-21).

use std::str::FromStr;

use rd_core::{AccountId, ByteCount, ChecksumAlgorithm, Failure, FailureKind, ProxyProfileId};
use rd_plugin_api::{ClientIdentity, ResolvedChecksum, ResolvedDownload, ResolvedHeader};
use url::Url;

use super::{MAX_CAPTCHA_IMAGE_BYTES, MAX_SITE_KEY_BYTES, wit_captcha, wit_types};
use crate::domain_allowed;

pub(super) fn host_disconnected() -> Failure {
    Failure::coded(
        FailureKind::Permanent,
        "plugin.host_disconnected",
        "Plugin host is not connected",
    )
}

/// Refuses a `solve-captcha` call for a challenge whose answer is not a token.
pub(super) fn answer_shape_refused() -> Failure {
    Failure::coded(
        FailureKind::Permanent,
        "captcha.answer_shape",
        "A click-point captcha answers with a point: call solve-challenge, not solve-captcha",
    )
}

/// Converts a guest challenge, rejecting oversized image payloads and unusable site keys
/// before they reach a paid solver service.
pub(super) fn from_wit_challenge(
    value: wit_captcha::CaptchaChallenge,
) -> Result<rd_plugin_api::CaptchaChallenge, Failure> {
    use rd_plugin_api::{CaptchaChallenge, CutcaptchaChallenge, ImageChallenge, WidgetChallenge};

    fn site_key(value: &str) -> Result<(), Failure> {
        if value.is_empty() || value.len() > MAX_SITE_KEY_BYTES {
            return Err(Failure::coded(
                FailureKind::Permanent,
                "captcha.site_key_invalid",
                "Plugin reported an unusable captcha site key",
            ));
        }
        Ok(())
    }

    fn widget(value: wit_captcha::WidgetChallenge) -> Result<WidgetChallenge, Failure> {
        site_key(&value.site_key)?;
        let page_url = Url::parse(&value.page_url).map_err(permanent)?;
        Ok(WidgetChallenge {
            site_key: value.site_key,
            page_url: page_url.to_string(),
            invisible: value.invisible,
        })
    }

    fn picture(value: wit_captcha::ImageChallenge) -> Result<ImageChallenge, Failure> {
        if value.data.is_empty() || value.data.len() > MAX_CAPTCHA_IMAGE_BYTES {
            return Err(Failure::coded(
                FailureKind::Permanent,
                "captcha.image_invalid",
                "Plugin reported an unusable captcha image",
            ));
        }
        Ok(ImageChallenge {
            mime: value.mime,
            data: value.data,
            prompt: value.prompt,
        })
    }

    Ok(match value {
        wit_captcha::CaptchaChallenge::RecaptchaV2(inner) => {
            CaptchaChallenge::RecaptchaV2(widget(inner)?)
        }
        wit_captcha::CaptchaChallenge::Hcaptcha(inner) => {
            CaptchaChallenge::HCaptcha(widget(inner)?)
        }
        wit_captcha::CaptchaChallenge::Turnstile(inner) => {
            CaptchaChallenge::Turnstile(widget(inner)?)
        }
        wit_captcha::CaptchaChallenge::Image(inner) => CaptchaChallenge::Image(picture(inner)?),
        wit_captcha::CaptchaChallenge::ClickPoint(inner) => {
            CaptchaChallenge::ClickPoint(picture(inner)?)
        }
        // Both keys are held to the site-key rule: the solver task needs both, and an empty
        // one would be a paid request that cannot succeed.
        wit_captcha::CaptchaChallenge::Cutcaptcha(inner) => {
            site_key(&inner.site_key)?;
            site_key(&inner.misery_key)?;
            let page_url = Url::parse(&inner.page_url).map_err(permanent)?;
            CaptchaChallenge::Cutcaptcha(CutcaptchaChallenge {
                site_key: inner.site_key,
                misery_key: inner.misery_key,
                page_url: page_url.to_string(),
            })
        }
    })
}

/// Where a redirect response points, if it is one: the `Location` of a `3xx`, resolved
/// against the address that answered.
pub(super) fn unfollowed_redirect(response: &rd_plugin_api::HostHttpResponse) -> Option<Url> {
    if !(300..400).contains(&response.status) {
        return None;
    }
    let location = response
        .headers
        .iter()
        .find(|header| header.name.eq_ignore_ascii_case("location"))?;
    response.final_url.join(&location.value).ok()
}

pub(crate) fn from_wit_download(
    value: wit_types::ResolvedDownload,
    expected_identity: ClientIdentity,
    domains: &[String],
) -> Result<ResolvedDownload, Failure> {
    let url = Url::parse(&value.url).map_err(permanent)?;
    if !domain_allowed(&url, domains) {
        return Err(Failure::coded(
            FailureKind::Permanent,
            "plugin.resolved_url_not_allowed",
            "Resolved URL is not allowed by the plugin manifest",
        ));
    }
    let returned_identity = from_wit_identity(value.client)?;
    if returned_identity != expected_identity {
        return Err(Failure::coded(
            FailureKind::Permanent,
            "plugin.identity_changed",
            "Plugin changed the bound client identity",
        ));
    }
    let checksum = match (value.checksum_algorithm, value.checksum_value) {
        (None, None) => None,
        (Some(algorithm), Some(value)) => Some(ResolvedChecksum {
            algorithm: parse_checksum_algorithm(&algorithm)?,
            value,
        }),
        _ => {
            return Err(Failure::coded(
                FailureKind::Permanent,
                "plugin.checksum_incomplete",
                "Plugin checksum is incomplete",
            ));
        }
    };
    Ok(ResolvedDownload {
        url,
        file_name: value.file_name,
        size: value
            .size
            .map(ByteCount::new)
            .transpose()
            .map_err(permanent)?,
        headers: value
            .headers
            .into_iter()
            .map(|header| ResolvedHeader {
                name: header.name,
                value: header.value,
            })
            .collect(),
        checksum,
        client: expected_identity,
    })
}

pub(crate) fn to_wit_identity(value: &ClientIdentity) -> wit_types::ClientIdentity {
    wit_types::ClientIdentity {
        account_id: value.account_id.map(|id| id.to_string()),
        proxy_profile_id: value.proxy_profile_id.map(|id| id.to_string()),
        tls_revision: value.tls_revision,
    }
}

fn from_wit_identity(value: wit_types::ClientIdentity) -> Result<ClientIdentity, Failure> {
    Ok(ClientIdentity {
        account_id: value
            .account_id
            .map(|id| AccountId::from_str(&id))
            .transpose()
            .map_err(permanent)?,
        proxy_profile_id: value
            .proxy_profile_id
            .map(|id| ProxyProfileId::from_str(&id))
            .transpose()
            .map_err(permanent)?,
        tls_revision: value.tls_revision,
    })
}

fn parse_checksum_algorithm(value: &str) -> Result<ChecksumAlgorithm, Failure> {
    match value.to_ascii_lowercase().as_str() {
        "md5" => Ok(ChecksumAlgorithm::Md5),
        "sha1" | "sha-1" => Ok(ChecksumAlgorithm::Sha1),
        "sha256" | "sha-256" => Ok(ChecksumAlgorithm::Sha256),
        "crc32" | "crc-32" => Ok(ChecksumAlgorithm::Crc32),
        "dropbox_content_hash" => Ok(ChecksumAlgorithm::DropboxContentHash),
        _ => Err(Failure::coded(
            FailureKind::Permanent,
            "plugin.checksum_unknown",
            "Plugin returned an unknown checksum algorithm",
        )),
    }
}

pub(crate) fn component_failure(error: wasmtime::Error) -> Failure {
    let (code, reason) = describe_component_error(&error);
    // A timeout says how long this attempt took, not that the plugin is broken — the same call
    // routinely succeeds on a retry, which is why users worked around it by starting the
    // download again. Everything else here (panic, exhausted fuel, memory limit) is a genuine
    // defect and stays permanent.
    let kind = if code == "plugin.timeout" {
        FailureKind::Transient {
            retry_after_seconds: None,
        }
    } else {
        FailureKind::Permanent
    };
    Failure::coded(kind, code, format!("Plugin execution failed: {reason}"))
        .with_param("reason", reason)
}

/// Summarises a wasmtime error as a stable code plus a single line: the trap
/// reason (panic, exhausted fuel, memory limit, timeout) matters to operators,
/// the wasm backtrace does not.
fn describe_component_error(error: &wasmtime::Error) -> (&'static str, String) {
    let root = error.root_cause().to_string();
    if let Some(trap) = error.downcast_ref::<wasmtime::Trap>() {
        return match trap {
            wasmtime::Trap::OutOfFuel => (
                "plugin.fuel_exhausted",
                "plugin compute budget (fuel) exhausted".to_owned(),
            ),
            wasmtime::Trap::Interrupt => {
                ("plugin.timeout", "plugin time limit exceeded".to_owned())
            }
            wasmtime::Trap::UnreachableCodeReached => (
                "plugin.trapped",
                "plugin crashed (panic/unreachable)".to_owned(),
            ),
            other => ("plugin.trapped", format!("wasm trap: {other}")),
        };
    }
    let reason = if root.starts_with("error while executing") {
        error.to_string()
    } else {
        root
    };
    ("plugin.execution_failed", reason)
}

pub(super) fn permanent(error: impl std::fmt::Display) -> Failure {
    Failure::new(FailureKind::Permanent, error.to_string())
}

pub(crate) fn to_wit_failure(value: Failure) -> wit_types::Failure {
    wit_types::Failure {
        category: match value.category {
            FailureKind::Transient {
                retry_after_seconds,
            } => wit_types::FailureKind::Transient(retry_after_seconds),
            FailureKind::Permanent => wit_types::FailureKind::Permanent,
            FailureKind::Offline => wit_types::FailureKind::Offline,
            FailureKind::AuthRequired => wit_types::FailureKind::AuthRequired,
            FailureKind::AccountInvalid => wit_types::FailureKind::AccountInvalid,
            FailureKind::RateLimited {
                retry_after_seconds,
            } => wit_types::FailureKind::RateLimited(retry_after_seconds),
            FailureKind::NeedsCaptcha => wit_types::FailureKind::NeedsCaptcha,
            FailureKind::Unsupported => wit_types::FailureKind::Unsupported,
            FailureKind::IpBlocked {
                retry_after_seconds,
            } => wit_types::FailureKind::IpBlocked(retry_after_seconds),
            FailureKind::CaptchaFailed => wit_types::FailureKind::CaptchaFailed,
        },
        message: value.message,
        code: value.code,
        params: value.params.into_iter().collect(),
    }
}

pub(crate) fn from_wit_failure(value: wit_types::Failure) -> Failure {
    // A plugin's wait is held to the day every other `Retry-After` is held to (RD-191-06,
    // PLUG-04): a value past it parked a download for years or overflowed the due time.
    let delay = |seconds: Option<u64>| seconds.map(rd_core::clamp_retry_after);
    let mut failure = Failure::new(
        match value.category {
            wit_types::FailureKind::Transient(seconds) => FailureKind::Transient {
                retry_after_seconds: delay(seconds),
            },
            wit_types::FailureKind::Permanent => FailureKind::Permanent,
            wit_types::FailureKind::Offline => FailureKind::Offline,
            wit_types::FailureKind::AuthRequired => FailureKind::AuthRequired,
            wit_types::FailureKind::AccountInvalid => FailureKind::AccountInvalid,
            wit_types::FailureKind::RateLimited(seconds) => FailureKind::RateLimited {
                retry_after_seconds: delay(seconds),
            },
            wit_types::FailureKind::NeedsCaptcha => FailureKind::NeedsCaptcha,
            wit_types::FailureKind::Unsupported => FailureKind::Unsupported,
            wit_types::FailureKind::IpBlocked(seconds) => FailureKind::IpBlocked {
                retry_after_seconds: delay(seconds),
            },
            wit_types::FailureKind::CaptchaFailed => FailureKind::CaptchaFailed,
        },
        value.message,
    );
    failure.code = value.code.filter(|code| code.len() <= 128);
    failure.params = value
        .params
        .into_iter()
        .take(16)
        .filter(|(key, value)| key.len() <= 64 && value.len() <= 512)
        .collect();
    failure
}
