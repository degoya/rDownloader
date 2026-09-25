//! Unit coverage for the host-free page helpers: the content URL, the download form, every error
//! marker and the direct link.

use super::*;

/// A trimmed-down version of the real free download page: a first form with the hidden fields the
/// site carries (`adz` among them) plus the `save` checkbox JD drops.
const FORM_PAGE: &str = r#"<html><head><title>1fichier.com: Download release.rar</title></head><body>
<form method="POST" action="./?abc12defg3" id="f1">
<input type="hidden" name="adz" value="1a2b3c">
<input type="hidden" name="did" value="0">
<input type="checkbox" name="save" value="1">
<input type="submit" value="Download">
</form>
</body></html>"#;

/// The page a password-protected file shows: the same form, plus a `pass` field.
const PASSWORD_PAGE: &str = r#"<html><body>
<form method="POST" action="">
<input type="hidden" name="adz" value="1a2b3c">
<input type="password" name="pass" value="">
</form>
</body></html>"#;

/// The answer to the posted form: the download button.
const LINK_PAGE: &str = r#"<html><body><div style="vertical-align:middle">
<a href="https://a-7.1fichier.com/p123456789" class="ok btn-general btn-orange">Click here to download</a>
</div></body></html>"#;

fn url(value: &str) -> Url {
    value.parse().expect("URL")
}

#[test]
fn the_content_url_keeps_the_uploader_domain_and_forces_english() {
    assert_eq!(
        content_url(&url("https://alterupload.com/?abc12defg3")).as_deref(),
        Some("https://alterupload.com/?abc12defg3&lg=en")
    );
    assert_eq!(content_url(&url("https://example.test/?abc12defg3")), None);
}

#[test]
fn the_download_form_is_the_pages_first_form_with_its_hidden_fields() {
    let form = download_form(FORM_PAGE).expect("form");
    assert_eq!(form.action.as_deref(), Some("./?abc12defg3"));
    assert!(
        form.fields
            .iter()
            .any(|(name, value)| name == "adz" && value == "1a2b3c"),
        "{:?}",
        form.fields
    );
}

/// JD removes `save` and puts `did=1`; every other field the page carried is submitted unchanged.
#[test]
fn the_free_submission_drops_save_and_sets_did() {
    let submitted = free_form(&download_form(FORM_PAGE).expect("form"));
    assert!(
        submitted
            .iter()
            .any(|(name, value)| name == "adz" && value == "1a2b3c")
    );
    assert!(
        submitted
            .iter()
            .any(|(name, value)| name == "did" && value == "1"),
        "{submitted:?}"
    );
    assert!(!submitted.iter().any(|(name, _)| name == "save"));
    // The page's own `did=0` must not survive alongside the `did=1` JD sends.
    assert_eq!(
        submitted.iter().filter(|(name, _)| name == "did").count(),
        1
    );
}

#[test]
fn the_encoded_form_is_url_encoded() {
    let encoded = encode_form(&[("adz".to_owned(), "1a 2b".to_owned())]);
    assert_eq!(String::from_utf8_lossy(&encoded), "adz=1a+2b");
}

#[test]
fn a_password_protected_form_is_recognized() {
    assert!(is_password_protected(
        &download_form(PASSWORD_PAGE).expect("form")
    ));
    assert!(!is_password_protected(
        &download_form(FORM_PAGE).expect("form")
    ));
}

#[test]
fn a_clean_page_reports_no_error() {
    assert_eq!(page_error(FORM_PAGE), None);
    assert_eq!(page_error(LINK_PAGE), None);
}

#[test]
fn an_offline_page_is_recognized_only_as_element_text() {
    assert_eq!(
        page_error("<div class=\"ct_warn\">File not found !</div>"),
        Some(PageError::Offline)
    );
    // A stray mention in a script must not turn a working page offline.
    assert_eq!(
        page_error("<script>var msg = 'File not found';</script>"),
        None
    );
}

#[test]
fn a_stated_wait_is_parsed_in_minutes() {
    assert_eq!(
        page_error(
            "<div>Warning ! Without premium status, you must wait up to 12 minutes between each downloads</div>"
        ),
        Some(PageError::IpBlocked(12 * 60))
    );
    assert_eq!(
        page_error("<span>You must wait 3 minutes before downloading again</span>"),
        Some(PageError::IpBlocked(180))
    );
}

/// A limit stated without a duration keeps JD's five-minute default; "IP Locked" is an hour.
#[test]
fn a_limit_without_a_duration_uses_the_default_cooldown() {
    assert_eq!(
        page_error("<div>Without Premium, you can only download one file at a time</div>"),
        Some(PageError::IpBlocked(300))
    );
    assert_eq!(
        page_error("<div>IP Locked<br/>Will be unlocked within 1h.</div>"),
        Some(PageError::IpBlocked(3600))
    );
}

#[test]
fn the_no_free_slots_notice_is_its_own_case() {
    assert_eq!(
        page_error("<div>Free download is temporarily limited due to high demand</div>"),
        Some(PageError::NoFreeSlots)
    );
}

#[test]
fn an_account_only_file_is_recognized() {
    assert_eq!(
        page_error("<div>It is not possible to free unregistered users</div>"),
        Some(PageError::AccountRequired)
    );
    assert_eq!(
        page_error(
            "<div>The owner of this file has reserved access to the subscribers of our services</div>"
        ),
        Some(PageError::AccountRequired)
    );
}

#[test]
fn server_side_problems_are_transient() {
    assert_eq!(
        page_error("<h1>Software error:</h1>"),
        Some(PageError::ServerError(600))
    );
    assert_eq!(
        page_error("<div>Your requests are too fast</div>"),
        Some(PageError::TooFast)
    );
}

#[test]
fn the_direct_link_comes_from_the_download_button() {
    assert_eq!(
        direct_link(LINK_PAGE).as_deref(),
        Some("https://a-7.1fichier.com/p123456789")
    );
}

/// Without the button's wording, an absolute link on one of the hoster's content domains is the
/// fallback JD accepts — and an unrelated link must not be mistaken for one.
#[test]
fn the_direct_link_falls_back_to_a_content_domain_link() {
    assert_eq!(
        direct_link(
            r#"<a href="/register.pl">Sign up</a><a href="https://a-3.desfichiers.com/p42">Go</a>"#
        )
        .as_deref(),
        Some("https://a-3.desfichiers.com/p42")
    );
    assert_eq!(
        direct_link(r#"<a href="https://example.test/p42">Go</a>"#),
        None
    );
}

#[test]
fn the_diagnosis_quotes_the_pages_own_wording() {
    assert_eq!(
        diagnose(FORM_PAGE),
        "1fichier.com: Download release.rar",
        "the title is the most useful thing an unexpected page carries"
    );
    assert_eq!(
        diagnose("<body><h1>Access to this file is protected</h1></body>"),
        "Access to this file is protected"
    );
    assert_eq!(diagnose("<body></body>"), "unexpected 1fichier page");
}

/// RD-109-36: the page head of every 1fichier page carries `<link rel="icon">` and
/// `<link rel="stylesheet">` pointing at `img.1fichier.com` — a content domain by the fallback's
/// own test. Taking one of those as the direct link is how a 1150-byte favicon was downloaded and
/// booked as the finished 405 MB release. Only an anchor may be the download link.
#[test]
fn the_head_assets_are_not_mistaken_for_the_direct_link() {
    const ASSET_HEAD: &str = r#"<html><head>
<link rel="icon" href="https://img.1fichier.com/favicon.ico" />
<link rel="stylesheet" href="https://img.1fichier.com/css/style.css" />
</head><body>
<div>High demand: all free guest slots are currently in use.</div>
<a href="/login.pl">Sign in and download now</a>
</body></html>"#;
    assert_eq!(
        direct_link(ASSET_HEAD),
        None,
        "a stylesheet or favicon is not the payload"
    );
}

/// RD-109-36: the same page states the file's name and size; the free flow had been taking the
/// name from the transfer's `Content-Disposition` alone, which an error page does not carry.
#[test]
fn the_link_page_states_the_file_name_and_size() {
    const TIER: &str = r#"<div class="tiers"><div class="tier">
<div class="tier-body">
<span class="tier-name">outlander.s08e01.german.bdrip.x264-intention.rar</span>
<span class="tier-feat">405.44 MB</span>
</div></div></div>"#;
    let info = stated_file(TIER).expect("the page states both");
    assert_eq!(
        info.name,
        "outlander.s08e01.german.bdrip.x264-intention.rar"
    );
    assert_eq!(
        info.size, 425_134_653,
        "405.44 MiB, the unit 1fichier prints as MB"
    );
}

/// RD-109-36: 1fichier's out-of-slots wording moved on; the old marker no longer matches, so the
/// page fell through to the direct-link fallback instead of holding the hoster back.
#[test]
fn the_current_no_free_slots_wording_is_recognized() {
    assert_eq!(
        page_error("<div>High demand: all free guest slots are currently in use.</div>"),
        Some(PageError::NoFreeSlots)
    );
}
