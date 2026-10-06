//! The account-less (free) XFS download flow, as DDownload runs it.
//!
//! The flow is `xfs_common::free::FreeFlow` (RD-1120-10), JD's `XFileSharingProBasic.doFree`,
//! which `DdownloadCom.doFree` calls straight through to (it only adds the "free dialog" prompt on
//! top). What is DDownload's own is a field each: since 2026-09-17 the file page carries the
//! `download2` form directly and no `download1` at all (RD-108-28), so the first step is skipped
//! when the page already offers the second; both forms are posted with `adblock_detected`
//! cleared; the captcha scan starts at the form; the countdown is `dk2CountdownNum`; and the
//! direct link may sit on the CDN. See `crate::page`'s module doc for the full IMPL-VERIFY record.

use plugin_common::{Failure, PluginHost, ResolveInput, Resolved};
use url::Url;
use xfs_common::free::FreeFlow;

use super::api::{FREE, SITE};
use crate::{messages, page};

/// DDownload's free flow.
const FLOW: FreeFlow = FreeFlow {
    words: FREE,
    invalid_url: messages::INVALID_URL,
    captcha_rejected: messages::CAPTCHA_REJECTED,
    rewrite_host: None,
    download2_on_file_page: true,
    free_fields,
    page_check: None,
    widget_marker: page::widget_marker,
    wait_seconds: page::free_wait_seconds,
    // `page::direct_link` accepts ddownload.com and the `*.zeuscdn.org` delivery hosts this
    // plugin's manifest allows; a free link served from another alias JD lists (`ucdn.to`) would
    // surface as `no_free_link` rather than as a wrong download.
    direct_link: |html, _, hints| page::direct_link(html, hints),
    referer: |_| SITE.referer_header(),
};

/// Runs the account-less XFS free flow and turns its result into a transfer.
pub(super) async fn resolve<H: PluginHost>(
    host: &H,
    request: &ResolveInput,
    parsed: &Url,
    code: &str,
) -> Result<Resolved, Failure> {
    FLOW.resolve(host, &request.url, parsed, code).await
}

/// The free submission for either step: `method_free` kept, `method_premium` dropped, and
/// ddownload's `adblock_detected` field cleared when the form carries it.
fn free_fields(fields: &[(String, String)]) -> Vec<(String, String)> {
    page::with_adblock_cleared(&page::free_form(fields))
}
