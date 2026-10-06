//! The failures the broker raises when nobody can answer a challenge, and who could.

use rd_core::{Failure, FailureKind};
use rd_plugin_api::CaptchaChallenge;

pub(super) fn key_missing() -> Failure {
    Failure::coded(
        FailureKind::Permanent,
        "captcha.solver_key_missing",
        "The captcha solver has no API key",
    )
}

/// Reports an unreadable key without quoting the stored value or its reference.
pub(super) fn key_unreadable(error: &impl std::fmt::Display) -> Failure {
    Failure::coded(
        FailureKind::Permanent,
        "captcha.solver_key_missing",
        format!("The captcha solver API key could not be read: {error}"),
    )
}

/// Who, besides a solver service, could answer a challenge.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Answerers {
    /// A person, in the web interface: the picture is shown and the answer typed or clicked.
    Person,
    /// A browser on the hoster's own page, through the extension.
    Browser,
    /// Nobody: the widget's token is not readable by any browser rDownloader drives.
    ServiceOnly,
}

pub(super) const fn answerers(challenge: &CaptchaChallenge) -> Answerers {
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
pub(super) fn no_solver(challenge: &CaptchaChallenge) -> Failure {
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

pub(super) fn widget_needs_solver() -> Failure {
    Failure::coded(
        FailureKind::NeedsCaptcha,
        "captcha.widget_needs_solver",
        "This hoster uses a captcha widget: answer it in your browser through the \
         rDownloader extension, in the desktop agent's window, or configure a solver service",
    )
}
