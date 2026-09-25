//! The account credential a **transfer** carries, as opposed to a resolver's API call.
//!
//! A resolver reaches its provider through markers the host expands; the bytes are fetched by
//! the download engine, which runs no plugin code and so needs the decision made for it. Two
//! shapes exist. An OAuth provider's access token rides as `Bearer` (RD-106-04). A provider
//! whose row declares `transfer_auth = "basic"` gets `Basic base64(username:secret)`
//! (RD-120-38) -- Seedr, whose file addresses answer HTTP Basic and nothing else, and
//! Pixeldrain, whose API key is a Basic password under an empty user name.
//!
//! Both are held to one gate: only over TLS, and only to an exact host the credential's own
//! slot lists under `secret_domains`, checked against the address the transfer actually goes
//! to. The caller asks again for every address it sends to, which is what keeps a redirect to
//! a foreign host from inheriting the header.

use rd_core::Failure;
use rd_provider_registry::{CredentialKind, ProviderSpec, TransferAuth};
use url::Url;

use super::expand::{basic_credential, bearer_allowed};

/// The `Authorization` value a transfer to `target` carries for an account of `provider`, or
/// `None` when it carries nothing.
///
/// `username` and `secret` are the account's own. The refusals are the ones a resolver's
/// `{{basic:…}}` marker gives, from the same builder: a missing user name where the provider
/// requires one is `plugin.username_missing`, a colon in it `plugin.basic_username_invalid`.
/// Neither carries any part of the credential.
///
/// # Errors
///
/// Only for a Basic provider whose account cannot form the pair; see above.
pub fn provider_download_authorization(
    provider: &str,
    target: &Url,
    username: Option<&str>,
    secret: &str,
) -> Result<Option<String>, Failure> {
    let Some(spec) = rd_provider_registry::by_slug(provider) else {
        return Ok(None);
    };
    download_authorization(&spec, target, username, secret)
}

/// Whether a transfer to `target` carries anything for an account of `provider` at all.
///
/// The gate alone, without a credential in hand, so a caller can decide before it opens the
/// vault. [`provider_download_authorization`] applies the same gate again.
#[must_use]
pub fn provider_download_carries_credential(provider: &str, target: &Url) -> bool {
    rd_provider_registry::by_slug(provider)
        .is_some_and(|spec| bearer_allowed(&spec, target) || basic_allowed(&spec, target))
}

/// The decision itself, apart from the lookup so it can be driven without the process-wide
/// provider table.
pub(super) fn download_authorization(
    spec: &ProviderSpec,
    target: &Url,
    username: Option<&str>,
    secret: &str,
) -> Result<Option<String>, Failure> {
    if bearer_allowed(spec, target) {
        return Ok(Some(format!("Bearer {secret}")));
    }
    if !basic_allowed(spec, target) {
        return Ok(None);
    }
    let pair = basic_credential(secret, username, !spec.username_required)?;
    Ok(Some(format!("Basic {pair}")))
}

/// Whether a `transfer_auth = "basic"` provider's credential may be sent to `target`.
///
/// The same gate the Bearer path has, applied to the one slot the person fills: TLS, and an
/// exact host from that slot's `secret_domains`. The kind is checked again although the
/// manifest validation already refuses anything else, because this is the last place before
/// a password leaves the machine.
pub(super) fn basic_allowed(spec: &ProviderSpec, target: &Url) -> bool {
    if target.scheme() != "https" {
        return false;
    }
    let Some(host) = target.host_str() else {
        return false;
    };
    spec.transfer_auth == TransferAuth::Basic
        && matches!(
            spec.credentials,
            CredentialKind::ApiKey | CredentialKind::UsernamePassword
        )
        && spec
            .person_secret_slot()
            .is_some_and(|slot| slot.domains.iter().any(|domain| domain == host))
}

#[cfg(test)]
mod tests;
