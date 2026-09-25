//! Captcha solving for resolvers: a configured solver service, a person, or neither.
//!
//! Free downloads are gated by captchas, which a sandboxed resolver plugin cannot solve. It
//! hands the challenge to this broker, which either buys an answer from a solver service or
//! asks the user, and returns the token to submit.
//!
//! Widget captchas (reCAPTCHA, hCaptcha, Turnstile) are bound to the hoster's domain and
//! cannot be rendered anywhere else — measured against DDownload's real site key, a page
//! served from `localhost` earns `110200 - domain not allowed`. Three things can therefore
//! answer one: a solver service; the browser extension, which opens the hoster's own page in
//! the person's real browser and hands back the token earned there (RD-108-02); or the
//! desktop capture agent's WebView, which does the same in an embedded engine that Turnstile
//! was measured to refuse (RD-107-03). Image captchas can always be shown to the user, and so
//! can click-point captchas, whose answer is a spot in the picture rather than text. A
//! CutCaptcha is answered by a solver service alone: no browser reads its token, so without a
//! service it is refused at once rather than queued (RD-110-15).

mod manual;
mod presence;
mod settings;
mod solver;

use std::{sync::Arc, time::Duration};

use rd_core::{CaptchaId, EventEnvelope, EventKind, Failure, FailureKind};
use rd_db::Database;
use rd_plugin_api::{CaptchaAnswer, CaptchaChallenge, ClickPoint};
use secrecy::ExposeSecret;

pub use manual::{AnswerSource, CaptchaKind, PendingCaptcha, PendingWidget, SubmitOutcome};
pub use presence::{BROWSER_EXTENSION_PRESENCE_WINDOW, CaptchaAnswerers};
pub use settings::{CaptchaSettings, DEFAULT_ENDPOINT, SETTINGS_KEY, SolverKind};

/// Timeout for one solver-service HTTP call.
const SOLVER_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Longest one solver-service round trip can take: the grace period before the first poll,
/// the polling window, and one last request running into its timeout.
const SOLVER_CEILING: Duration = Duration::from_secs(200);

/// Added to the time a captcha may need, so reserving it never cuts the answer short.
const ALLOWANCE_MARGIN: Duration = Duration::from_secs(15);

/// Smallest useful reservation; below this even a fast solver service cannot answer.
const MIN_ALLOWANCE: Duration = Duration::from_secs(30);

struct Inner {
    database: Database,
    secrets: rd_secrets::SecretStore,
    manual: manual::ManualQueue,
    http: reqwest::Client,
    /// When a browser extension last polled the waiting widgets. In memory only, like the
    /// queue itself: presence is a live fact, not a setting.
    browser_extension_seen: std::sync::Mutex<Option<chrono::DateTime<chrono::Utc>>>,
}

/// Shared entry point for solving captchas; cloning shares one queue.
#[derive(Clone)]
pub struct CaptchaBroker {
    inner: Arc<Inner>,
}

impl CaptchaBroker {
    /// Creates a broker reading its configuration from the settings table on every call, so
    /// changing the solver in the UI takes effect without a restart.
    #[must_use]
    pub fn new(database: Database, secrets: rd_secrets::SecretStore) -> Self {
        let http = reqwest::Client::builder()
            .timeout(SOLVER_REQUEST_TIMEOUT)
            .user_agent(concat!("rDownloader/", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_default();
        Self {
            inner: Arc::new(Inner {
                database,
                secrets,
                manual: manual::ManualQueue::default(),
                http,
                browser_extension_seen: std::sync::Mutex::new(None),
            }),
        }
    }

    /// Current configuration, with stored values clamped to usable ranges.
    pub async fn settings(&self) -> CaptchaSettings {
        let stored = self
            .inner
            .database
            .get_setting(SETTINGS_KEY)
            .await
            .ok()
            .flatten();
        stored
            .and_then(|value| serde_json::from_value::<CaptchaSettings>(value).ok())
            .unwrap_or_default()
            .sanitized()
    }

    /// Every captcha currently waiting for a person, oldest first.
    #[must_use]
    pub fn pending(&self) -> Vec<PendingCaptcha> {
        self.inner.manual.pending()
    }

    /// Every widget challenge currently waiting, as the desktop agent may see it.
    ///
    /// Narrower than [`Self::pending`] on purpose: this is read with a capture token.
    #[must_use]
    pub fn pending_widgets(&self) -> Vec<PendingWidget> {
        self.inner.manual.pending_widgets()
    }

    /// Records that a browser extension just asked for the waiting widgets, so the web
    /// interface can say whether one is around to answer (RD-108-02).
    pub fn note_browser_extension(&self) {
        if let Ok(mut seen) = self.inner.browser_extension_seen.lock() {
            *seen = Some(chrono::Utc::now());
        }
    }

    /// Who is currently around to answer a widget captcha, as far as the service can tell.
    #[must_use]
    pub fn answerers(&self) -> CaptchaAnswerers {
        let seen = self
            .inner
            .browser_extension_seen
            .lock()
            .ok()
            .and_then(|seen| *seen);
        presence::answerers(seen, chrono::Utc::now())
    }

    /// Submits an answer a person typed into the web interface.
    ///
    /// Widgets stay refused here — see [`AnswerSource`] — and so is a click-point captcha,
    /// which takes a point through [`Self::submit_click`].
    pub fn submit(&self, id: CaptchaId, token: String) -> SubmitOutcome {
        self.deliver(id, Some(CaptchaAnswer::Token(token)), AnswerSource::Typed)
    }

    /// Submits the spot a person clicked in a click-point captcha, in pixels of the image as
    /// served. Refused with [`SubmitOutcome::WrongAnswerShape`] for every other kind.
    pub fn submit_click(&self, id: CaptchaId, point: ClickPoint) -> SubmitOutcome {
        self.deliver(id, Some(CaptchaAnswer::Point(point)), AnswerSource::Typed)
    }

    /// Submits a widget token read out of the hoster's own page — by the browser extension
    /// in the person's real browser (RD-108-02) or by the desktop agent's WebView.
    ///
    /// The token is passed straight to the waiting resolver and never stored, logged or
    /// announced: it is a single-use credential that expires in minutes (RD-107-03).
    pub fn submit_from_browser(&self, id: CaptchaId, token: String) -> SubmitOutcome {
        self.deliver(id, Some(CaptchaAnswer::Token(token)), AnswerSource::Browser)
    }

    /// Declines a captcha, failing the download that waits for it.
    pub fn skip(&self, id: CaptchaId) -> bool {
        self.deliver(id, None, AnswerSource::Typed) == SubmitOutcome::Delivered
    }

    /// Ends a widget challenge the browser extension found no widget for on the hoster's page,
    /// failing the waiting resolver with `captcha.page_without_widget` instead of letting it
    /// wait out its timeout (RD-120-45).
    pub fn report_page_without_widget(&self, id: CaptchaId) -> SubmitOutcome {
        let outcome = self.inner.manual.report_page_without_widget(id);
        if outcome == SubmitOutcome::Delivered {
            self.announce();
        }
        outcome
    }

    fn deliver(
        &self,
        id: CaptchaId,
        answer: Option<CaptchaAnswer>,
        source: AnswerSource,
    ) -> SubmitOutcome {
        let outcome = self.inner.manual.resolve(id, answer, source);
        if outcome == SubmitOutcome::Delivered {
            self.announce();
        }
        outcome
    }

    /// Waiting time one challenge may need with the current configuration, so the plugin
    /// host can reserve it before a resolver hands a captcha over.
    pub async fn solve_allowance(&self) -> Duration {
        let settings = self.settings().await;
        let mut needed = Duration::ZERO;
        if settings.has_solver() {
            needed += SOLVER_CEILING;
        }
        // A failed service falls back to the user, so both can run for one challenge.
        if settings.manual_enabled {
            needed += Duration::from_secs(settings.manual_timeout_seconds);
        }
        (needed + ALLOWANCE_MARGIN).max(MIN_ALLOWANCE)
    }

    /// Solves a challenge within the waiting time the host reserved, preferring the solver
    /// service and falling back to the user.
    pub async fn solve_within(
        &self,
        challenge: CaptchaChallenge,
        limit: Duration,
    ) -> Result<CaptchaAnswer, Failure> {
        // The individual steps keep themselves inside `limit`; this only stops a solver
        // service that neither answers nor times out from outliving the download.
        match tokio::time::timeout(limit + ALLOWANCE_MARGIN, self.attempt(challenge, limit)).await {
            Ok(result) => result,
            Err(_) => Err(Failure::coded(
                FailureKind::NeedsCaptcha,
                "captcha.timeout",
                "Nobody solved the captcha in time",
            )),
        }
    }

    async fn attempt(
        &self,
        challenge: CaptchaChallenge,
        limit: Duration,
    ) -> Result<CaptchaAnswer, Failure> {
        let settings = self.settings().await;
        let mut service_failure = None;
        if settings.has_solver() {
            match self.solve_with_service(&settings, &challenge).await {
                Ok(answer) => return Ok(answer),
                // A rejected key or an empty balance is a configuration problem: asking the
                // user instead would hide it behind an unexplained prompt every time.
                Err(failure) if failure.category == FailureKind::Permanent => {
                    return Err(failure);
                }
                Err(failure) => {
                    tracing::warn!(%failure, "captcha solver failed, falling back");
                    service_failure = Some(failure);
                }
            }
        }
        // A picture — an image captcha, or a click-point captcha — is offered to the user to
        // answer here. A widget captcha cannot be answered anywhere but the hoster's own page,
        // so it goes to the browser extension, which opens that page and hands back the token
        // the person earned (RD-108-02) — and to the web interface as a hint naming the
        // hoster, for an installation without one. It is not offered at all when a solver is
        // configured and merely failed: there, pointing at the solver settings would mislead.
        // A CutCaptcha is offered to nobody: no browser reads its token, so queueing it would
        // only wait out a timeout for an answer that cannot come (RD-110-15).
        let offered = match answerers(&challenge) {
            Answerers::Person => settings.manual_enabled,
            Answerers::Browser => settings.manual_enabled && !settings.has_solver(),
            Answerers::ServiceOnly => false,
        };
        if offered {
            let timeout = Duration::from_secs(settings.manual_timeout_seconds).min(limit);
            let widget = answerers(&challenge) == Answerers::Browser;
            return self.solve_manually(&challenge, timeout, widget).await;
        }
        if let Some(failure) = service_failure {
            return Err(failure);
        }
        Err(no_solver(&challenge))
    }

    /// Checks that the configured solver answers and the key is accepted, reporting the
    /// remaining balance. `endpoint` and `api_key` override the stored configuration, so a
    /// key can be tried before it is saved.
    ///
    /// The key never leaves this crate: the caller passes one in or names none at all.
    pub async fn test_solver(
        &self,
        endpoint: Option<String>,
        api_key: Option<String>,
    ) -> Result<f64, Failure> {
        let settings = self.settings().await;
        let endpoint = endpoint.unwrap_or(settings.endpoint);
        let key = match api_key {
            Some(key) => secrecy::SecretString::from(key),
            None => {
                let reference = settings.api_key_ref.as_deref().ok_or_else(key_missing)?;
                self.inner
                    .secrets
                    .get(reference)
                    .await
                    .map_err(|error| key_unreadable(&error))?
            }
        };
        solver::balance(&self.inner.http, &endpoint, key.expose_secret()).await
    }

    async fn solve_with_service(
        &self,
        settings: &CaptchaSettings,
        challenge: &CaptchaChallenge,
    ) -> Result<CaptchaAnswer, Failure> {
        let reference = settings.api_key_ref.as_deref().ok_or_else(key_missing)?;
        let key = self
            .inner
            .secrets
            .get(reference)
            .await
            .map_err(|error| key_unreadable(&error))?;
        solver::solve(
            &self.inner.http,
            &settings.endpoint,
            key.expose_secret(),
            challenge,
        )
        .await
    }

    /// Offers a challenge to the user. `widget` marks a widget captcha, which the browser
    /// extension or the desktop agent answers on the hoster's page and which the web
    /// interface can only explain.
    async fn solve_manually(
        &self,
        challenge: &CaptchaChallenge,
        timeout: Duration,
        widget: bool,
    ) -> Result<CaptchaAnswer, Failure> {
        let (pending, receiver) = self.inner.manual.enqueue(challenge, timeout);
        // Whatever ends the wait — an answer, a timeout, or the download being paused or
        // cancelled out from under this future — the queue entry goes with it.
        let _guard = QueueGuard {
            inner: Arc::clone(&self.inner),
            id: pending.id,
        };
        self.announce();
        let outcome = match tokio::time::timeout(timeout, receiver).await {
            Ok(Ok(manual::Reply::Answer(answer))) => manual::ManualOutcome::Solved(answer),
            Ok(Ok(manual::Reply::Declined)) => manual::ManualOutcome::Skipped,
            Ok(Ok(manual::Reply::PageWithoutWidget)) => manual::ManualOutcome::PageWithoutWidget,
            // The queue entry vanished with the broker; treat it like a timeout.
            Ok(Err(_)) => manual::ManualOutcome::TimedOut,
            Err(_) => manual::ManualOutcome::TimedOut,
        };
        match outcome {
            manual::ManualOutcome::Solved(answer) => Ok(answer),
            manual::ManualOutcome::Skipped => Err(Failure::coded(
                FailureKind::NeedsCaptcha,
                "captcha.skipped",
                "The captcha was declined",
            )),
            manual::ManualOutcome::PageWithoutWidget => {
                Err(page_without_widget(pending.host.as_deref()))
            }
            // An unanswered widget means nobody solved it in a browser and no solver was
            // configured in time. That is the problem to report — not that nobody typed an
            // answer, which was never possible for a widget.
            manual::ManualOutcome::TimedOut if widget => Err(widget_needs_solver()),
            manual::ManualOutcome::TimedOut => Err(Failure::coded(
                FailureKind::NeedsCaptcha,
                "captcha.timeout",
                "Nobody solved the captcha in time",
            )),
        }
    }

    /// Tells live clients that the pending list changed. Not persisted: a captcha only
    /// exists while a download waits for it.
    fn announce(&self) {
        announce(&self.inner);
    }
}

/// Drops a queue entry once nothing waits for it any more.
struct QueueGuard {
    inner: Arc<Inner>,
    id: CaptchaId,
}

impl Drop for QueueGuard {
    fn drop(&mut self) {
        // An answered or skipped captcha is already gone; only report a change this made.
        if self.inner.manual.forget(self.id) {
            announce(&self.inner);
        }
    }
}

fn announce(inner: &Inner) {
    let pending = inner.manual.pending();
    inner.database.broadcast(EventEnvelope::new(
        EventKind::CaptchaChanged,
        serde_json::json!({ "pending": pending }),
    ));
}

fn key_missing() -> Failure {
    Failure::coded(
        FailureKind::Permanent,
        "captcha.solver_key_missing",
        "The captcha solver has no API key",
    )
}

/// Reports an unreadable key without quoting the stored value or its reference.
fn key_unreadable(error: &impl std::fmt::Display) -> Failure {
    Failure::coded(
        FailureKind::Permanent,
        "captcha.solver_key_missing",
        format!("The captcha solver API key could not be read: {error}"),
    )
}

/// Who, besides a solver service, could answer a challenge.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Answerers {
    /// A person, in the web interface: the picture is shown and the answer typed or clicked.
    Person,
    /// A browser on the hoster's own page, through the extension.
    Browser,
    /// Nobody: the widget's token is not readable by any browser rDownloader drives.
    ServiceOnly,
}

const fn answerers(challenge: &CaptchaChallenge) -> Answerers {
    match challenge {
        CaptchaChallenge::Image(_) | CaptchaChallenge::ClickPoint(_) => Answerers::Person,
        CaptchaChallenge::RecaptchaV2(_)
        | CaptchaChallenge::HCaptcha(_)
        | CaptchaChallenge::Turnstile(_) => Answerers::Browser,
        CaptchaChallenge::Cutcaptcha(_) => Answerers::ServiceOnly,
    }
}

/// Explains why a challenge cannot be answered at all, so the UI can point at the fix.
///
/// For a picture only reachable with manual solving switched off; with it on, the user is
/// shown the challenge itself. A widget is shown as a hint naming its hoster. A CutCaptcha
/// always ends here without a solver, because nothing else can answer one.
fn no_solver(challenge: &CaptchaChallenge) -> Failure {
    match answerers(challenge) {
        Answerers::Person => Failure::coded(
            FailureKind::NeedsCaptcha,
            "captcha.no_solver",
            "No captcha solver is configured",
        ),
        Answerers::Browser => widget_needs_solver(),
        Answerers::ServiceOnly => Failure::coded(
            FailureKind::NeedsCaptcha,
            "captcha.cutcaptcha_needs_solver",
            "This hoster uses CutCaptcha, which only a solver service can answer: configure one",
        ),
    }
}

/// The one sentence every widget stall ends in, so the three places that raise it agree.
/// The browser found the hoster's page without the widget the service met there.
///
/// Worded for the one cause measured so far (RD-120-45): the person's browser holds a session
/// at the hoster, so the page the service fetched as a guest is skipped over in the browser.
/// The service cannot read that session by itself; the text says what can.
pub(crate) fn page_without_widget(host: Option<&str>) -> Failure {
    let host = host.unwrap_or_default();
    Failure::coded(
        FailureKind::NeedsCaptcha,
        "captcha.page_without_widget",
        format!(
            "{host} showed no captcha in your browser, most likely because the browser is \
             already signed in there, and rDownloader cannot use that session by itself. Take \
             it over with \"Take over from browser\" at the account, or sign out of {host} in \
             the browser and test the account again, so the sign-in page shows its captcha"
        ),
    )
    .with_param("host", host)
}

fn widget_needs_solver() -> Failure {
    Failure::coded(
        FailureKind::NeedsCaptcha,
        "captcha.widget_needs_solver",
        "This hoster uses a captcha widget: answer it in your browser through the \
         rDownloader extension, in the desktop agent's window, or configure a solver service",
    )
}

#[async_trait::async_trait]
impl rd_plugin_api::CaptchaSolver for CaptchaBroker {
    async fn allowance(&self) -> Duration {
        Self::solve_allowance(self).await
    }

    async fn solve(
        &self,
        challenge: CaptchaChallenge,
        limit: Duration,
    ) -> Result<CaptchaAnswer, Failure> {
        Self::solve_within(self, challenge, limit).await
    }
}

#[cfg(test)]
#[path = "broker_tests.rs"]
mod tests;
