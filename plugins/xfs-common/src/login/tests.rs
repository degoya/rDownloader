//! Fixtures are trimmed from the real pages (`https://ddownload.com/login.html`, fetched
//! 2026-09-06) — the surrounding theme markup is dropped, every field and message that the
//! parsing depends on is kept verbatim.

use super::*;

const LOGIN_PAGE: &str = r#"<!DOCTYPE html><html><body>
<div class="card">
<form method="POST" action="https://ddownload.com/" name="FL">
<input type="hidden" name="op" value="login">
<input type="hidden" name="token" value="09509f485f267c36c2b54fa96330c2b3">
<input type="hidden" name="rand" value="5ek5lfox7asr35ttysyf7tlm7vufvwofcdh6xbpz5a">
<input type="hidden" name="redirect" value="">
<input type="text" name="login" value="" class="form-control" placeholder="Enter your username or email" required>
<input type="password" name="password" class="form-control" placeholder="Enter your password" required>
<button type="submit">Log in</button>
</form>
</div></body></html>"#;

fn form() -> LoginForm {
    login_form(LOGIN_PAGE).expect("login form")
}

#[test]
fn the_login_form_yields_its_action_and_its_per_session_hidden_fields() {
    let form = form();
    assert_eq!(form.action.as_deref(), Some("https://ddownload.com/"));
    assert_eq!(
        form.hidden,
        vec![
            ("op".to_owned(), "login".to_owned()),
            (
                "token".to_owned(),
                "09509f485f267c36c2b54fa96330c2b3".to_owned()
            ),
            (
                "rand".to_owned(),
                "5ek5lfox7asr35ttysyf7tlm7vufvwofcdh6xbpz5a".to_owned()
            ),
            ("redirect".to_owned(), String::new()),
        ]
    );
}

/// The visible credential inputs are not hidden fields and must not be picked up as such —
/// [`login_body`] appends them itself, as markers.
#[test]
fn the_visible_credential_inputs_are_not_treated_as_hidden_fields() {
    let form = form();
    assert!(
        !form
            .hidden
            .iter()
            .any(|(name, _)| name == "login" || name == "password")
    );
}

#[test]
fn the_body_carries_the_hidden_fields_and_then_the_credential_markers() {
    let body = login_body(&form(), "{{username}}", "{{secret:ddownload_password}}");
    assert_eq!(
        String::from_utf8(body).expect("utf-8"),
        "op=login&token=09509f485f267c36c2b54fa96330c2b3\
         &rand=5ek5lfox7asr35ttysyf7tlm7vufvwofcdh6xbpz5a&redirect=\
         &login={{username}}&password={{secret:ddownload_password}}"
    );
}

/// Percent-encoding the markers would stop the host from recognising them, so they have to be
/// appended after the hidden fields are encoded, not passed through the encoder.
#[test]
fn the_credential_markers_survive_the_form_encoding_intact() {
    let body = login_body(&form(), "{{username}}", "{{secret:ddownload_password}}");
    let body = String::from_utf8(body).expect("utf-8");
    assert!(body.contains("login={{username}}"));
    assert!(!body.contains("%7B%7B"));
}

// -- outcomes ---------------------------------------------------------

#[test]
fn the_session_cookie_confirms_a_sign_in() {
    let cookies = vec!["xfss=abc123; Path=/; HttpOnly".to_owned()];
    assert_eq!(
        login_outcome(&cookies, LOGIN_PAGE),
        LoginOutcome::Authenticated
    );
}

/// XFS clears the session by setting the same cookie to an empty or `deleted` value; that is a
/// sign-out, not a sign-in.
#[test]
fn a_cleared_session_cookie_does_not_count_as_a_sign_in() {
    for value in ["xfss=; Path=/", "xfss=deleted; Max-Age=0"] {
        assert!(!session_cookie_present(&[value.to_owned()]));
    }
    assert!(!session_cookie_present(&["srv=c868; Path=/".to_owned()]));
}

/// The sign-in redirects, so the response the caller sees may carry no `Set-Cookie` at all. A
/// page offering the sign-out link is then the evidence — even one whose modals still offer
/// the login link, which nobody has ruled out for a signed-in page.
#[test]
fn the_sign_out_link_counts_as_a_sign_in_without_a_cookie() {
    let dashboard = r#"<html><body><a href="/?op=logout">Log out</a>
<div class="rm-login-link">Already have an account? <a href="/login">Login</a></div></body></html>"#;
    assert_eq!(login_outcome(&[], dashboard), LoginOutcome::Authenticated);
}

/// A page with neither marker — a maintenance page, an interstitial — is not a sign-in either,
/// and says so rather than blaming the cookies.
#[test]
fn a_page_with_neither_marker_is_not_a_sign_in() {
    match login_outcome(
        &[],
        "<html><head><title>Maintenance</title></head><body>Back soon</body></html>",
    ) {
        LoginOutcome::Unknown(reason) => assert!(reason.contains("neither"), "{reason}"),
        other => panic!("expected an unknown outcome, got {other:?}"),
    }
}

/// The other direction: a page that is not the login form is not yet a signed-in page. The
/// homepage a refused sign-in redirects to carries no `op=login` form, but it still shows the
/// guest header, and believing it would report a green account with no session behind it.
#[test]
fn a_guest_page_without_the_login_form_is_not_a_sign_in() {
    let guest_home = r#"<html><head><title>DDownload</title></head><body>
<a class="nav-link outlined" href="/login">Login</a>
<a class="nav-link primary" href="/register">Sign Up</a></body></html>"#;
    match login_outcome(&[], guest_home) {
        LoginOutcome::Unknown(reason) => {
            assert!(reason.contains("guest header"), "{reason}");
        }
        other => panic!("a guest page must not count as a sign-in, got {other:?}"),
    }
    // The page's own message still wins when there is one.
    let with_message = format!("<div class=\"err\">Too many attempts</div>{guest_home}");
    assert_eq!(
        login_outcome(&[], &with_message),
        LoginOutcome::Unknown("page message: Too many attempts".to_owned())
    );
}

#[test]
fn wrong_credentials_are_reported_as_such_rather_than_as_a_generic_failure() {
    let page = format!("<div class=\"err\">Incorrect Login or Password</div>{LOGIN_PAGE}");
    assert_eq!(login_outcome(&[], &page), LoginOutcome::BadCredentials);
}

/// A blocked IP is the site refusing this network, not these credentials — the caller must not
/// invalidate the account over it.
#[test]
fn a_blocked_ip_is_distinguished_from_wrong_credentials() {
    for message in [
        "You can't login from this IP",
        "Your IP is banned",
        "Your IP was banned",
    ] {
        let page = format!("<div class=\"err\">{message}</div>{LOGIN_PAGE}");
        assert_eq!(login_outcome(&[], &page), LoginOutcome::IpBlocked);
    }
}

#[test]
fn an_unexplained_login_wall_carries_the_pages_own_diagnosis() {
    match login_outcome(&[], LOGIN_PAGE) {
        LoginOutcome::Unknown(reason) => assert!(reason.contains("login")),
        other => panic!("expected an unknown outcome, got {other:?}"),
    }
}

// -- api key ----------------------------------------------------------

#[test]
fn the_api_key_is_read_from_a_ready_made_api_url() {
    let page =
        r#"<code>https://api-v2.ddownload.com/api/account/info?key=abcdef0123456789ab</code>"#;
    assert_eq!(api_key(page).as_deref(), Some("abcdef0123456789ab"));
}

#[test]
fn the_api_key_is_read_from_the_lone_read_only_field() {
    let page = r#"<input type="text" class="form-control" value="0123456789abcdef01" readonly onfocus="this.select();">"#;
    assert_eq!(api_key(page).as_deref(), Some("0123456789abcdef01"));
}

/// Two read-only fields give no way to tell which one is the key, and guessing would send the
/// wrong value to the API as a credential.
#[test]
fn two_read_only_fields_are_refused_rather_than_guessed() {
    let page = concat!(
        r#"<input value="0123456789abcdef01" readonly>"#,
        r#"<input value="fedcba9876543210ff" readonly>"#
    );
    assert_eq!(api_key(page), None);
}

#[test]
fn a_short_or_non_key_value_is_not_mistaken_for_a_key() {
    assert_eq!(api_key(r#"<input value="tooshort" readonly>"#), None);
    assert_eq!(
        api_key(r#"<input value="Not-A-Key-At-All00" readonly>"#),
        None
    );
    assert_eq!(api_key("<p>no key here</p>"), None);
}

/// DDownload's login form as it stands since Cloudflare Turnstile was added to it.
///
/// Reduced to the parts that decide the flow — the `op=login` form with its hidden fields and
/// the widget inside it — plus the register modal's own widget, which must not be the one the
/// sign-in answers. Captured from the live page on 2026-09-07, when every sign-in was failing.
const LOGIN_PAGE_WITH_TURNSTILE: &str = r#"<html><body>
<div id="rm-ts"><div class="cf-turnstile" data-sitekey="0xREGISTERMODALKEY"></div></div>
<form method="POST" action="https://ddownload.com/" name="FL">
  <input type="hidden" name="op" value="login">
  <input type="hidden" name="token" value="9096bc7781ed51a106c31e3f6e5300b1">
  <input type="hidden" name="rand" value="mttmi7hiltosurmttipt6epolj7hdh5pf5a4d7xp">
  <input type="hidden" name="redirect" value="">
  <div class="cf-turnstile" data-sitekey="0x4AAAAAABm53D0OJNkESa1O" id="cf-turnstile-widget"></div>
  <input type="text" name="login" value="">
  <input type="password" name="password" value="">
</form></body></html>"#;

/// What the site answers when the form is posted without a token: the credentials are never
/// read, and the page comes back as a login wall carrying its own explanation.
const WRONG_CAPTCHA_PAGE: &str = r#"<html><body>
<div class="alert alert-danger" id="loginAlert"> <span id="alertMsg">Wrong captcha</span> </div>
<form method="POST" action="https://ddownload.com/" name="FL">
  <input type="hidden" name="op" value="login">
</form></body></html>"#;

#[test]
fn the_login_forms_own_widget_is_the_one_that_gets_answered() {
    let marker = super::login_challenge(LOGIN_PAGE_WITH_TURNSTILE).expect("a challenge");
    assert_eq!(marker.kind, crate::free::WidgetKind::Turnstile);
    // Not the register modal's key, which sits earlier in the document.
    assert_eq!(marker.site_key, "0x4AAAAAABm53D0OJNkESa1O");
}

#[test]
fn a_form_without_a_widget_asks_for_no_captcha() {
    let plain = LOGIN_PAGE_WITH_TURNSTILE.replace("cf-turnstile", "not-a-widget");
    assert!(super::login_challenge(&plain).is_none());
}

#[test]
fn the_solved_token_travels_in_the_field_the_service_expects() {
    let form = super::login_form(LOGIN_PAGE_WITH_TURNSTILE).expect("form");
    let marker = super::login_challenge(LOGIN_PAGE_WITH_TURNSTILE).expect("challenge");
    let answered = super::with_challenge_token(&form, marker.kind, "TOKEN-VALUE");
    assert!(
        answered
            .hidden
            .iter()
            .any(|(name, value)| name == "cf-turnstile-response" && value == "TOKEN-VALUE")
    );
    // The form's own fields survive: without `token` and `rand` the submission is refused.
    for field in ["op", "token", "rand", "redirect"] {
        assert!(
            answered.hidden.iter().any(|(name, _)| name == field),
            "{field} was dropped"
        );
    }
    let body = String::from_utf8(super::login_body(&answered, "{{username}}", "{{secret:x}}"))
        .expect("utf-8");
    assert!(body.contains("cf-turnstile-response=TOKEN-VALUE"));
    assert!(body.ends_with("&login={{username}}&password={{secret:x}}"));
}

/// The reported failure, and why it read as a cookie problem.
///
/// The answer to a captcha-less submission is a login wall, so the generic branch claimed the
/// cookies had not been sent — advice about something that was never wrong, for a page that
/// said "Wrong captcha" in plain sight.
#[test]
fn a_refused_captcha_is_not_reported_as_a_cookie_problem() {
    match super::login_outcome(&[], WRONG_CAPTCHA_PAGE) {
        LoginOutcome::CaptchaRejected(challenge) => assert_eq!(challenge, "a captcha"),
        other => panic!("expected a captcha refusal, got {other:?}"),
    }
    assert!(!crate::page::diagnose(WRONG_CAPTCHA_PAGE).contains("cookies"));
    assert!(crate::page::diagnose(WRONG_CAPTCHA_PAGE).contains("Wrong captcha"));
}

/// Wrong credentials stay wrong credentials, widget or not.
#[test]
fn a_refused_password_is_still_reported_as_a_refused_password() {
    let page = WRONG_CAPTCHA_PAGE.replace("Wrong captcha", "Incorrect Login or Password");
    assert_eq!(
        super::login_outcome(&[], &page),
        LoginOutcome::BadCredentials
    );
}
