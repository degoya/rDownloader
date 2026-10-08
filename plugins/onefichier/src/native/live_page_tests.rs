//! The two pages the live site served on 2026-09-20 (RD-109-36), through the free flow in
//! `free_tests.rs`, whose request, fixtures and helpers they use; split from it to keep both
//! files under the crate layout's 500 lines.

use rd_plugin_api::{HostHttpResponse, Resolver};
use rd_plugin_types::FailureKind;

use super::{DOWNLOAD_URL, LINK_PAGE, MockHost, free_request, html, resolver};

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
        resolved.size.map(rd_plugin_types::ByteCount::get),
        Some(425_134_653)
    );
}
