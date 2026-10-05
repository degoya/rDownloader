//! Where a credential, a request and a redirect may go, which methods and headers a plugin may
//! send, and the failures a refused or broken request is reported as.
//!
//! Split out of `expand.rs` (PLUG-21). Like the rest of `expand`, nothing here touches the
//! database, the secret store or the network; the items `super::host` and its neighbours use
//! are `pub(in crate::native)`, the reach they had as `pub(super)` items of `expand`.

use rd_core::{Failure, FailureKind};
use rd_provider_registry::CredentialMode;
use url::Url;

/// Whether `reference` is the slot an account of `provider` in `mode` actually uses.
///
/// Ownership alone is not enough once a provider owns two slots: DDownload owns both
/// `ddownload_api_key` and `ddownload_password`, but an account in `api_key` mode must not be
/// able to have its API key posted to the website's login form, nor the reverse.
pub(in crate::native) fn reference_active_for_account(
    provider: &str,
    reference: &str,
    mode: Option<CredentialMode>,
) -> bool {
    rd_provider_registry::by_slug(provider).is_some_and(|spec| {
        let effective = spec.effective_credential_mode(mode);
        spec.secret_slot(reference)
            .is_some_and(|slot| slot.allows_mode(effective))
    })
}

pub(in crate::native) fn secret_domain_allowed(reference: &str, url: &Url) -> bool {
    rd_provider_registry::secret_domain_allowed(reference, url)
}

/// Whether `provider`'s account secret (and thus its username, treated the same way) may be
/// sent to `url`'s host. Providers without a secret slot never expand `{{username}}`, and a
/// provider with one slot per mode admits only the hosts of the account's active slot.
pub(in crate::native) fn username_domain_allowed(
    provider: &str,
    mode: Option<CredentialMode>,
    url: &Url,
) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    rd_provider_registry::by_slug(provider).is_some_and(|spec| {
        let effective = spec.effective_credential_mode(mode);
        spec.active_secret_slot(effective)
            .is_some_and(|slot| slot.domains.iter().any(|domain| domain == host))
    })
}

pub(in crate::native) fn validate_request_domain(url: &Url) -> Result<(), Failure> {
    let allowed = url
        .host_str()
        .is_some_and(rd_provider_registry::request_domain_allowed);
    if url.scheme() != "https" || !allowed {
        let host = url.host_str().unwrap_or("(no host)");
        return Err(Failure::coded(
            FailureKind::Permanent,
            "plugin.target_not_allowed",
            format!("Resolver target {host} is not allowed by the provider manifest"),
        )
        .with_param("host", host));
    }
    Ok(())
}

/// Base URL whose domain (and subdomains) receive the account's cookies.
///
/// The scope must not depend on the request: the client is cached per account, so the
/// first request (often the `api-v2.` metadata API) would otherwise decide where the
/// cookies go. Unknown providers fall back to the request host.
#[must_use]
pub fn provider_cookie_scope(provider: &str) -> Option<Url> {
    rd_provider_registry::cookie_scope(provider)
}

pub(in crate::native) fn cookie_scope(provider: Option<&str>, fallback: &Url) -> Url {
    provider
        .and_then(provider_cookie_scope)
        .unwrap_or_else(|| fallback.clone())
}

/// Hosters hand the file over to a CDN on another domain (ddownload: `*.zeuscdn.org`),
/// so plain requests may leave the manifest domains via HTTPS redirect. Requests that
/// carried a secret or username must stay on their manifest domain.
pub(in crate::native) fn validate_redirect(
    source: &Url,
    target: &Url,
    carries_credential: bool,
) -> Result<(), Failure> {
    if source.host_str() == target.host_str() {
        return Ok(());
    }
    if carries_credential {
        return validate_request_domain(target);
    }
    if target.scheme() != "https" || target.host_str().is_none() {
        return Err(Failure::coded(
            FailureKind::Permanent,
            "plugin.redirect_not_allowed",
            format!("Resolver redirect to {target} is not allowed"),
        )
        .with_param("target", target));
    }
    Ok(())
}

/// Whether a plugin may send this method at all.
///
/// Two lists, and the line between them is what the method *does* rather than how unusual it
/// is. `GET`, `POST`, `HEAD` and `PROPFIND` read: the last one is WebDAV's directory listing,
/// which is why a folder crawler needs it and why it used to be unreachable — it sat in the
/// write gate purely for being an uncommon verb. `PUT`, `DELETE` and `MKCOL` change what is at
/// the far end, and only a plugin whose whole purpose is that — a storage destination — may
/// use them.
pub(in crate::native) fn method_allowed(method: &str, write_methods: bool) -> bool {
    let reads = matches!(method, "GET" | "POST" | "HEAD" | "PROPFIND");
    let writes = write_methods && matches!(method, "PUT" | "DELETE" | "MKCOL");
    reads || writes
}

pub(in crate::native) fn allowed_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "authorization"
            | "content-type"
            | "range"
            | "user-agent"
            | "accept"
            // WebDAV's listing depth. A `PROPFIND` without it is served at the server's
            // default depth, which for a collection is either everything or nothing; a
            // crawler that could not say `1` could not list one folder.
            | "depth"
            // Free-download flows are indistinguishable from a browser without these: the
            // hoster's form posts are rejected without a matching Referer/Origin, and its
            // countdown endpoints only answer XMLHttpRequest calls.
            | "referer"
            | "origin"
            | "x-requested-with"
            // Box (`box`, `box-crawler`): the API opens a shared link only through this
            // header, `shared_link=<link>[&shared_link_password=<password>]` — there is no
            // query or body spelling. It carries the link someone pasted, never an account
            // secret, so it is on the list below as well (RD-120-60).
            | "boxapi"
            // KrakenFiles (`krakenfiles`): the free-download POST is answered only with the
            // page's `data-file-hash` in a `hash` header, the way the site's own script and
            // pyLoad send it. A value read off a public page, never a credential (RD-120-60).
            | "hash"
    )
}

/// Headers that describe where a request came from, or carry a value a plugin read off a
/// link or a page. They travel to hosts a vault credential was never meant for, so credential
/// markers must never be expanded into them — secrets stay in `authorization`, the query
/// string and the body, as before.
pub(super) fn header_forbids_credentials(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "referer" | "origin" | "x-requested-with" | "boxapi" | "hash"
    )
}

pub(in crate::native) fn http_failure(error: reqwest::Error) -> Failure {
    // `reqwest::Error`'s `Display` appends " for url (...)" with the fully expanded URL,
    // which for several plugins carries the account password or premium key in the query
    // string. Strip it before it reaches the persisted, redaction-safe failure message.
    let error = error.without_url();
    // `without_url` only drops the URL reqwest itself attached; a plugin that expanded a
    // secret into a message of its own still needs redacting. The causes go along: the top
    // line alone ("error sending request") hides a failed DNS lookup or a refused connection.
    let message = rd_core::error_with_causes(&error);
    transient(
        "plugin.http_error",
        &format!("Resolver HTTP error: {message}"),
    )
    .with_param("error", message)
}

pub(in crate::native) fn transient(code: &str, message: &str) -> Failure {
    Failure::coded(
        FailureKind::Transient {
            retry_after_seconds: None,
        },
        code,
        message,
    )
}

/// Whether a provider signed in with OAuth carries its access token onto the transfer itself,
/// and whether `target` is one of the hosts it may reach (RD-106-04).
///
/// A cloud drive is the case this exists for. Its bytes come from an API address that answers
/// with the account's access token and with nothing else, and neither half of the usual
/// arrangement fits: a resolver states download headers as *values*, and it has none to state
/// — the whole point of `store-oauth-token` is that no plugin ever reads a credential back.
/// So the decision moves to where the credential already lives, and stays as narrow as it can
/// be: only `credentials = "oauth"`, only the exact hosts that provider's own manifest listed
/// under `secret_domains`, and checked against the address the transfer actually goes to
/// rather than the one it started from — a resolver that answered with somebody else's host
/// must not take the token there.
#[must_use]
pub fn provider_download_bearer(provider: &str, target: &Url) -> bool {
    rd_provider_registry::by_slug(provider).is_some_and(|spec| bearer_allowed(&spec, target))
}

/// Whether this provider keeps its access token beside the sign-in flow rather than in the
/// account's own credential slot (RD-106-03).
///
/// Almost every OAuth provider stores the token as the account's secret, and asking is then
/// pointless. The exception is a provider whose person registers their own application: there
/// the account's secret is the *client* secret, every renewal still needs it, and the token
/// lives beside the flow. Anything that turns a stored credential into a request has to know
/// which of the two it is holding, or it hands the provider the wrong one.
#[must_use]
pub fn provider_token_beside_the_flow(provider: &str) -> bool {
    rd_provider_registry::by_slug(provider).is_some_and(|spec| token_beside_the_flow(&spec))
}

/// The decision itself, apart from the lookup so it can be driven without the process-wide
/// provider table.
pub(super) fn token_beside_the_flow(spec: &rd_provider_registry::ProviderSpec) -> bool {
    spec.flow_secret_slot().is_some()
}

/// The decision itself, apart from the lookup so it can be driven without the process-wide
/// provider table.
pub(in crate::native) fn bearer_allowed(
    spec: &rd_provider_registry::ProviderSpec,
    target: &Url,
) -> bool {
    // Only over TLS. An access token in a cleartext header is a token published.
    if target.scheme() != "https" {
        return false;
    }
    let Some(host) = target.host_str() else {
        return false;
    };
    spec.credentials == rd_provider_registry::CredentialKind::OAuth
        && spec
            .secrets
            .iter()
            .any(|slot| slot.domains.iter().any(|domain| domain == host))
}
