//! Tests of the broker rules that need no database: who answers what, and how a refusal reads.

use rd_plugin_api::{CaptchaChallenge, CutcaptchaChallenge, ImageChallenge, WidgetChallenge};

use super::{Answerers, answerers, no_solver, page_without_widget};

fn cutcaptcha() -> CaptchaChallenge {
    CaptchaChallenge::Cutcaptcha(CutcaptchaChallenge {
        site_key: "SAs61IAI".to_owned(),
        misery_key: "a1488b66da00bf332a1488993a5443c79047e752".to_owned(),
        page_url: "https://filecrypt.cc/Container/ABC.html".to_owned(),
    })
}

fn widget() -> CaptchaChallenge {
    CaptchaChallenge::RecaptchaV2(WidgetChallenge {
        site_key: "key".to_owned(),
        page_url: "https://rapidgator.net/file".to_owned(),
        invisible: false,
    })
}

fn image() -> CaptchaChallenge {
    CaptchaChallenge::Image(ImageChallenge {
        mime: "image/png".to_owned(),
        data: b"x".to_vec(),
        prompt: None,
    })
}

/// A widget captcha nobody can answer is a different problem from having nothing
/// configured at all, and the message must say so.
#[test]
fn the_reported_reason_distinguishes_a_widget_from_a_missing_solver() {
    assert_eq!(
        no_solver(&widget()).code.as_deref(),
        Some("captcha.widget_needs_solver")
    );
    assert_eq!(
        no_solver(&image()).code.as_deref(),
        Some("captcha.no_solver")
    );
    assert_eq!(
        no_solver(&cutcaptcha()).code.as_deref(),
        Some("captcha.cutcaptcha_needs_solver")
    );
}

/// A click-point captcha is a picture a person can answer, like an image captcha; a
/// CutCaptcha is answered by a solver service or not at all.
#[test]
fn the_new_kinds_are_offered_to_the_right_answerers() {
    let clicked = CaptchaChallenge::ClickPoint(ImageChallenge {
        mime: "image/png".to_owned(),
        data: b"x".to_vec(),
        prompt: None,
    });
    assert_eq!(answerers(&clicked), Answerers::Person);
    assert_eq!(answerers(&cutcaptcha()), Answerers::ServiceOnly);
    assert_eq!(answerers(&widget()), Answerers::Browser);
}

/// The failure a sign-in ends with when the browser saw no widget names the hoster and carries
/// its own code, so the interface can say what to do instead of "declined" (RD-120-45).
#[test]
fn a_page_without_its_widget_is_reported_with_its_own_code_and_host() {
    let failure = page_without_widget(Some("ddownload.com"));
    assert_eq!(failure.code.as_deref(), Some("captcha.page_without_widget"));
    assert_eq!(
        failure.params.get("host").map(String::as_str),
        Some("ddownload.com")
    );
    assert!(failure.message.contains("ddownload.com"));
}
