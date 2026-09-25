//! Account-less (free) resolve coverage for 1fichier's website flow.
//!
//! Drives the real resolver against queued mock responses and asserts what the plugin *did* —
//! which requests, in which order, with which body and which `Referer` — not only what it
//! returned. The flow itself and its JD provenance are documented in `crate::page`.

use std::sync::Arc;

use rd_core::FailureKind;
use rd_plugin_api::{
    ClientIdentity, HostHttpRequest, HostHttpResponse, ResolveRequest, ResolvedHeader, Resolver,
    ResolverHost,
};

use super::super::OneFichierResolver;
use super::MockHost;

/// The account-less request the free flow answers. Uses the `alterupload.com` alias on purpose:
/// the flow must stay on the domain the link came with (see `page`'s module doc).
fn free_request() -> ResolveRequest {
    ResolveRequest {
        url: "https://alterupload.com/?abc12defg3".parse().expect("URL"),
        client: ClientIdentity {
            account_id: None,
            proxy_profile_id: None,
            tls_revision: 0,
        },
    }
}

const PAGE_URL: &str = "https://alterupload.com/?abc12defg3&lg=en";
const DOWNLOAD_URL: &str = "https://a-7.alterupload.com/p123456789";

/// The file page's download form, with the hidden fields the live site carries.
const FORM_PAGE: &str = r#"<html><head><title>alterupload.com: release.rar</title></head><body>
<form method="POST" action="">
<input type="hidden" name="adz" value="1a2b3c">
<input type="checkbox" name="save" value="1">
</form></body></html>"#;

/// The answer to the posted form: the download button.
const LINK_PAGE: &str = r#"<html><body><a href="https://a-7.alterupload.com/p123456789">Click here to download</a></body></html>"#;

fn html(body: &str) -> HostHttpResponse {
    HostHttpResponse {
        status: 200,
        final_url: PAGE_URL.parse().expect("URL"),
        headers: vec![ResolvedHeader {
            name: "Content-Type".to_owned(),
            value: "text/html; charset=UTF-8".to_owned(),
        }],
        body: body.as_bytes().to_vec(),
    }
}

fn file(final_url: &str) -> HostHttpResponse {
    HostHttpResponse {
        status: 206,
        final_url: final_url.parse().expect("URL"),
        headers: vec![ResolvedHeader {
            name: "Content-Disposition".to_owned(),
            value: "attachment; filename=\"release.rar\"".to_owned(),
        }],
        body: vec![0],
    }
}

fn resolver(host: &Arc<MockHost>) -> OneFichierResolver {
    OneFichierResolver::new(Arc::clone(host) as Arc<dyn ResolverHost>)
}

fn body_of(request: &HostHttpRequest) -> String {
    String::from_utf8_lossy(&request.body).into_owned()
}

fn header_of<'a>(request: &'a HostHttpRequest, name: &str) -> Option<&'a str> {
    request
        .headers
        .iter()
        .find(|header| header.name.eq_ignore_ascii_case(name))
        .map(|header| header.value_template.as_str())
}

#[tokio::test]
async fn the_free_flow_fetches_the_page_posts_the_form_and_follows_the_download_button() {
    let host = MockHost::free(
        vec![html(FORM_PAGE), html(LINK_PAGE), file(DOWNLOAD_URL)],
        Some("unused"),
    );
    let resolved = resolver(&host)
        .resolve(free_request())
        .await
        .expect("free download resolves");

    assert_eq!(resolved.url.as_str(), DOWNLOAD_URL);
    assert_eq!(resolved.file_name.as_deref(), Some("release.rar"));
    // The transfer must carry the referer, or 1fichier refuses the direct link.
    assert_eq!(
        resolved
            .headers
            .iter()
            .find(|header| header.name == "Referer")
            .map(|header| header.value.as_str()),
        Some("https://alterupload.com/")
    );
    assert!(resolved.client.account_id.is_none());

    let requests = host.requests.lock().expect("mock lock");
    assert_eq!(requests.len(), 3, "page, form post, direct link");

    // 1. The page is fetched on the domain the link carried, with English forced.
    assert_eq!(requests[0].method, "GET");
    assert_eq!(requests[0].url.as_str(), PAGE_URL);
    // No secret may be templated into the account-less flow's requests.
    assert!(
        !requests.iter().any(|request| request
            .headers
            .iter()
            .any(|header| header.value_template.contains("{{secret:")
                || header.value_template.contains("{{username}}"))),
        "the free flow must never carry a credential"
    );

    // 2. The form is posted back to the page, with a matching referer, `save` dropped and `did=1`.
    assert_eq!(requests[1].method, "POST");
    assert_eq!(requests[1].url.as_str(), PAGE_URL);
    assert_eq!(
        header_of(&requests[1], "Referer"),
        Some("https://alterupload.com/")
    );
    assert_eq!(
        header_of(&requests[1], "Content-Type"),
        Some("application/x-www-form-urlencoded")
    );
    let posted = body_of(&requests[1]);
    assert!(posted.contains("adz=1a2b3c"), "{posted}");
    assert!(posted.contains("did=1"), "{posted}");
    assert!(!posted.contains("save"), "{posted}");

    // 3. The download button's link is probed with the same referer.
    assert_eq!(requests[2].method, "GET");
    assert_eq!(requests[2].url.as_str(), DOWNLOAD_URL);
    assert_eq!(
        header_of(&requests[2], "Referer"),
        Some("https://alterupload.com/")
    );

    // The flow is captcha-free and needs no countdown.
    assert!(host.captchas.lock().expect("mock lock").is_empty());
    assert!(host.waits.lock().expect("mock lock").is_empty());
}

/// A hotlink — 1fichier serving the file straight from the link page — needs no form at all.
#[tokio::test]
async fn a_hotlink_short_circuits_the_whole_flow() {
    let host = MockHost::free(vec![file(DOWNLOAD_URL)], None);
    let resolved = resolver(&host)
        .resolve(free_request())
        .await
        .expect("hotlink");

    assert_eq!(resolved.url.as_str(), DOWNLOAD_URL);
    assert_eq!(resolved.file_name.as_deref(), Some("release.rar"));
    assert_eq!(host.requests.lock().expect("mock lock").len(), 1);
}

/// A stated wait must be reported as `IpBlocked` with the parsed seconds, so the scheduler holds
/// back the hoster's other free links — and it must be found before the form is posted.
#[tokio::test]
async fn a_stated_limit_becomes_an_ip_block_with_the_parsed_seconds() {
    let host = MockHost::free(
        vec![html(
            "<div class=\"ct_warn\">Warning ! Without premium status, you must wait up to 12 minutes between each downloads</div>",
        )],
        Some("unused"),
    );
    let failure = resolver(&host)
        .resolve(free_request())
        .await
        .expect_err("a limit must fail the resolve");

    assert_eq!(
        failure.category,
        FailureKind::IpBlocked {
            retry_after_seconds: Some(12 * 60)
        }
    );
    assert_eq!(failure.code.as_deref(), Some("1fichier.free_limit_reached"));
    assert_eq!(
        failure.params.get("wait_seconds").map(String::as_str),
        Some("720")
    );
    assert_eq!(
        host.requests.lock().expect("mock lock").len(),
        1,
        "the form must not be posted into a stated limit"
    );
    assert!(
        host.captchas.lock().expect("mock lock").is_empty(),
        "no captcha may be spent on a link that is blocked anyway"
    );
    assert!(host.waits.lock().expect("mock lock").is_empty());
}

/// A limit stated without a duration keeps JD's five-minute default.
#[tokio::test]
async fn a_limit_without_a_duration_still_blocks_the_hoster() {
    let host = MockHost::free(
        vec![html(
            "<div>Without Premium, you can only download one file at a time</div>",
        )],
        None,
    );
    let failure = resolver(&host)
        .resolve(free_request())
        .await
        .expect_err("a limit must fail");

    assert_eq!(
        failure.category,
        FailureKind::IpBlocked {
            retry_after_seconds: Some(300)
        }
    );
}

/// The hoster running out of free slots is its own case: the link is fine, the hoster is busy.
#[tokio::test]
async fn the_no_free_slots_notice_blocks_the_hoster_under_its_own_code() {
    let host = MockHost::free(
        vec![html(
            "<div>Free download is temporarily limited due to high demand</div>",
        )],
        None,
    );
    let failure = resolver(&host)
        .resolve(free_request())
        .await
        .expect_err("no free slots");

    assert_eq!(
        failure.category,
        FailureKind::IpBlocked {
            retry_after_seconds: Some(300)
        }
    );
    assert_eq!(failure.code.as_deref(), Some("1fichier.no_free_slots"));
}

/// An offline file must be reported as offline, distinctly from every other page problem.
#[tokio::test]
async fn an_offline_file_is_reported_as_offline() {
    let host = MockHost::free(
        vec![html("<div class=\"ct_warn\">File not found !</div>")],
        None,
    );
    let failure = resolver(&host)
        .resolve(free_request())
        .await
        .expect_err("offline file");

    assert_eq!(failure.category, FailureKind::Offline);
    assert_eq!(failure.code.as_deref(), Some("1fichier.file_offline"));
}

/// An HTTP 404 for the page itself is the other way 1fichier reports an offline file.
#[tokio::test]
async fn a_404_page_is_reported_as_offline() {
    let host = MockHost::free(
        vec![HostHttpResponse {
            status: 404,
            final_url: PAGE_URL.parse().expect("URL"),
            headers: Vec::new(),
            body: Vec::new(),
        }],
        None,
    );
    let failure = resolver(&host)
        .resolve(free_request())
        .await
        .expect_err("offline file");

    assert_eq!(failure.category, FailureKind::Offline);
    assert_eq!(failure.code.as_deref(), Some("1fichier.file_offline"));
}

/// A password-protected file is reported under its own code — the account-less flow has no
/// download password to submit, so posting the form would only fail on the next page.
#[tokio::test]
async fn a_password_protected_file_is_reported_before_the_form_is_posted() {
    let host = MockHost::free(
        vec![html(
            r#"<html><body><form method="POST" action="">
<input type="hidden" name="adz" value="1a2b3c">
<input type="password" name="pass" value="">
</form></body></html>"#,
        )],
        None,
    );
    let failure = resolver(&host)
        .resolve(free_request())
        .await
        .expect_err("password-protected file");

    assert_eq!(failure.category, FailureKind::Permanent);
    assert_eq!(failure.code.as_deref(), Some("1fichier.password_required"));
    assert_eq!(
        host.requests.lock().expect("mock lock").len(),
        1,
        "a form that needs a password must not be posted"
    );
}

/// A file only registered users can download must say so, not fail as a broken page.
#[tokio::test]
async fn an_account_only_file_reports_that_an_account_is_required() {
    let host = MockHost::free(
        vec![html(
            "<div>Sorry, it is not possible to free unregistered users</div>",
        )],
        None,
    );
    let failure = resolver(&host)
        .resolve(free_request())
        .await
        .expect_err("account required");

    assert_eq!(failure.category, FailureKind::AuthRequired);
    assert_eq!(failure.code.as_deref(), Some("1fichier.account_required"));
}

/// A page with no form at all is a plugin/site mismatch; the page's own wording must reach the
/// user rather than a bare "something went wrong".
#[tokio::test]
async fn a_page_without_a_form_reports_a_diagnosis() {
    let host = MockHost::free(
        vec![html(
            "<html><head><title>1fichier.com: Maintenance</title></head><body></body></html>",
        )],
        None,
    );
    let failure = resolver(&host)
        .resolve(free_request())
        .await
        .expect_err("no form to post");

    assert_eq!(failure.category, FailureKind::Permanent);
    assert_eq!(failure.code.as_deref(), Some("1fichier.no_free_form"));
    assert_eq!(
        failure.params.get("diagnosis").map(String::as_str),
        Some("1fichier.com: Maintenance")
    );
}

/// The form was accepted but the answer carries no download link: another clear coded failure,
/// never a silently wrong URL.
#[tokio::test]
async fn a_posted_form_without_a_link_reports_a_diagnosis() {
    let host = MockHost::free(
        vec![
            html(FORM_PAGE),
            html(
                "<html><head><title>1fichier.com: Nothing here</title></head><body><p>?</p></body></html>",
            ),
        ],
        None,
    );
    let failure = resolver(&host)
        .resolve(free_request())
        .await
        .expect_err("no link on the page");

    assert_eq!(failure.category, FailureKind::Permanent);
    assert_eq!(failure.code.as_deref(), Some("1fichier.no_free_link"));
    assert!(failure.params.contains_key("diagnosis"));
}

/// A server-side problem is transient, not a permanent failure of the link.
#[tokio::test]
async fn a_server_error_page_is_transient() {
    let host = MockHost::free(vec![html("<h1>Software error:</h1>")], None);
    let failure = resolver(&host)
        .resolve(free_request())
        .await
        .expect_err("server error");

    assert_eq!(
        failure.category,
        FailureKind::Transient {
            retry_after_seconds: Some(600)
        }
    );
    assert_eq!(failure.code.as_deref(), Some("1fichier.server_error"));
}

/// The free path must be reachable without any account at all — the `requires_account = false`
/// metadata the host dispatches on. The API-key path stays account-only.
///
/// `metadata.domains` is the intake list: the hosts whose links this resolver claims. What it
/// may *request* is the `net_http` capability, enforced per plugin by the host and covered
/// there (`native::granted::tests`), because that list also carries the API and CDN hosts no
/// link is ever posted for.
#[test]
fn the_resolver_no_longer_requires_an_account() {
    let host = MockHost::free(Vec::new(), None);
    let metadata = resolver(&host).metadata().clone();
    assert!(!metadata.requires_account);
    for domain in [
        "1fichier.com",
        "www.1fichier.com",
        "alterupload.com",
        "pjointe.com",
    ] {
        assert!(
            metadata.domains.iter().any(|value| value == domain),
            "{domain} must be claimed at intake"
        );
    }
}

/// The two pages the live site served on 2026-09-20, trimmed to the parts that decide the flow:
/// the head assets that pass `is_content_host`, the file card, the form, and the answer that
/// says there is no free slot.
const REAL_PAGE_HEAD: &str = r#"<html><head><title>1fichier.com: Cloud Storage</title>
<link rel="icon" href="https://img.1fichier.com/favicon.ico" />
<link rel="stylesheet" href="https://img.1fichier.com/css/style.css" />
</head><body>"#;

/// RD-109-36, end to end: the page that answers the posted form carries no download button, only
/// the hoster's own head assets. The flow must stop at the notice, and must never fetch one of
/// those assets as if it were the file.
#[tokio::test]
async fn the_out_of_slots_answer_stops_the_flow_instead_of_fetching_an_asset() {
    let card_page = format!(
        r#"{REAL_PAGE_HEAD}
<div class="tier-body"><span class="tier-name">outlander.s08e01.german.bdrip.x264-intention.rar</span>
<span class="tier-feat">405.44 MB</span></div>
<form method="POST" action=""><input type="checkbox" name="dl_no_ssl" /></form>
</body></html>"#
    );
    let refusal = format!(
        r#"{REAL_PAGE_HEAD}
<div>High demand: all free guest slots are currently in use.</div>
<a href="/login.pl">Sign in and download now</a>
</body></html>"#
    );
    let host = MockHost::free(vec![html(&card_page), html(&refusal)], None);
    let failure = resolver(&host)
        .resolve(free_request())
        .await
        .expect_err("no free slot is not a download");

    assert_eq!(failure.code.as_deref(), Some("1fichier.no_free_slots"));
    assert_eq!(
        failure.category,
        FailureKind::IpBlocked {
            retry_after_seconds: Some(300)
        }
    );
    let requests = host.requests.lock().expect("mock lock");
    assert_eq!(
        requests.len(),
        2,
        "the page and the form post, nothing else"
    );
    assert!(
        !requests
            .iter()
            .any(|request| request.url.as_str().contains("img.1fichier.com")),
        "a favicon is not the payload: {:?}",
        requests.iter().map(|r| r.url.as_str()).collect::<Vec<_>>()
    );
}

/// RD-109-36: the link page states the name and the size, so a transfer starts with both known
/// even when the direct link answers without a `Content-Disposition`.
#[tokio::test]
async fn the_page_card_supplies_the_name_and_size_the_transfer_needs() {
    let card_page = format!(
        r#"{REAL_PAGE_HEAD}
<div class="tier-body"><span class="tier-name">outlander.s08e01.german.bdrip.x264-intention.rar</span>
<span class="tier-feat">405.44 MB</span></div>
<form method="POST" action=""></form>
</body></html>"#
    );
    let bare = HostHttpResponse {
        status: 206,
        final_url: DOWNLOAD_URL.parse().expect("URL"),
        headers: Vec::new(),
        body: vec![0],
    };
    let host = MockHost::free(vec![html(&card_page), html(LINK_PAGE), bare], None);
    let resolved = resolver(&host)
        .resolve(free_request())
        .await
        .expect("free download resolves");

    assert_eq!(
        resolved.file_name.as_deref(),
        Some("outlander.s08e01.german.bdrip.x264-intention.rar")
    );
    assert_eq!(
        resolved.size.map(rd_core::ByteCount::get),
        Some(425_134_653)
    );
}
