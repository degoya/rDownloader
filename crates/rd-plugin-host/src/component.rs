//! Wasmtime Component Model adapter for the public resolver WIT world.

use std::{str::FromStr, sync::Arc, time::Instant};

use async_trait::async_trait;
use rand::RngCore;
use rd_core::{
    AccountId, ByteCount, ChecksumAlgorithm, Failure, FailureKind, LinkCheckResult, LinkStatus,
    ProxyProfileId,
};
use rd_plugin_api::{
    AccountStatus, CaptchaAnswer, CheckRequest, ClientIdentity, HostHttpRequest, HostRequestValue,
    ResolvedChecksum, ResolvedDownload, ResolvedHeader, Resolver, ResolverHost, ResolverMetadata,
};
use url::Url;
use wasmtime::component::{HasSelf, Linker};

use crate::{
    PluginManifest, SandboxEngine, captcha_target_refused, domain_allowed,
    runtime::PluginStoreState,
};

wasmtime::component::bindgen!({
    path: "../rd-plugin-api/wit",
    world: "resolver-plugin",
    imports: { default: async },
    exports: { default: async },
});

use rdownloader::plugin::{
    captcha as wit_captcha, cookies as wit_cookies, host as wit_host, http as wit_http,
    types as wit_types,
};

/// Smallest reservation worth making for a captcha; below this no service answers and
/// nobody types in time, so the plugin is told its budget is gone instead.
const MIN_CAPTCHA_ALLOWANCE: std::time::Duration = std::time::Duration::from_secs(30);
/// Longest accepted captcha site key; real ones are far shorter.
const MAX_SITE_KEY_BYTES: usize = 256;
/// Longest accepted captcha image, well above any real one.
const MAX_CAPTCHA_IMAGE_BYTES: usize = 512 * 1024;
/// Most random bytes one `host.random-bytes` call may ask for.
///
/// A PKCE verifier needs 32 and the largest thing a plugin plausibly seeds is a key; a request
/// beyond this is a mistake or an attempt to make the host allocate, and either is answered
/// with nothing rather than served.
const MAX_RANDOM_BYTES: u32 = 1024;

/// A compiled, version-specific Component implementing the native resolver abstraction.
pub struct ComponentResolver {
    metadata: ResolverMetadata,
    host_domains: Vec<String>,
    download_domains: Vec<String>,
    sandbox: SandboxEngine,
    pre: ResolverPluginPre<PluginStoreState>,
    host: Arc<dyn ResolverHost>,
    log: Arc<crate::ExecutionLog>,
}

impl ComponentResolver {
    /// Compiles a verified package and pins its manifest version for this resolver instance.
    pub fn new(
        manifest: PluginManifest,
        component_bytes: &[u8],
        host: Arc<dyn ResolverHost>,
    ) -> anyhow::Result<Self> {
        let sandbox = SandboxEngine::new(manifest.limits)?;
        let component = sandbox.compile_component(component_bytes, &manifest)?;
        let mut linker = Linker::new(sandbox.engine());
        // One grant, one interface. Only what the manifest asks for is linked, so a
        // component that imports an interface it was not granted fails to instantiate
        // instead of being turned away later, at a call the user is already waiting on.
        rdownloader::plugin::host::add_to_linker::<_, HasSelf<_>>(&mut linker, |state| state)?;
        if manifest.capabilities.net_http.is_some() {
            rdownloader::plugin::http::add_to_linker::<_, HasSelf<_>>(&mut linker, |state| state)?;
        }
        if manifest.capabilities.cookies {
            rdownloader::plugin::cookies::add_to_linker::<_, HasSelf<_>>(&mut linker, |state| {
                state
            })?;
        }
        if manifest.capabilities.captcha {
            rdownloader::plugin::captcha::add_to_linker::<_, HasSelf<_>>(&mut linker, |state| {
                state
            })?;
        }
        let pre = ResolverPluginPre::new(linker.instantiate_pre(&component)?)?;
        let host_domains = manifest.domains().to_vec();
        let provider_slug = manifest.message_slug().to_owned();
        let match_domains = if manifest.match_domains.is_empty() {
            host_domains.clone()
        } else {
            manifest.match_domains
        };
        let download_domains = if manifest.download_domains.is_empty() {
            host_domains.clone()
        } else {
            manifest.download_domains
        };
        let metadata = ResolverMetadata {
            plugin_id: manifest.id,
            name: manifest.name,
            version: manifest.version,
            provider_slug,
            domains: match_domains,
            max_concurrent_downloads: manifest.max_concurrent_downloads,
            requires_account: manifest.requires_account,
        };
        Ok(Self {
            metadata,
            host_domains,
            download_domains,
            sandbox,
            pre,
            host,
            log: crate::ExecutionLog::disabled(),
        })
    }

    /// Records this resolver's invocations in the plugin execution history.
    #[must_use]
    pub fn with_execution_log(mut self, log: Arc<crate::ExecutionLog>) -> Self {
        self.log = log;
        self
    }

    /// Asks the guest whether it claims `url`.
    ///
    /// The host's own `matches` only consults the manifest; this is the guest's answer, and
    /// the two disagreeing is a plugin that link intake routes work to which it then refuses.
    pub async fn guest_claims(&self, url: &str) -> Result<bool, Failure> {
        if crate::foreign_address::carries_marker(url) {
            return Ok(false);
        }
        let identity = ClientIdentity {
            account_id: None,
            proxy_profile_id: None,
            tls_revision: 0,
        };
        let mut store = self.store(identity)?;
        let bindings = self.bindings(&mut store).await?;
        bindings
            .rdownloader_plugin_resolver()
            .call_match_url(&mut store, url)
            .await
            .map_err(component_failure)
    }

    /// Times one call and files what became of it.
    async fn recorded<T, F>(&self, operation: &'static str, call: F) -> Result<T, Failure>
    where
        F: std::future::Future<Output = Result<T, Failure>>,
    {
        let invocation = self.log.begin(
            &self.metadata.plugin_id.to_string(),
            &self.metadata.name,
            &self.metadata.version,
            "resolver",
            operation,
        );
        let result = call.await;
        self.log.finish(invocation, &result);
        result
    }

    async fn bindings(
        &self,
        store: &mut wasmtime::Store<PluginStoreState>,
    ) -> Result<ResolverPlugin, Failure> {
        self.pre
            .instantiate_async(store)
            .await
            .map_err(component_failure)
    }

    fn store(
        &self,
        identity: ClientIdentity,
    ) -> Result<wasmtime::Store<PluginStoreState>, Failure> {
        self.sandbox
            .create_invocation_store(self.host_domains.clone(), Arc::clone(&self.host), identity)
            .map_err(|error| {
                Failure::coded(
                    FailureKind::Permanent,
                    "plugin.execution_failed",
                    format!("Plugin execution failed: {error:#}"),
                )
            })
    }
}

#[async_trait]
impl Resolver for ComponentResolver {
    fn metadata(&self) -> &ResolverMetadata {
        &self.metadata
    }

    fn matches(&self, url: &Url) -> bool {
        domain_allowed(url, &self.metadata.domains)
    }

    async fn check_account(&self, account_id: AccountId) -> Result<AccountStatus, Failure> {
        self.recorded("check_account", self.check_account_inner(account_id))
            .await
    }

    async fn resolve(
        &self,
        request: rd_plugin_api::ResolveRequest,
    ) -> Result<ResolvedDownload, Failure> {
        self.recorded("resolve", self.resolve_inner(request)).await
    }

    async fn hosters(&self, account_id: AccountId) -> Result<Vec<String>, Failure> {
        self.recorded("hosters", self.hosters_inner(account_id))
            .await
    }

    async fn check(&self, request: CheckRequest) -> Result<Vec<LinkCheckResult>, Failure> {
        self.recorded("check", self.check_inner(request)).await
    }
}

/// The guest-facing bodies. Split out so every one of them is timed and recorded by the
/// trait method above rather than by remembering to do it in each.
impl ComponentResolver {
    async fn check_account_inner(&self, account_id: AccountId) -> Result<AccountStatus, Failure> {
        let identity = ClientIdentity {
            account_id: Some(account_id),
            proxy_profile_id: None,
            tls_revision: 0,
        };
        let mut store = self.store(identity)?;
        let bindings = self.bindings(&mut store).await?;
        let result = bindings
            .rdownloader_plugin_resolver()
            .call_check_account(&mut store, &account_id.to_string())
            .await
            .map_err(component_failure)?
            .map_err(from_wit_failure)?;
        Ok(AccountStatus {
            valid: result.valid,
            premium: result.premium,
            label: crate::account_label::from_wit_label(result.label)?,
            traffic_left: result
                .traffic_left
                .map(ByteCount::new)
                .transpose()
                .map_err(permanent)?,
        })
    }

    async fn resolve_inner(
        &self,
        request: rd_plugin_api::ResolveRequest,
    ) -> Result<ResolvedDownload, Failure> {
        if !self.matches(&request.url) {
            return Err(Failure::coded(
                FailureKind::Unsupported,
                "plugin.url_outside_domains",
                "URL is outside the plugin domains",
            ));
        }
        if crate::foreign_address::carries_marker(request.url.as_str()) {
            return Err(crate::foreign_address::refused());
        }
        let expected_identity = request.client.clone();
        let input_url = request.url.to_string();
        let input = wit_types::ResolveRequest {
            url: input_url.clone(),
            client: to_wit_identity(&request.client),
        };
        let mut store = self.store(request.client)?;
        let bindings = self.bindings(&mut store).await?;
        let guest = bindings.rdownloader_plugin_resolver();
        if !guest
            .call_match_url(&mut store, &input_url)
            .await
            .map_err(component_failure)?
        {
            return Err(Failure::coded(
                FailureKind::Unsupported,
                "plugin.url_rejected",
                "Plugin rejected the input URL",
            ));
        }
        let result = guest
            .call_resolve(&mut store, &input)
            .await
            .map_err(component_failure)?
            .map_err(from_wit_failure)?;
        from_wit_download(result, expected_identity, &self.download_domains)
    }

    async fn hosters_inner(&self, account_id: AccountId) -> Result<Vec<String>, Failure> {
        let identity = ClientIdentity {
            account_id: Some(account_id),
            proxy_profile_id: None,
            tls_revision: 0,
        };
        let mut store = self.store(identity)?;
        let bindings = self.bindings(&mut store).await?;
        let hosters = bindings
            .rdownloader_plugin_resolver()
            .call_hosters(&mut store, &account_id.to_string())
            .await
            .map_err(component_failure)?
            .map_err(from_wit_failure)?;
        Ok(hosters
            .into_iter()
            .filter(|host| host.len() <= 253 && !host.is_empty())
            .take(10_000)
            .collect())
    }

    async fn check_inner(&self, request: CheckRequest) -> Result<Vec<LinkCheckResult>, Failure> {
        let (urls, mut unknown) = crate::foreign_address::checkable(request.urls);
        if urls.is_empty() {
            return Ok(unknown);
        }
        let requested: Vec<String> = urls.iter().map(ToString::to_string).collect();
        let input = wit_types::CheckRequest {
            urls: requested.clone(),
            client: to_wit_identity(&request.client),
        };
        let mut store = self.store(request.client)?;
        let bindings = self.bindings(&mut store).await?;
        let results = bindings
            .rdownloader_plugin_resolver()
            .call_check(&mut store, &input)
            .await
            .map_err(component_failure)?
            .map_err(from_wit_failure)?;
        let mut checked = results
            .into_iter()
            .map(|result| {
                if !requested.contains(&result.url) {
                    return Err(Failure::coded(
                        FailureKind::Permanent,
                        "plugin.unexpected_url",
                        "Plugin reported a URL that was not requested",
                    ));
                }
                Ok(LinkCheckResult {
                    url: Url::parse(&result.url).map_err(permanent)?,
                    status: match result.status {
                        wit_types::LinkStatus::Online => LinkStatus::Online,
                        wit_types::LinkStatus::Offline => LinkStatus::Offline,
                        wit_types::LinkStatus::Unknown => LinkStatus::Unknown,
                        wit_types::LinkStatus::Cached => LinkStatus::Cached,
                    },
                    file_name: result.file_name,
                    size: result
                        .size
                        .map(ByteCount::new)
                        .transpose()
                        .map_err(permanent)?,
                    media: None,
                })
            })
            .collect::<Result<Vec<_>, Failure>>()?;
        checked.append(&mut unknown);
        Ok(checked)
    }
}

impl wit_types::Host for PluginStoreState {}

impl wit_http::Host for PluginStoreState {
    async fn http_request(
        &mut self,
        method: String,
        url: String,
        query: Vec<wit_http::RequestQuery>,
        headers: Vec<wit_http::RequestHeader>,
        body: Vec<u8>,
    ) -> Result<wit_http::HttpResponse, wit_types::Failure> {
        self.controlled_http_request(method, url, query, headers, body)
            .await
            .map_err(to_wit_failure)
    }
}

impl wit_cookies::Host for PluginStoreState {
    async fn cookies_get(&mut self, account_id: String, url: String) -> Vec<(String, String)> {
        let Ok(account_id) = AccountId::from_str(&account_id) else {
            return Vec::new();
        };
        if self.identity().account_id != Some(account_id) {
            return Vec::new();
        }
        let Ok(url) = Url::parse(&url) else {
            return Vec::new();
        };
        if !domain_allowed(&url, self.allowed_domains()) {
            return Vec::new();
        }
        let Some(host) = self.host() else {
            return Vec::new();
        };
        let cookies = host.cookies_get(account_id, &url).await;
        self.remember_redactions(cookies.iter().map(|(_, value)| value.clone()));
        cookies
    }
}

impl wit_host::Host for PluginStoreState {
    async fn secret_available(&mut self, account_id: String, reference: String) -> bool {
        let Ok(account_id) = AccountId::from_str(&account_id) else {
            return false;
        };
        if self.identity().account_id != Some(account_id) {
            return false;
        }
        let Some(host) = self.host() else {
            return false;
        };
        host.secret_available(account_id, &reference).await
    }

    async fn now_unix_seconds(&mut self) -> u64 {
        match self.host() {
            Some(host) => host.now_unix_seconds(),
            None => u64::try_from(chrono::Utc::now().timestamp()).unwrap_or_default(),
        }
    }

    async fn random_bytes(&mut self, count: u32) -> Vec<u8> {
        random_bytes(count)
    }

    async fn wait(&mut self, seconds: u32) -> Result<(), wit_types::Failure> {
        self.controlled_wait(seconds).await.map_err(to_wit_failure)
    }

    async fn log(&mut self, level: String, message: String) {
        let message = self.redact_log(&message);
        match level.to_ascii_lowercase().as_str() {
            "error" => tracing::error!(plugin = true, "{message}"),
            "warn" => tracing::warn!(plugin = true, "{message}"),
            "debug" | "trace" => tracing::debug!(plugin = true, "{message}"),
            _ => tracing::info!(plugin = true, "{message}"),
        }
    }
}

impl wit_captcha::Host for PluginStoreState {
    async fn solve_captcha(
        &mut self,
        challenge: wit_captcha::CaptchaChallenge,
    ) -> Result<wit_captcha::CaptchaSolution, wit_types::Failure> {
        self.solve_for_token(challenge)
            .await
            .map_err(to_wit_failure)
    }

    async fn solve_challenge(
        &mut self,
        challenge: wit_captcha::CaptchaChallenge,
    ) -> Result<wit_captcha::CaptchaAnswer, wit_types::Failure> {
        let challenge = from_wit_challenge(challenge).map_err(to_wit_failure)?;
        let answer = self
            .controlled_solve_captcha(challenge)
            .await
            .map_err(to_wit_failure)?;
        Ok(match answer {
            CaptchaAnswer::Token(token) => wit_captcha::CaptchaAnswer::Token(token),
            CaptchaAnswer::Point(point) => {
                wit_captcha::CaptchaAnswer::Point(wit_captcha::ClickPoint {
                    x: point.x,
                    y: point.y,
                })
            }
        })
    }
}

impl PluginStoreState {
    /// Waits on the host's clock, bounded by the plugin's wait budget.
    async fn controlled_wait(&mut self, seconds: u32) -> Result<(), Failure> {
        let requested = std::time::Duration::from_secs(u64::from(seconds));
        let Some(granted) = self.claim_wait(requested) else {
            return Err(Failure::coded(
                FailureKind::Permanent,
                "plugin.wait_budget_exhausted",
                "Plugin requested a longer wait than its budget allows",
            ));
        };
        let host = self.host().ok_or_else(host_disconnected)?;
        let identity = self.identity().clone();
        let seconds = u32::try_from(granted.as_secs()).unwrap_or(u32::MAX);
        host.wait(&identity, seconds).await
    }

    /// The token-shaped entry point. A click-point challenge is refused here, before any
    /// budget is claimed and before a service or a person is asked: `captcha-solution`
    /// cannot carry a point, so an answer would only be thrown away (RD-110-15).
    async fn solve_for_token(
        &mut self,
        challenge: wit_captcha::CaptchaChallenge,
    ) -> Result<wit_captcha::CaptchaSolution, Failure> {
        let challenge = from_wit_challenge(challenge)?;
        if challenge.answers_with_point() {
            return Err(answer_shape_refused());
        }
        match self.controlled_solve_captcha(challenge).await? {
            CaptchaAnswer::Token(token) => Ok(wit_captcha::CaptchaSolution { token }),
            CaptchaAnswer::Point(_) => Err(answer_shape_refused()),
        }
    }

    /// Solves a challenge the guest already had validated, within the waiting budget.
    async fn controlled_solve_captcha(
        &mut self,
        challenge: rd_plugin_api::CaptchaChallenge,
    ) -> Result<CaptchaAnswer, Failure> {
        let host = self.host().ok_or_else(host_disconnected)?;
        let identity = self.identity().clone();
        // A widget challenge names a page the browser extension will open, so it is an
        // outbound target like any other and is held to the same manifest boundary as
        // `net_http` (RD-107-03). Checked here rather than in the browser alone: the browser
        // is the second fence, and a plugin must not be able to name a page outside its
        // manifest even when none is attached.
        if let Some(page) = challenge.page_url() {
            let page = Url::parse(page).map_err(permanent)?;
            if !domain_allowed(&page, self.allowed_domains()) {
                return Err(captcha_target_refused());
            }
        }
        // Solving is a wait too: it runs on a service or a person, so it must not count
        // against the plugin's compute timeout. Reserve what answering may actually take —
        // a configured manual timeout can be far longer than one solver round trip — and
        // settle for the rest of the budget when that no longer fits.
        let wanted = host.captcha_allowance().await.min(self.wait_budget());
        let granted = (wanted >= MIN_CAPTCHA_ALLOWANCE)
            .then(|| self.claim_wait(wanted))
            .flatten();
        let Some(granted) = granted else {
            return Err(Failure::coded(
                FailureKind::Permanent,
                "plugin.wait_budget_exhausted",
                "Plugin requested a captcha with no waiting budget left",
            ));
        };
        // The host answers within the time it was granted, so the resolver behind it stays
        // inside the execution deadline that reservation just bought.
        host.solve_captcha(&identity, challenge, granted).await
    }

    async fn controlled_http_request(
        &mut self,
        method: String,
        url: String,
        query: Vec<wit_http::RequestQuery>,
        headers: Vec<wit_http::RequestHeader>,
        body: Vec<u8>,
    ) -> Result<wit_http::HttpResponse, Failure> {
        // Only the `{{secret}}` the plugin wrote literally stays the granted marker; the same
        // text encoded into a path from somebody else's file name does not (RD-120-66).
        let url = crate::foreign_address::guest_url(&url).map_err(permanent)?;
        if !domain_allowed(&url, self.allowed_domains()) {
            return Err(Failure::coded(
                FailureKind::Permanent,
                "plugin.http_target_not_allowed",
                "Plugin HTTP target is not allowed by the manifest",
            ));
        }
        let host = self.host().ok_or_else(|| {
            Failure::coded(
                FailureKind::Permanent,
                "plugin.host_disconnected",
                "Plugin host is not connected",
            )
        })?;
        let identity = self.identity().clone();
        let granted_secret = self.granted_secret().map(str::to_owned);
        let authority = self.request_authority();
        let write_methods = self.write_methods();
        // The execution deadline is wall-clock, so every second spent waiting for the hoster
        // used to be charged against the plugin's compute budget — with the HTTP timeout set to
        // the same 15s as that budget, one slow response was enough to end the call as
        // "plugin.timeout". Ten downloads at once made that routine. The waiting time is
        // credited back below, exactly as the transfer sink already does.
        let started = Instant::now();
        // Every redirect hop is put to the same list as the first address *before* it is
        // followed (RD-130-24). Checking only where the request ended let a `307` carry the
        // plugin's body to a host outside its domains first and be refused afterwards.
        let domains: Arc<[String]> = Arc::from(self.allowed_domains());
        let gate: rd_http::RedirectGate = Arc::new(move |hop: &Url| domain_allowed(hop, &domains));
        let request = host.http_request(
            &identity,
            HostHttpRequest {
                method,
                url,
                query: query
                    .into_iter()
                    .map(|value| HostRequestValue {
                        name: value.name,
                        value_template: value.value_template,
                    })
                    .collect(),
                headers: headers
                    .into_iter()
                    .map(|value| HostRequestValue {
                        name: value.name,
                        value_template: value.value_template,
                    })
                    .collect(),
                body,
                // From the store, never from the guest: a plugin writes `{{secret}}`
                // without naming a reference precisely because naming one is not its
                // business.
                granted_secret: granted_secret.clone(),
                authority,
                write_methods,
            },
        );
        let response = rd_http::with_redirect_gate(gate, request).await;
        // Credited on the failure path too: a request that timed out still spent that time
        // waiting on the network rather than computing.
        self.credit_host_time(started.elapsed());
        let response = response?;
        // A hop the gate refused comes back unfollowed, as the redirect itself; it is refused
        // here exactly like a request that had ended outside the list, so a plugin sees one
        // behaviour for "your redirect leaves your domains" whichever way it was caught.
        let outside = if domain_allowed(&response.final_url, self.allowed_domains()) {
            unfollowed_redirect(&response)
                .filter(|target| !domain_allowed(target, self.allowed_domains()))
        } else {
            Some(response.final_url.clone())
        };
        if let Some(outside) = outside {
            let redirect_host = outside.host_str().unwrap_or("(no host)");
            return Err(Failure::coded(
                FailureKind::Permanent,
                "plugin.redirect_outside_domains",
                format!("Plugin HTTP redirect to {redirect_host} leaves the manifest domains"),
            )
            .with_param("host", redirect_host));
        }
        self.account_response_bytes(response.body.len())
            .map_err(permanent)?;
        Ok(wit_http::HttpResponse {
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
}

fn host_disconnected() -> Failure {
    Failure::coded(
        FailureKind::Permanent,
        "plugin.host_disconnected",
        "Plugin host is not connected",
    )
}

/// Refuses a `solve-captcha` call for a challenge whose answer is not a token.
fn answer_shape_refused() -> Failure {
    Failure::coded(
        FailureKind::Permanent,
        "captcha.answer_shape",
        "A click-point captcha answers with a point: call solve-challenge, not solve-captcha",
    )
}

/// Converts a guest challenge, rejecting oversized image payloads and unusable site keys
/// before they reach a paid solver service.
fn from_wit_challenge(
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
fn unfollowed_redirect(response: &rd_plugin_api::HostHttpResponse) -> Option<Url> {
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

fn permanent(error: impl std::fmt::Display) -> Failure {
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
    let mut failure = Failure::new(
        match value.category {
            wit_types::FailureKind::Transient(delay) => FailureKind::Transient {
                retry_after_seconds: delay,
            },
            wit_types::FailureKind::Permanent => FailureKind::Permanent,
            wit_types::FailureKind::Offline => FailureKind::Offline,
            wit_types::FailureKind::AuthRequired => FailureKind::AuthRequired,
            wit_types::FailureKind::AccountInvalid => FailureKind::AccountInvalid,
            wit_types::FailureKind::RateLimited(delay) => FailureKind::RateLimited {
                retry_after_seconds: delay,
            },
            wit_types::FailureKind::NeedsCaptcha => FailureKind::NeedsCaptcha,
            wit_types::FailureKind::Unsupported => FailureKind::Unsupported,
            wit_types::FailureKind::IpBlocked(delay) => FailureKind::IpBlocked {
                retry_after_seconds: delay,
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

/// The host's answer to `host.random-bytes`: `count` bytes from the operating system's
/// generator, or nothing at all.
///
/// Refusing an oversized request with an empty list rather than a truncated one matters: a
/// guest that is handed fewer bytes than it asked for must notice, and silently shortening the
/// answer is exactly how a 32-byte verifier turns into an 8-byte one nobody spots.
fn random_bytes(count: u32) -> Vec<u8> {
    if count == 0 || count > MAX_RANDOM_BYTES {
        return Vec::new();
    }
    let mut bytes = vec![0u8; count as usize];
    // `rand::rng()` is the OS-seeded, periodically reseeded CSPRNG the rest of the application
    // draws its keys and nonces from; nothing here is derived from a clock or an identifier.
    rand::rng().fill_bytes(&mut bytes);
    bytes
}

#[cfg(test)]
#[path = "component_tests.rs"]
mod tests;
