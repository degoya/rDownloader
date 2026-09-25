//! What a plugin may ask of whichever host is running it.

use crate::types::{
    CaptchaAnswer, CaptchaChallenge, CaptchaSolution, Failure, HttpRequest, HttpResponse,
};

/// The host, as the protocol logic sees it.
///
/// One method per capability, and nothing else: there is no file system here, no clock the
/// plugin controls and no way to reach a host that is not carrying the request. What an
/// adapter cannot do — because the manifest did not grant it — it reports as a failure, so
/// logic written against this trait behaves the same under both.
#[allow(async_fn_in_trait)] // Used generically, never as a trait object; boxing would be waste.
pub trait PluginHost {
    /// Performs one request within the plugin's declared domains.
    async fn http(&self, request: HttpRequest) -> Result<HttpResponse, Failure>;

    /// Cookies stored for one account and URL. The jar itself never reaches the plugin.
    async fn cookies(&self, account_id: &str, url: &str) -> Vec<(String, String)>;

    /// Whether a credential exists under `reference`. Never its value.
    async fn secret_available(&self, account_id: &str, reference: &str) -> bool;

    /// Waits out a hoster countdown on the host's clock.
    async fn wait(&self, seconds: u32) -> Result<(), Failure>;

    /// Hands a captcha to the application, which solves it or asks the user. Token-shaped:
    /// a `ClickPoint` challenge is refused here with `captcha.answer_shape`.
    async fn solve_captcha(&self, challenge: CaptchaChallenge) -> Result<CaptchaSolution, Failure>;

    /// The same, for every kind: the answer comes back in the shape the challenge has.
    async fn solve_challenge(&self, challenge: CaptchaChallenge) -> Result<CaptchaAnswer, Failure>;

    /// Seconds since the Unix epoch. Not a capability — the guest simply has no clock.
    async fn now_unix_seconds(&self) -> u64;

    /// Cryptographically strong random bytes. Not a capability either — the guest simply has
    /// no random source, and a value derived from the clock and an account id is one anybody
    /// can recompute.
    ///
    /// An answer shorter than `count` — an empty one, for a request above the host's cap — is a
    /// refusal. Treat it as a failure; never stretch it into the value you needed.
    async fn random_bytes(&self, count: u32) -> Vec<u8>;

    /// Writes one diagnostic line. Redacted by the host before it reaches a log.
    fn log(&self, level: &str, message: &str);
}
