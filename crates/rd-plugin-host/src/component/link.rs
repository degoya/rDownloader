//! The interfaces a manifest's capabilities grant, linked in one place (RD-1280-21).
//!
//! The resolver, the transfer backend and every extension wrapper built their linker with the
//! same capability checks in three copies; three copies were three chances for one of them to
//! link an interface the manifest never asked for, or to miss one it did. What a plugin can
//! reach is decided here. What a plugin type *is* — a transfer's sink, a post-processor's
//! source, an authentication plugin's credential store — stays with its type's wrapper.
//!
//! The three `http-request` implementations the wrappers seemed to carry are one: the transfer
//! and extension bindings map `rdownloader:plugin/http` onto this module's, so every type links
//! the same generated `add_to_linker` over the one `Host` impl in `host_impls.rs`. The same
//! holds for `net`, which the transfer bindings generate and the extensions reuse.

use anyhow::Result;
use wasmtime::component::{HasSelf, Linker};

use crate::{PluginManifest, runtime::PluginStoreState};

/// Links `host` and every interface `manifest` grants — `http`, `net`, `cookies`, `captcha` and
/// key derivation — and nothing else. A component that imports an interface it was not granted
/// then fails to instantiate instead of being turned away later, at a call the user is already
/// waiting on. An interface a component's world cannot import costs nothing when linked.
pub(crate) fn link_granted(
    manifest: &PluginManifest,
    linker: &mut Linker<PluginStoreState>,
) -> Result<()> {
    let capabilities = &manifest.capabilities;
    super::rdownloader::plugin::host::add_to_linker::<_, HasSelf<_>>(linker, |state| state)?;
    if capabilities.net_http.is_some() {
        super::rdownloader::plugin::http::add_to_linker::<_, HasSelf<_>>(linker, |state| state)?;
    }
    if capabilities.net_stream.is_some() {
        crate::transfer::rdownloader::plugin::net::add_to_linker::<_, HasSelf<_>>(
            linker,
            |state| state,
        )?;
    }
    // A folder behind a sign-in is reached with the account's cookies, and a share behind a
    // challenge with the captcha broker.
    if capabilities.cookies {
        super::rdownloader::plugin::cookies::add_to_linker::<_, HasSelf<_>>(linker, |state| state)?;
    }
    if capabilities.captcha {
        super::rdownloader::plugin::captcha::add_to_linker::<_, HasSelf<_>>(linker, |state| state)?;
    }
    // Computing over a credential the guest never sees (RD-120-20). Hand-wired rather than
    // generated, because the charge it makes needs the store and not the store's data.
    if capabilities.key_derivation {
        crate::keyderive::add_to_linker(linker)?;
    }
    Ok(())
}
