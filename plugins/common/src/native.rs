//! The native adapter, written once for every plugin.
//!
//! Converting between `rd_plugin_api`'s vocabulary and this crate's is the same work for every
//! hoster, so it happens here rather than eleven times. A plugin's `native.rs` is left with its
//! metadata and the four delegating methods.

use std::sync::Arc;

use rand::RngCore;
use rd_core::{AccountId, ByteCount};
use rd_plugin_api::{ClientIdentity, ResolverHost};

use crate::PluginHost;
use crate::types::{
    Account, CaptchaAnswer, CaptchaChallenge, CaptchaSolution, ClickPoint, Failure, FailureKind,
    HttpRequest, HttpResponse, LinkCheck, LinkStatus, ResolveInput, Resolved,
};

/// Most random bytes one `random_bytes` call may ask for; the WebAssembly host caps it at
/// the same number.
const MAX_RANDOM_BYTES: u32 = 1024;

/// One invocation's view of the native host, carrying the identity that call belongs to.
pub struct NativeHost {
    host: Arc<dyn ResolverHost>,
    client: ClientIdentity,
}

impl NativeHost {
    /// Binds the shared host to the client identity of one request.
    #[must_use]
    pub fn new(host: Arc<dyn ResolverHost>, client: ClientIdentity) -> Self {
        Self { host, client }
    }

    /// The identity this adapter speaks for, which the caller has to put back on the result.
    #[must_use]
    pub fn client(&self) -> &ClientIdentity {
        &self.client
    }

    /// Binds to an account with no proxy or TLS pinning, for the account-level calls.
    #[must_use]
    pub fn for_account(host: Arc<dyn ResolverHost>, account_id: AccountId) -> Self {
        Self::new(
            host,
            ClientIdentity {
                account_id: Some(account_id),
                proxy_profile_id: None,
                tls_revision: 0,
            },
        )
    }
}

impl PluginHost for NativeHost {
    async fn http(&self, request: HttpRequest) -> Result<HttpResponse, Failure> {
        let url = url::Url::parse(&request.url).map_err(|error| {
            Failure::coded(
                FailureKind::Permanent,
                "plugin.invalid_url",
                error.to_string(),
            )
        })?;
        let native = rd_plugin_api::HostHttpRequest {
            method: request.method,
            url,
            query: request.query.into_iter().map(to_native_value).collect(),
            headers: request.headers.into_iter().map(to_native_value).collect(),
            body: request.body,
            granted_secret: None,
            authority: rd_plugin_api::RequestAuthority::Provider,
            write_methods: false,
        };
        let response = self
            .host
            .http_request(&self.client, native)
            .await
            .map_err(from_native_failure)?;
        Ok(HttpResponse {
            status: response.status,
            final_url: response.final_url.to_string(),
            headers: response
                .headers
                .into_iter()
                .map(|header| (header.name, header.value))
                .collect(),
            body: response.body,
        })
    }

    async fn cookies(&self, account_id: &str, url: &str) -> Vec<(String, String)> {
        let (Ok(account_id), Ok(url)) = (account_id.parse::<AccountId>(), url.parse::<url::Url>())
        else {
            return Vec::new();
        };
        self.host.cookies_get(account_id, &url).await
    }

    async fn secret_available(&self, account_id: &str, reference: &str) -> bool {
        let Ok(account_id) = account_id.parse::<AccountId>() else {
            return false;
        };
        self.host.secret_available(account_id, reference).await
    }

    async fn wait(&self, seconds: u32) -> Result<(), Failure> {
        self.host
            .wait(&self.client, seconds)
            .await
            .map_err(from_native_failure)
    }

    async fn solve_captcha(&self, challenge: CaptchaChallenge) -> Result<CaptchaSolution, Failure> {
        // The same refusal the WebAssembly host makes, before anyone is asked.
        if matches!(challenge, CaptchaChallenge::ClickPoint(_)) {
            return Err(answer_shape_refused());
        }
        match self.solve_challenge(challenge).await? {
            CaptchaAnswer::Token(token) => Ok(CaptchaSolution { token }),
            CaptchaAnswer::Point(_) => Err(answer_shape_refused()),
        }
    }

    async fn solve_challenge(&self, challenge: CaptchaChallenge) -> Result<CaptchaAnswer, Failure> {
        // The budget is the host's business on both paths: the guest never sees one either.
        let limit = self.host.captcha_allowance().await;
        let answer = self
            .host
            .solve_captcha(&self.client, to_native_challenge(challenge), limit)
            .await
            .map_err(from_native_failure)?;
        Ok(match answer {
            rd_plugin_api::CaptchaAnswer::Token(token) => CaptchaAnswer::Token(token),
            rd_plugin_api::CaptchaAnswer::Point(point) => CaptchaAnswer::Point(ClickPoint {
                x: point.x,
                y: point.y,
            }),
        })
    }

    async fn now_unix_seconds(&self) -> u64 {
        u64::try_from(chrono::Utc::now().timestamp()).unwrap_or_default()
    }

    async fn random_bytes(&self, count: u32) -> Vec<u8> {
        // The same rule the WebAssembly host applies, so logic written against this trait sees
        // one behaviour on both targets: nothing at all above the cap, rather than a short
        // answer a caller might not notice.
        if count == 0 || count > MAX_RANDOM_BYTES {
            return Vec::new();
        }
        let mut bytes = vec![0u8; count as usize];
        rand::rng().fill_bytes(&mut bytes);
        bytes
    }

    fn log(&self, level: &str, message: &str) {
        match level {
            "error" => tracing::error!(plugin = true, "{message}"),
            "warn" => tracing::warn!(plugin = true, "{message}"),
            "debug" | "trace" => tracing::debug!(plugin = true, "{message}"),
            _ => tracing::info!(plugin = true, "{message}"),
        }
    }
}

fn to_native_value(header: crate::types::Header) -> rd_plugin_api::HostRequestValue {
    rd_plugin_api::HostRequestValue {
        name: header.name,
        value_template: header.value,
    }
}

fn answer_shape_refused() -> Failure {
    Failure::coded(
        FailureKind::Permanent,
        "captcha.answer_shape",
        "A click-point captcha answers with a point: call solve_challenge, not solve_captcha",
    )
}

fn to_native_challenge(challenge: CaptchaChallenge) -> rd_plugin_api::CaptchaChallenge {
    fn widget(value: crate::types::WidgetChallenge) -> rd_plugin_api::WidgetChallenge {
        rd_plugin_api::WidgetChallenge {
            site_key: value.site_key,
            page_url: value.page_url,
            invisible: value.invisible,
        }
    }
    fn picture(value: crate::types::ImageChallenge) -> rd_plugin_api::ImageChallenge {
        rd_plugin_api::ImageChallenge {
            mime: value.mime,
            data: value.data,
            prompt: value.prompt,
        }
    }
    match challenge {
        CaptchaChallenge::RecaptchaV2(value) => {
            rd_plugin_api::CaptchaChallenge::RecaptchaV2(widget(value))
        }
        CaptchaChallenge::HCaptcha(value) => {
            rd_plugin_api::CaptchaChallenge::HCaptcha(widget(value))
        }
        CaptchaChallenge::Turnstile(value) => {
            rd_plugin_api::CaptchaChallenge::Turnstile(widget(value))
        }
        CaptchaChallenge::Image(value) => rd_plugin_api::CaptchaChallenge::Image(picture(value)),
        CaptchaChallenge::ClickPoint(value) => {
            rd_plugin_api::CaptchaChallenge::ClickPoint(picture(value))
        }
        CaptchaChallenge::Cutcaptcha(value) => {
            rd_plugin_api::CaptchaChallenge::Cutcaptcha(rd_plugin_api::CutcaptchaChallenge {
                site_key: value.site_key,
                misery_key: value.misery_key,
                page_url: value.page_url,
            })
        }
    }
}

/// A failure the host produced, in this crate's vocabulary.
fn from_native_failure(failure: rd_core::Failure) -> Failure {
    Failure {
        kind: from_native_kind(failure.category),
        message: failure.message,
        code: failure.code,
        params: failure.params.into_iter().collect(),
    }
}

fn from_native_kind(kind: rd_core::FailureKind) -> FailureKind {
    match kind {
        rd_core::FailureKind::Transient {
            retry_after_seconds,
        } => FailureKind::Transient(retry_after_seconds),
        rd_core::FailureKind::Permanent => FailureKind::Permanent,
        rd_core::FailureKind::Offline => FailureKind::Offline,
        rd_core::FailureKind::AuthRequired => FailureKind::AuthRequired,
        rd_core::FailureKind::AccountInvalid => FailureKind::AccountInvalid,
        rd_core::FailureKind::RateLimited {
            retry_after_seconds,
        } => FailureKind::RateLimited(retry_after_seconds),
        rd_core::FailureKind::NeedsCaptcha => FailureKind::NeedsCaptcha,
        rd_core::FailureKind::Unsupported => FailureKind::Unsupported,
        rd_core::FailureKind::IpBlocked {
            retry_after_seconds,
        } => FailureKind::IpBlocked(retry_after_seconds),
        rd_core::FailureKind::CaptchaFailed => FailureKind::CaptchaFailed,
    }
}

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

/// An account label as `code(name=value)` parts separated by spaces: what the interface
/// translates, in a form a test can assert on and a log can show.
#[must_use]
pub fn label_summary(parts: &[rd_plugin_api::LabelPart]) -> String {
    parts
        .iter()
        .map(|part| {
            let params: Vec<String> = part
                .params
                .iter()
                .map(|(name, value)| format!("{name}={value}"))
                .collect();
            format!("{}({})", part.code, params.join(","))
        })
        .collect::<Vec<_>>()
        .join(" ")
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
