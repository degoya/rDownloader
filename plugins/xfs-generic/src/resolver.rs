//! The protocol logic, written once for both builds.
//!
//! Short by design. There is no account, so there is no login, no cookie session and no premium
//! transfer — only the standard XFS free flow, which lives in [`free`]. What is left here is the
//! entry point, the host list, and the two answers that exist because the `Resolver` contract
//! asks for them and this plugin has nothing to give: a link check and an account check.

pub(crate) mod api;
mod free;

use plugin_common::{
    Account, CheckInput, Failure, FailureKind, Header, LinkCheck, PluginHost, ResolveInput,
    Resolved,
};
use url::Url;

use self::api::coded;
use crate::messages;

/// Whether this plugin claims `url`.
///
/// Two conditions, both required: the host is one of [`crate::HOSTERS`] exactly, and the path
/// looks like an XFS file code. Nothing here touches the network — the conformance check runs
/// this against a host that refuses every call, and a resolver reaching out during `match-url`
/// is doing something it has no business doing.
#[must_use]
pub(crate) fn matches(url: &str) -> bool {
    Url::parse(url)
        .ok()
        .as_ref()
        .and_then(api::file_code)
        .is_some()
}

/// Hoster domains this plugin serves. Neither the host nor the account changes the answer; the
/// arguments exist because a multihoster's catalogue does depend on both.
pub(crate) async fn hosters<H: PluginHost>(
    _host: &H,
    _account_id: &str,
) -> Result<Vec<String>, Failure> {
    Ok(crate::HOSTERS
        .iter()
        .map(|host| (*host).to_owned())
        .collect())
}

/// XFS exposes link status through its account API, which is exactly what this plugin does not
/// have. Fetching the page instead would be resolving, so there is nothing cheaper to offer.
pub(crate) async fn check<H: PluginHost>(
    _host: &H,
    _request: &CheckInput,
) -> Result<Vec<LinkCheck>, Failure> {
    Err(coded(FailureKind::Unsupported, messages::CHECK_UNSUPPORTED))
}

/// There is no account to check. Reported as such rather than answered with a cheerful `valid`,
/// which would claim something this provider cannot have.
pub(crate) async fn check_account<H: PluginHost>(
    _host: &H,
    _account_id: &str,
) -> Result<Account, Failure> {
    Err(coded(FailureKind::Unsupported, messages::NO_ACCOUNT))
}

/// Turns a link into a download through the free flow. The account, if the request carries one,
/// is deliberately ignored: it cannot belong to this provider, which has none.
pub(crate) async fn resolve<H: PluginHost>(
    host: &H,
    request: &ResolveInput,
) -> Result<Resolved, Failure> {
    let parsed = Url::parse(&request.url).map_err(|error| api::invalid_url(&error))?;
    let code = api::file_code(&parsed)
        .ok_or_else(|| coded(FailureKind::Unsupported, messages::UNSUPPORTED_LINK))?
        .to_owned();
    free::resolve(host, request, &parsed, &code).await
}

/// The file name segment of a `/<code>/<name>` link.
pub(super) fn second_path_segment(url: &Url) -> Option<String> {
    url.path_segments()
        .and_then(|segments| segments.filter(|segment| !segment.is_empty()).nth(1))
        .map(str::to_owned)
}

/// The `Referer` a free transfer must carry, so the site sees the page that earned it.
///
/// Taken from the link rather than from a constant: which site this is, is only known once a URL
/// has arrived.
pub(super) fn referer_header(page_url: &Url) -> Header {
    Header::new("Referer", origin_of(page_url))
}

/// `https://host/` for a link, the form XFS installations expect as a referer.
pub(super) fn origin_of(url: &Url) -> String {
    match url.host_str() {
        Some(host) => format!("{}://{host}/", url.scheme()),
        None => url.as_str().to_owned(),
    }
}
