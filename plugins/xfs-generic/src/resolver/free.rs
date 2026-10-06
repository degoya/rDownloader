//! The account-less (free) XFS download flow.
//!
//! The standard shape, and only the standard shape: `xfs_common::free::FreeFlow` (RD-1120-10)
//! fetches the file page, posts the `download1` form in free mode, solves whatever captcha the
//! answer asks for, waits out the countdown, posts `download2`, and takes the direct link from
//! what comes back. Every page is checked for an IP limit first, because hitting one means no
//! amount of waiting or captcha solving will help until it expires.
//!
//! FileJoker runs the same flow with its own markers on top. Where that plugin recognises
//! FileJoker's own phrasings for an offline file, a premium-only file and a size limit, this one
//! does not: it cannot know which clone it is talking to, and guessing would produce a confident
//! wrong answer. Such a page falls through to `no_free_form` or `no_free_link`
//! (`xfs_common::free::FreeWords`), both of which carry the page's own diagnosis, so the failure
//! names a cause instead of being empty. What this plugin adds is what having no site of its own
//! asks for: the free button keeps the page's own label, and the link host and the referer are
//! taken from the link being resolved.

use plugin_common::{Failure, PluginHost, ResolveInput, Resolved};
use url::Url;
use xfs_common::free::FreeFlow;

use super::api::FREE;
use crate::{messages, page};

/// The generic free flow.
const FLOW: FreeFlow = FreeFlow {
    words: FREE,
    invalid_url: messages::INVALID_URL,
    captcha_rejected: messages::CAPTCHA_REJECTED,
    rewrite_host: None,
    download2_on_file_page: false,
    free_fields: page::free_form,
    page_check: None,
    widget_marker: page::widget_marker,
    wait_seconds: page::free_wait_seconds,
    // The link may sit on a delivery host of the same site rather than on the page's own host;
    // what it must not do is leave the sandbox, and the host's network gate enforces that on the
    // request whatever this plugin believes.
    direct_link,
    // XFS installations reject a direct link fetched without the page it came from as referer.
    referer: super::referer_header,
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

/// The direct link on the final page.
///
/// Candidate hosts are the page's own host and its registrable-looking parent, so a delivery
/// subdomain of the same site is accepted while an unrelated host is not.
fn direct_link(html: &str, parsed: &Url, hints: &[&str]) -> Option<String> {
    let host = parsed.host_str()?;
    let mut domains = vec![host];
    if let Some((_, parent)) = host.split_once('.')
        && parent.contains('.')
    {
        domains.push(parent);
    }
    page::direct_link(html, hints, &domains)
}

#[cfg(test)]
#[path = "free/tests.rs"]
mod tests;
