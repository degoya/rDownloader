//! The account-less (free) XFS download flow, as KatFile runs it.
//!
//! The flow is `xfs_common::free::FreeFlow` (RD-1120-10), JD's `XFileSharingProBasic.doFree`;
//! what is KatFile's own is a field each: the link is browsed on today's main domain
//! (`canonicalize_host`, JD's `KatfileCom.rewriteHost`), the countdown is read off its own
//! `var estimated_time` marker first, and the direct link has to be on `katfile.biz`.

use plugin_common::{Failure, PluginHost, ResolveInput, Resolved};
use url::Url;
use xfs_common::free::FreeFlow;

use super::api::{FREE, SITE, canonicalize_host};
use crate::{messages, page};

/// KatFile's free flow.
const FLOW: FreeFlow = FreeFlow {
    words: FREE,
    invalid_url: messages::INVALID_URL,
    captcha_rejected: messages::CAPTCHA_REJECTED,
    rewrite_host: Some(canonicalize_host),
    download2_on_file_page: false,
    free_fields: page::free_form,
    page_check: None,
    widget_marker: xfs_common::free::widget_marker,
    wait_seconds: page::free_wait_seconds,
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
