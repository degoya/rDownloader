//! The account-less (free) XFS download flow, as FileJoker runs it.
//!
//! The flow is `xfs_common::free::FreeFlow` (RD-1120-10), JD's `XFileSharingProBasic.doFree`;
//! what is FileJoker's own is a field each: every page is checked for FileJoker's own dead ends
//! ([`free_page_failure`]), the countdown is read off its `Please Wait` marker first, and the
//! direct link has to be on `filejoker.net`. See `crate::page`'s module doc for the verification
//! record behind every marker.

use plugin_common::failure::{diagnosed, free_limit};
use plugin_common::{Failure, FailureKind, PluginHost, ResolveInput, Resolved};
use url::Url;
use xfs_common::free::FreeFlow;

use super::api::{FREE, PRIMARY_DOMAIN, coded};
use crate::{messages, page};

/// FileJoker's free flow.
const FLOW: FreeFlow = FreeFlow {
    words: FREE,
    invalid_url: messages::INVALID_URL,
    captcha_rejected: messages::CAPTCHA_REJECTED,
    rewrite_host: None,
    download2_on_file_page: false,
    free_fields: page::free_form,
    page_check: Some(free_page_failure),
    widget_marker: xfs_common::free::widget_marker,
    wait_seconds: page::free_wait_seconds,
    direct_link: |html, _, hints| page::direct_link(html, hints),
    referer: |_| xfs_common::site::referer(PRIMARY_DOMAIN),
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

/// Aborts a free flow on any page that cannot lead to a download: an IP limit first (reported as
/// `IpBlocked` so the scheduler holds back the hoster's other free links instead of spending
/// another wait and captcha on each of them), then FileJoker's offline, free-size-limit and
/// premium-only markers, none of which a wait or a captcha can get past.
fn free_page_failure(html: &str) -> Result<(), Failure> {
    free_limit(
        page::ip_block_seconds(html),
        messages::FREE_LIMIT_REACHED,
        messages::free_limit_reached,
    )?;
    if page::is_file_offline(html) {
        return Err(coded(FailureKind::Offline, messages::FILE_OFFLINE));
    }
    if page::is_free_size_limited(html) {
        return Err(coded(FailureKind::AuthRequired, messages::FREE_SIZE_LIMIT));
    }
    if page::is_premium_only(html) {
        return Err(diagnosed(
            FailureKind::AuthRequired,
            messages::NO_PREMIUM_FILE,
            messages::no_premium_file,
            page::diagnose(html),
        ));
    }
    Ok(())
}
