//! Tests of the shared XFS page helpers, split out of `page.rs` to keep it under the line
//! limit. Fixtures that quote a real page say which page and which day.

use super::{
    SessionState, SessionVerdict, classify_session, diagnose, direct_link, download_form,
    encode_form, form_html, has_captcha_challenge, is_login_wall, premium_form, session_verdict,
    shows_guest_header, shows_signed_in, without_style_script_and_comments,
};

const PAGE: &str = r#"<html><body>
<form name="F1" method="POST" action="" style="display:contents;">
  <input type="hidden" name="op" value="download2">
  <input type="hidden" name="id" value="z31889n8peey">
  <input type="hidden" name="rand" value="2wde33y7krah">
  <input type="hidden" name="referer" value="">
  <input type="hidden" name="method_free" value="">
  <input type="hidden" name="method_premium" value="">
  <input type="email" class="rm-input" id="rm-email" name="email">
</form></body></html>"#;

#[test]
fn extracts_hidden_fields_of_the_download_form() {
    let fields = download_form(PAGE, "download2").expect("form");
    assert_eq!(
        fields,
        [
            ("op", "download2"),
            ("id", "z31889n8peey"),
            ("rand", "2wde33y7krah"),
            ("referer", ""),
            ("method_free", ""),
            ("method_premium", ""),
        ]
        .map(|(name, value)| (name.to_owned(), value.to_owned()))
    );
    assert_eq!(
        encode_form(&fields),
        b"op=download2&id=z31889n8peey&rand=2wde33y7krah&referer=&method_free=&method_premium="
    );
    assert!(download_form("<html><form><input name='x'></form></html>", "download2").is_none());
}

#[test]
fn form_html_scopes_to_the_form_carrying_the_op_marker() {
    let outside = format!(r#"<div class="g-recaptcha" data-sitekey="unrelated"></div>{PAGE}"#);
    let scoped = form_html(&outside, "download2").expect("form html");
    assert!(scoped.starts_with("<form"));
    assert!(scoped.contains("op") && scoped.contains("download2"));
    assert!(
        !scoped.contains("g-recaptcha"),
        "form_html must not include markup preceding the <form> tag"
    );
    assert!(form_html("<html><p>no form here</p></html>", "download2").is_none());
}

/// A site that delivers from its own CDN puts the link on a different host, sometimes
/// with a port. Missing it made an otherwise complete free download report "no link".
#[test]
fn a_delivery_cdn_link_is_found_including_its_port() {
    let html = r#"<a href="https://ddownload.com/premium">Buy</a>
<a href="https://eu-hydra5.zeuscdn.org:183/d/tok3n/release.rar">Download</a>"#;

    assert_eq!(
        super::direct_link_any(html, &["release.rar"], &["ddownload.com", "*.zeuscdn.org"])
            .as_deref(),
        Some("https://eu-hydra5.zeuscdn.org:183/d/tok3n/release.rar")
    );
    // Without the CDN in the list the link is correctly not claimed.
    assert_eq!(
        super::direct_link_any(html, &["release.rar"], &["ddownload.com"]),
        None
    );
    // A look-alike host must not pass as a subdomain.
    let lookalike = r#"<a href="https://notddownload.com/d/x/release.rar">x</a>"#;
    assert_eq!(
        super::direct_link_any(lookalike, &["release.rar"], &["ddownload.com"]),
        None
    );
}

#[test]
fn prefers_the_link_matching_the_file_name() {
    let html = r#"<a href="https://ddownload.com/premium">Buy</a>
<a href="https://fs12.ddownload.com/d/abc123/release.rar?x=1&amp;y=2" id="direct">Download</a>"#;
    assert_eq!(
        direct_link(html, &["release.rar"], "ddownload.com").as_deref(),
        Some("https://fs12.ddownload.com/d/abc123/release.rar?x=1&y=2")
    );
    assert_eq!(
        direct_link(html, &["other.rar"], "ddownload.com").as_deref(),
        Some("https://fs12.ddownload.com/d/abc123/release.rar?x=1&y=2")
    );
    assert!(
        direct_link(
            "<a href=\"https://example.com/d/x\">",
            &["x"],
            "ddownload.com"
        )
        .is_none()
    );
}

#[test]
fn premium_form_drops_free_marker_and_sets_premium_button() {
    let form = download_form(PAGE, "download2").expect("form");
    let premium = premium_form(&form, "Premium Download");
    assert!(premium.iter().all(|(name, _)| name != "method_free"));
    assert_eq!(
        premium.iter().find(|(name, _)| name == "method_premium"),
        Some(&("method_premium".to_owned(), "Premium Download".to_owned()))
    );
    let without = vec![("op".to_owned(), "download2".to_owned())];
    assert_eq!(premium_form(&without, "Premium Download").len(), 2);
}

/// The header's login link, verbatim from DDownload's file page of 2026-09-17, where it
/// appears four times on a page that is not a login page at all (RD-108-28).
const NAVIGATION_LOGIN_LINK: &str = r#"<nav class="header-nav">
<a class="nav-link outlined" href="/login" onclick="if(typeof LoginModal!=='undefined'){return LoginModal.show()}">
Login
</a></nav>
<div class="rm-login-link">Already have an account? <a href="/login" onclick="event.preventDefault(); LoginModal.show();">Login</a></div>"#;

const LOGIN_FORM: &str = r#"<form method="POST" action="https://ddownload.com/" name="FL">
<input type="hidden" name="op" value="login">
<input type="text" name="login" value="">
</form>"#;

#[test]
fn is_login_wall_needs_the_login_form_and_not_the_navigation_link() {
    assert!(is_login_wall(LOGIN_FORM));
    assert!(
        !is_login_wall(NAVIGATION_LOGIN_LINK),
        "the header link is on every page a guest sees"
    );
    // The LoginModal script ships on every page, logged in or not — it alone must not match.
    assert!(!is_login_wall(
        "<title>Pricing - DDownload</title><script>var LoginModal = {}</script>"
    ));
    // A form whose `op` is something else does not become a login form by mentioning login.
    assert!(!is_login_wall(
        "<form><input type=\"hidden\" name=\"op\" value=\"download2\"><input name=\"login\" value=\"login\"></form>"
    ));
}

/// The sign-out link is the positive signal a session check believes; the guest page of
/// 2026-09-17 carries none, and a signed-in page carries the login link in its modals for
/// all anybody knows, so only the sign-out link may decide.
#[test]
fn only_the_sign_out_link_says_signed_in() {
    assert!(shows_signed_in(
        "<a href=\"/?op=logout\">Log out</a><div class=\"rm-login-link\"><a href=\"/login\">Login</a></div>"
    ));
    assert!(!shows_signed_in(NAVIGATION_LOGIN_LINK));
    assert!(!shows_signed_in(LOGIN_FORM));
    assert!(!shows_signed_in("<title>Maintenance</title>"));
    // The marker is a word, not a substring: this page carries `dialog` sixteen times, and a
    // rule that took any occurrence would have read `dialog outside` as a session.
    assert!(!shows_signed_in("<div>the dialog outside the form</div>"));
    assert!(!shows_signed_in("<p>catalog output</p>"));
    assert!(shows_signed_in("<a href=\"/?op=logout\">x</a>"));
    // The marker at either end of the text: both boundary checks have nothing to look at.
    assert!(shows_signed_in("logout"));
    assert!(shows_signed_in("log out"));
    // An identifier separator is not a letter, so a class or a handler counts as the word. A
    // guest page does not carry these; a signed-in one is exactly what does.
    assert!(shows_signed_in("<div class=\"dialog-logout\"></div>"));
    assert!(shows_signed_in(
        "<button onclick=\"user_logout()\">x</button>"
    ));
    // The signed-in page's spelling is unmeasured, so every likely one counts ...
    for signed_in in [
        r#"<a href="/logout">Logout</a>"#,
        r#"<a href="/?op=LOGOUT">Sign out</a>"#,
        r#"<button class="nav-link">Log out</button>"#,
        r##"<a href="/account">Dashboard</a> | <a href="#" onclick="logOut()">Log-out</a>"##,
    ] {
        assert!(shows_signed_in(signed_in), "{signed_in}");
    }
    // ... and the one occurrence on the measured guest page does not: a stylesheet comment,
    // line 390 of the page of 2026-09-17, the only `logout` in 209 KB, zero once stripped.
    let guest_with_comment = format!(
        "<style>\n/* Narrow dropdown for Dashboard/Logout */\n.dd {{ width: 8rem }}\n</style>\n{NAVIGATION_LOGIN_LINK}"
    );
    assert!(!shows_signed_in(&guest_with_comment));
    assert!(!shows_signed_in(
        "<SCRIPT>var label = 'Log out';</SCRIPT><a href=\"/login\">Login</a>"
    ));
}

/// The stripper matches its tags in any case and survives a block that never closes.
#[test]
fn style_script_and_comment_blocks_are_stripped_whatever_their_case() {
    let page =
        "<STYLE>a { }</STYLE><p>kept</p><Script src=\"x.js\"></Script><b>also</b><style>open";
    assert_eq!(
        without_style_script_and_comments(page),
        "<p>kept</p><b>also</b>"
    );
    // A comment narrates what the markup beside it would mean, and the captured page really
    // does carry one that names the auth state. It must not decide a session.
    assert_eq!(
        without_style_script_and_comments("<p>a</p><!-- Logout dropdown --><p>b</p>"),
        "<p>a</p><p>b</p>"
    );
    assert!(!shows_signed_in("<p>a</p><!-- Logout dropdown --><p>b</p>"));
    assert_eq!(
        without_style_script_and_comments("<p>a</p><!-- open"),
        "<p>a</p>"
    );
}

/// The header link explains a page that is not signed in; it decides nothing on its own.
#[test]
fn the_guest_header_is_seen_on_guest_pages_and_not_on_signed_in_ones() {
    assert!(shows_guest_header(NAVIGATION_LOGIN_LINK));
    // The login form alone offers no link; the two markers are independent on purpose.
    assert!(!shows_guest_header(LOGIN_FORM));
    assert!(!shows_guest_header(
        "<a href=\"/?op=logout\">Log out</a><script>var LoginModal = {}</script>"
    ));
}

#[test]
fn diagnose_explains_login_pages_errors_and_titles() {
    assert!(diagnose(LOGIN_FORM).contains("login"));
    // A file page that lost its download form still carries the header's login link; the
    // honest answer names the page, not a login the site never asked for.
    assert_eq!(
        diagnose(&format!(
            "<title>Download release.rar</title>{NAVIGATION_LOGIN_LINK}"
        )),
        "page \"Download release.rar\" contains no premium link"
    );
    // Every XFS page ships the login modal script, even for logged-in users.
    assert!(
        !diagnose("<title>Pricing - DDownload</title><script>var LoginModal = {}</script>")
            .contains("login")
    );
    assert_eq!(
        diagnose("<div class=\"err\"><b>File</b> not  found</div>"),
        "page message: File not found"
    );
    assert_eq!(
        diagnose("<title>Please wait</title>"),
        "page \"Please wait\" contains no premium link"
    );
}

#[test]
fn detects_common_xfs_captcha_markers() {
    assert!(has_captcha_challenge(
        r#"<div class="g-recaptcha" data-sitekey="abc123"></div>"#
    ));
    assert!(has_captcha_challenge(
        r#"<div class="h-captcha" data-sitekey="x"></div>"#
    ));
    assert!(has_captcha_challenge(
        r#"<div class="cf-turnstile" data-sitekey="x"></div>"#
    ));
    assert!(!has_captcha_challenge(
        "<form><input name=\"op\" value=\"download2\"></form>"
    ));
}

#[test]
fn a_page_that_says_nothing_about_the_session_is_unknown_not_a_guest() {
    // The three answers the caller acts on differently. The first fix of RD-108-28 had only
    // two, and read every unrecognized page as "not signed in" — which a caller then turned
    // into "your account is invalid", on the very premium account the bug report said worked.
    assert_eq!(
        session_verdict("<a href=\"/?op=logout\">Log out</a>"),
        SessionVerdict::SignedIn
    );
    assert_eq!(session_verdict(LOGIN_FORM), SessionVerdict::Guest);
    assert_eq!(
        session_verdict(NAVIGATION_LOGIN_LINK),
        SessionVerdict::Guest
    );

    // Neither marker: the pages a hoster really serves instead of an answer. None of these
    // is evidence about the account.
    for neither in [
        "<title>Just a moment...</title><div id=\"cf-wrapper\"></div>",
        "<title>Maintenance</title><p>We are back shortly.</p>",
        "<html><body><div id=\"app\"></div></body></html>",
        "",
    ] {
        assert_eq!(
            session_verdict(neither),
            SessionVerdict::Unknown,
            "{neither}"
        );
    }
}

#[test]
fn the_sign_out_link_outranks_a_login_modal_that_ships_on_every_page() {
    // Both markers at once is the shape a signed-in XFS page is expected to have: the header
    // offers sign-out, and the register/password-reset modals still carry `href="/login"`.
    // The positive marker has to win, or a real session reads as a guest.
    let signed_in_with_modal =
        format!("<a href=\"/?op=logout\">Log out</a>{NAVIGATION_LOGIN_LINK}");
    assert!(shows_signed_in(&signed_in_with_modal));
    assert!(shows_guest_header(&signed_in_with_modal));
    assert_eq!(
        session_verdict(&signed_in_with_modal),
        SessionVerdict::SignedIn
    );
}

#[test]
fn a_stylesheet_or_comment_mentioning_logout_does_not_make_a_session() {
    // The stripping rule and the verdict, together: the capture of 2026-09-17 carries
    // `/* Narrow dropdown for Dashboard/Logout */` and no session. A guest page that says
    // the word must still come out Guest when it has the guest header, never SignedIn.
    let decoy = format!(
        "<style>/* Narrow dropdown for Dashboard/Logout */</style>\
         <!-- Logout dropdown -->{NAVIGATION_LOGIN_LINK}"
    );
    assert!(!shows_signed_in(&decoy));
    assert_eq!(session_verdict(&decoy), SessionVerdict::Guest);

    // The same decoy without the guest header settles nothing rather than condemning.
    let bare_decoy = "<style>/* Dashboard/Logout */</style><title>Maintenance</title>";
    assert!(!shows_signed_in(bare_decoy));
    assert_eq!(session_verdict(bare_decoy), SessionVerdict::Unknown);
}

/// The shape all three XFS account checks consume: the verdict, and — for the two answers that
/// have to explain themselves — the diagnosis of the very page that produced it.
#[test]
fn a_session_probe_is_classified_with_the_diagnosis_of_the_page_it_judged() {
    assert_eq!(
        classify_session("<a href=\"/?op=logout\">Log out</a>"),
        SessionState::Active
    );

    let guest = format!("<title>DDownload</title>{NAVIGATION_LOGIN_LINK}");
    let SessionState::Expired(diagnosis) = classify_session(&guest) else {
        panic!("a guest page is an expired session");
    };
    assert_eq!(diagnosis, diagnose(&guest));

    let interstitial = "<title>Just a moment...</title><div id=\"cf-wrapper\"></div>";
    let SessionState::Unconfirmed(diagnosis) = classify_session(interstitial) else {
        panic!("an interstitial settles nothing");
    };
    assert_eq!(diagnosis, diagnose(interstitial));
}
