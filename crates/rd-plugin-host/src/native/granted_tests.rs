use std::sync::atomic::{AtomicBool, Ordering};

use rd_plugin_api::{HostHttpResponse, WidgetChallenge};

use super::*;

/// Records whether a call made it through the grant check to the real host.
#[derive(Default)]
struct Recording {
    reached: AtomicBool,
}

#[async_trait]
impl ResolverHost for Recording {
    async fn http_request(
        &self,
        _client: &ClientIdentity,
        request: HostHttpRequest,
    ) -> Result<HostHttpResponse, Failure> {
        self.reached.store(true, Ordering::SeqCst);
        Ok(HostHttpResponse {
            status: 200,
            final_url: request.url,
            headers: Vec::new(),
            body: Vec::new(),
        })
    }

    async fn cookies_get(&self, _account_id: AccountId, _url: &Url) -> Vec<(String, String)> {
        self.reached.store(true, Ordering::SeqCst);
        vec![("session".to_owned(), "value".to_owned())]
    }

    async fn secret_available(&self, _account_id: AccountId, _reference: &str) -> bool {
        self.reached.store(true, Ordering::SeqCst);
        true
    }

    async fn solve_captcha(
        &self,
        _client: &ClientIdentity,
        _challenge: CaptchaChallenge,
        _time_limit: Duration,
    ) -> Result<CaptchaAnswer, Failure> {
        self.reached.store(true, Ordering::SeqCst);
        Ok(CaptchaAnswer::Token("solved".to_owned()))
    }
}

fn identity() -> ClientIdentity {
    ClientIdentity {
        account_id: None,
        proxy_profile_id: None,
        tls_revision: 0,
    }
}

fn challenge() -> CaptchaChallenge {
    CaptchaChallenge::RecaptchaV2(WidgetChallenge {
        site_key: "key".to_owned(),
        page_url: "https://ddownload.com/".to_owned(),
        invisible: false,
    })
}

fn request(url: &str) -> HostHttpRequest {
    HostHttpRequest {
        method: "GET".to_owned(),
        url: url.parse().expect("url"),
        query: Vec::new(),
        headers: Vec::new(),
        body: Vec::new(),
        granted_secret: None,
        authority: rd_plugin_api::RequestAuthority::Provider,
        write_methods: false,
    }
}

const PREMIUMIZE: &str = include_str!("../../../../plugins/premiumize/manifest.toml");
const DDOWNLOAD: &str = include_str!("../../../../plugins/ddownload/manifest.toml");
const ONEFICHIER: &str = include_str!("../../../../plugins/onefichier/manifest.toml");

fn wrap(manifest: &str) -> (Arc<Recording>, Arc<dyn ResolverHost>) {
    let inner = Arc::new(Recording::default());
    let manifest: PluginManifest = toml::from_str(manifest).expect("bundled manifest");
    let host = GrantedHost::confine(inner.clone(), &manifest);
    (inner, host)
}

/// Premiumize is an API-key multihoster: no captcha, no cookies. The component of the
/// same plugin cannot even link those interfaces, and the narrowed host must not let
/// anything under its manifest reach past them either.
#[tokio::test]
async fn a_plugin_without_the_captcha_grant_is_refused_by_the_narrowed_host_too() {
    let (inner, host) = wrap(PREMIUMIZE);
    let failure = host
        .solve_captcha(&identity(), challenge(), Duration::from_secs(60))
        .await
        .expect_err("premiumize declares no captcha capability");
    assert_eq!(
        failure.code.as_deref(),
        Some("plugin.capability_not_granted")
    );
    assert!(!inner.reached.load(Ordering::SeqCst));
}

/// A widget captcha ends with a person looking at that page in the desktop agent's
/// WebView (RD-107-03). A plugin that may reach its own hoster must not be able to point
/// that window at somebody else's site, so the page is held to the manifest's domains.
#[tokio::test]
async fn a_widget_captcha_page_outside_the_plugins_domains_is_refused() {
    let (inner, host) = wrap(DDOWNLOAD);
    let elsewhere = CaptchaChallenge::Turnstile(WidgetChallenge {
        site_key: "0x4AAA".to_owned(),
        page_url: "https://www.premiumize.me/login".to_owned(),
        invisible: false,
    });

    let failure = host
        .solve_captcha(&identity(), elsewhere, Duration::from_secs(60))
        .await
        .expect_err("outside ddownload's declared domains");

    assert_eq!(
        failure.code.as_deref(),
        Some("plugin.captcha_target_not_allowed")
    );
    assert!(
        !inner.reached.load(Ordering::SeqCst),
        "the broker must never see a challenge the manifest does not cover"
    );
}

/// The other direction: the hoster's own page still reaches the broker, and an image
/// challenge — which names no page at all — is untouched by the check.
#[tokio::test]
async fn the_plugins_own_page_and_an_image_challenge_still_reach_the_broker() {
    let (inner, host) = wrap(DDOWNLOAD);

    host.solve_captcha(&identity(), challenge(), Duration::from_secs(60))
        .await
        .expect("ddownload's own page passes");
    assert!(inner.reached.load(Ordering::SeqCst));

    let image = CaptchaChallenge::Image(rd_plugin_api::ImageChallenge {
        mime: "image/png".to_owned(),
        data: b"x".to_vec(),
        prompt: None,
    });
    host.solve_captcha(&identity(), image, Duration::from_secs(60))
        .await
        .expect("an image challenge names no page");
}

#[tokio::test]
async fn a_plugin_without_the_cookies_grant_reads_no_cookies() {
    let (inner, host) = wrap(PREMIUMIZE);
    let cookies = host
        .cookies_get(
            AccountId::new(),
            &"https://www.premiumize.me/".parse().expect("url"),
        )
        .await;
    assert!(cookies.is_empty());
    assert!(!inner.reached.load(Ordering::SeqCst));
}

/// The registry gate underneath allows every bundled plugin's domains at once, so
/// without a per-plugin check ddownload could have reached premiumize's API.
#[tokio::test]
async fn a_request_outside_the_plugins_own_domains_is_refused() {
    let (inner, host) = wrap(DDOWNLOAD);
    let failure = host
        .http_request(
            &identity(),
            request("https://www.premiumize.me/api/account/info"),
        )
        .await
        .expect_err("outside ddownload's declared domains");
    assert_eq!(
        failure.code.as_deref(),
        Some("plugin.http_target_not_allowed")
    );
    assert!(!inner.reached.load(Ordering::SeqCst));

    host.http_request(&identity(), request("https://ddownload.com/file"))
        .await
        .expect("its own domain still passes");
    assert!(inner.reached.load(Ordering::SeqCst));
}

/// The free flows reach hosts no link is ever posted for — an API endpoint, a CDN. Those
/// live in `net_http` and nowhere else, so the per-plugin gate has to allow them.
#[tokio::test]
async fn the_free_flows_api_and_delivery_hosts_stay_requestable() {
    let (_, onefichier) = wrap(ONEFICHIER);
    onefichier
        .http_request(
            &identity(),
            request("https://api.1fichier.com/v1/download/get_token.cgi"),
        )
        .await
        .expect("1fichier's API host is granted");

    let (_, ddownload) = wrap(DDOWNLOAD);
    ddownload
        .http_request(&identity(), request("https://s12.zeuscdn.org/file.bin"))
        .await
        .expect("ddownload's delivery wildcard is granted");
}

#[tokio::test]
async fn only_the_granted_secret_reference_is_visible() {
    let (inner, host) = wrap(DDOWNLOAD);
    assert!(
        !host
            .secret_available(AccountId::new(), "premiumize_api_key")
            .await
    );
    assert!(!inner.reached.load(Ordering::SeqCst));
    assert!(
        host.secret_available(AccountId::new(), "ddownload_api_key")
            .await
    );
    assert!(inner.reached.load(Ordering::SeqCst));
}

const TORBOX_JOBS: &str = include_str!("../../../../plugins/torbox-jobs/manifest.toml");

fn with_header(url: &str, value: &str) -> HostHttpRequest {
    let mut request = request(url);
    request.headers.push(rd_plugin_api::HostRequestValue {
        name: "Authorization".to_owned(),
        value_template: value.to_owned(),
    });
    request
}

/// A named vault reference in an outgoing request is held to `capabilities.secrets`, the
/// same grant `secret_available` and `derive_from_secret` check.
#[tokio::test]
async fn a_request_naming_an_undeclared_secret_is_refused() {
    let (inner, host) = wrap(TORBOX_JOBS);
    for template in [
        "Bearer {{secret:premiumize_api_key}}",
        "Basic {{basic:premiumize_api_key}}",
        "Bearer {{secret:torbox_api_key}} {{secret:premiumize_api_key}}",
    ] {
        let failure = host
            .http_request(
                &identity(),
                with_header("https://api.torbox.app/v1/api/user/me", template),
            )
            .await
            .expect_err("premiumize_api_key is not declared by torbox-jobs");
        assert_eq!(
            failure.code.as_deref(),
            Some("plugin.secret_target_not_allowed")
        );
    }
    assert!(!inner.reached.load(Ordering::SeqCst));
}

/// The declared reference still passes, and so does a host-minted renewal reference, which
/// no manifest can name and which the host underneath checks against the account itself.
#[tokio::test]
async fn a_declared_secret_and_a_host_minted_reference_still_pass() {
    let (inner, host) = wrap(TORBOX_JOBS);
    host.http_request(
        &identity(),
        with_header(
            "https://api.torbox.app/v1/api/user/me",
            "Bearer {{secret:torbox_api_key}}",
        ),
    )
    .await
    .expect("torbox_api_key is declared");
    assert!(inner.reached.load(Ordering::SeqCst));

    let (inner, host) = wrap(TORBOX_JOBS);
    host.http_request(
        &identity(),
        with_header(
            "https://api.torbox.app/v1/api/user/me",
            "{{secret:vault://019d0000-0000-7000-8000-000000000001}}",
        ),
    )
    .await
    .expect("a host-minted reference is decided underneath");
    assert!(inner.reached.load(Ordering::SeqCst));
}
