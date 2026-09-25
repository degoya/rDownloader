//! Signing in to an XFileSharing (XFS) installation with a stored username and password.
//!
//! The alternative this replaces is asking the user to copy a `Cookie:` header out of browser
//! devtools, which is neither obvious nor durable — the session expires and has to be pasted
//! again. JDownloader (`XFileSharingProBasic.loginWebsite`) and pyLoad (`XFSAccount.signin`)
//! both do the same thing instead: fetch the login page, resubmit its form with the account's
//! credentials, and keep the `xfss` session cookie the site sets.
//!
//! Everything here is string work on HTML and headers, target-neutral like the rest of this
//! crate: the caller performs the two requests and hands the responses back in. Two things
//! deliberately stay outside:
//!
//! * **The credentials.** The caller passes the host's `{{username}}` and `{{secret:…}}`
//!   markers into [`login_body`], and the host substitutes them on the way out. A plugin never
//!   holds the password, and cannot leak one it never had.
//! * **The session cookie.** The plugin does not read, store or resend it — it cannot; `Cookie`
//!   is not an allowed request header. The host's per-account cookie jar absorbs the
//!   `Set-Cookie` and replays it on every later request of the same account, which is why
//!   [`login_outcome`] only has to *report* whether the sign-in took.

use crate::{free, page};

/// The session cookie an XFS installation sets on a successful sign-in.
pub const SESSION_COOKIE: &str = "xfss";

/// Path of the page carrying the login form (`XFileSharingProBasic.getLoginURL`).
pub const LOGIN_PATH: &str = "/login.html";

/// Path of the account overview, which carries the API key among other things
/// (`XFileSharingProBasic.getRelativeAccountInfoURL`).
pub const ACCOUNT_INFO_PATH: &str = "/?op=my_account";

/// The `op=login` form as found on the login page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoginForm {
    /// The form's `action`, absent when it posts back to the page it came from.
    pub action: Option<String>,
    /// The form's hidden fields — `op`, plus the per-session `token` and `rand` an XFS
    /// installation rejects the submission without.
    pub hidden: Vec<(String, String)>,
}

/// Reads the `op=login` form off a login page.
///
/// Scoped by the `value="login"` marker of the form's own `op` field, the same way every other
/// XFS form in this crate is located, so a login widget rendered elsewhere on the page cannot
/// contribute fields to the submission.
#[must_use]
pub fn login_form(html: &str) -> Option<LoginForm> {
    let hidden = page::download_form(html, "login")?;
    Some(LoginForm {
        action: page::form_action(html, "login").map(str::to_owned),
        hidden,
    })
}

/// Builds the sign-in submission: the form's own hidden fields, then the credentials.
///
/// `username_template` and `secret_template` are the host's markers, not values. They are
/// appended raw, *after* the hidden fields have been percent-encoded, because a marker whose
/// braces were percent-encoded would no longer be recognised. The host encodes the substituted
/// value itself for an `application/x-www-form-urlencoded` body, so a password containing `&`
/// or `=` arrives intact and cannot add fields of its own.
#[must_use]
pub fn login_body(form: &LoginForm, username_template: &str, secret_template: &str) -> Vec<u8> {
    let mut body = page::encode_form(&form.hidden);
    if !body.is_empty() {
        body.push(b'&');
    }
    body.extend_from_slice(format!("login={username_template}").as_bytes());
    body.extend_from_slice(format!("&password={secret_template}").as_bytes());
    body
}

/// The captcha widget guarding the login form, if there is one.
///
/// Scoped to the `op=login` form rather than the whole page, the same way every other form
/// lookup in this crate is: a login page also carries the register and password-reset modals,
/// each with a widget of its own, and answering the wrong one produces a token the server
/// rejects without saying why.
#[must_use]
pub fn login_challenge(html: &str) -> Option<free::WidgetMarker> {
    free::widget_marker(page::form_html(html, "login")?)
}

/// Adds the solved token to the form's fields, replacing any placeholder already there.
#[must_use]
pub fn with_challenge_token(form: &LoginForm, kind: free::WidgetKind, token: &str) -> LoginForm {
    LoginForm {
        action: form.action.clone(),
        hidden: free::with_captcha_token(&form.hidden, kind, token),
    }
}

/// What the site made of a sign-in attempt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoginOutcome {
    /// The session cookie was set, or the answer is a page only a signed-in visitor sees.
    Authenticated,
    /// The site said the username or password is wrong. Retrying will not help.
    BadCredentials,
    /// The site refused this IP rather than these credentials. Retrying later may help.
    IpBlocked,
    /// The site rejected the captcha answer, so the credentials were never read.
    ///
    /// Distinct from [`Self::BadCredentials`] on purpose: nothing about the account is wrong,
    /// and telling the user their password was refused would send them to fix what is not
    /// broken. Carries the challenge's name for the message.
    CaptchaRejected(&'static str),
    /// Neither confirmed nor explained; carries the page's own diagnosis for the message.
    Unknown(String),
}

/// Classifies the answer to a sign-in submission.
///
/// `set_cookies` are the response's `Set-Cookie` values. The messages are XFS engine template
/// text, matched the same way JDownloader's `containsInvalidLoginsMessage` and
/// `containsBlockedIPLoginMessage` match them.
#[must_use]
pub fn login_outcome(set_cookies: &[String], html: &str) -> LoginOutcome {
    if contains_invalid_login_message(html) {
        return LoginOutcome::BadCredentials;
    }
    if contains_blocked_ip_message(html) {
        return LoginOutcome::IpBlocked;
    }
    if session_cookie_present(set_cookies) {
        return LoginOutcome::Authenticated;
    }
    // No cookie in *this* response is not yet a failure: the sign-in redirects, and the cookie
    // may have been set on the hop the caller no longer sees. The page itself settles it, and
    // only a positive marker may: the sign-out link a signed-in page offers. The guest file
    // page measured on 2026-09-17 carries `op=logout` zero times and `href="/login"` four
    // times, two of them in modals that may well ship on signed-in pages too — so the absence
    // of the login link cannot be required, and the absence of the login *form* is not enough
    // (the homepage a refused sign-in redirects to has no form either). RD-108-28 review.
    if page::shows_signed_in(html) {
        return LoginOutcome::Authenticated;
    }
    // From here on the answer is not a signed-in page: the login form again, the guest
    // homepage, or something that is neither. A rejected captcha is the one of these that has
    // its own outcome; it comes after the three above because a wrong password reported beside
    // a widget is still a wrong password.
    //
    // DDownload put Cloudflare Turnstile on its login form after RD-098-04 shipped, having
    // checked on 2026-09-06 that there was none. Until the caller started answering the
    // challenge, every sign-in ended here — and the reason that reached the user was that
    // their cookies had not been sent, a problem they did not have and could not fix.
    if free::is_wrong_captcha(html) {
        return LoginOutcome::CaptchaRejected(
            login_challenge(html).map_or("a captcha", |marker| marker.kind.display_name()),
        );
    }
    LoginOutcome::Unknown(sign_in_diagnosis(html))
}

/// Why a sign-in answer is not believed: the page's own message first, the login wall's usual
/// explanation when the form is there, the guest header when that is all the page shows, and
/// otherwise the honest "neither" — a page that is not signed in and not the login page.
fn sign_in_diagnosis(html: &str) -> String {
    if page::is_login_wall(html) || page::page_message(html).is_some() {
        return page::diagnose(html);
    }
    if page::shows_guest_header(html) {
        return "the answer still shows the site's guest header - no session was established"
            .to_owned();
    }
    "the answer is neither a signed-in page nor the login page - the sign-in was not confirmed"
        .to_owned()
}

/// Whether any `Set-Cookie` value assigns the XFS session cookie a non-empty value.
#[must_use]
pub fn session_cookie_present(set_cookies: &[String]) -> bool {
    set_cookies.iter().any(|value| {
        value
            .trim_start()
            .strip_prefix(SESSION_COOKIE)
            .and_then(|rest| rest.strip_prefix('='))
            .is_some_and(|rest| {
                let value = rest.split(';').next().unwrap_or_default().trim();
                !value.is_empty() && value != "deleted"
            })
    })
}

fn contains_invalid_login_message(html: &str) -> bool {
    [
        "Incorrect Login or Password",
        "Incorrect Username or Password",
    ]
    .iter()
    .any(|message| html.contains(message))
}

fn contains_blocked_ip_message(html: &str) -> bool {
    [
        "You can't login from this IP",
        "Your IP is banned",
        "Your IP was banned",
    ]
    .iter()
    .any(|message| html.contains(message))
}

/// Finds the account's API key on a signed-in page.
///
/// Two shapes, in JDownloader's order (`XFileSharingProBasic.regexAPIKey`): the key as it
/// appears in a ready-made API URL, else the lone read-only input the account page renders it
/// in. The second is only trusted when the page holds exactly one such field — more than one
/// and there is no telling which is the key.
#[must_use]
pub fn api_key(html: &str) -> Option<String> {
    if let Some(key) = key_after(html, "/api/account/info?key=") {
        return Some(key);
    }
    let mut found: Option<String> = None;
    let mut cursor = 0;
    while let Some(offset) = html[cursor..].find("<input") {
        let tag_start = cursor + offset;
        let Some(length) = html[tag_start..].find('>') else {
            break;
        };
        let tag = &html[tag_start..tag_start + length];
        cursor = tag_start + length;
        if !tag.contains("readonly") {
            continue;
        }
        let Some(value) = value_attribute(tag).filter(|value| is_api_key(value)) else {
            continue;
        };
        if found.is_some() {
            // Ambiguous page: refuse rather than pick one at random.
            return None;
        }
        found = Some(value.to_owned());
    }
    found
}

fn key_after(html: &str, marker: &str) -> Option<String> {
    let start = html.find(marker)? + marker.len();
    let key: String = html[start..]
        .chars()
        .take_while(|value| value.is_ascii_lowercase() || value.is_ascii_digit())
        .collect();
    is_api_key(&key).then_some(key)
}

fn value_attribute(tag: &str) -> Option<&str> {
    let start = tag.find("value=\"")? + "value=\"".len();
    let length = tag[start..].find('"')?;
    Some(&tag[start..start + length])
}

fn is_api_key(value: &str) -> bool {
    value.len() >= 16
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

#[cfg(test)]
mod tests;
