//! A name this machine cannot resolve, behind a proxy that is handed checked addresses only
//! (RD-1330-01).
//!
//! Behind a pinned proxy every target name is resolved here: a name only the proxy's network
//! knows is not handed to the proxy unchecked, it fails. [`ResolvedHere`] marks that failure as
//! [`NameUnresolved`], so a caller tells it from a refused address or an unreachable server and
//! names it with the stable code [`CODE_NAME_UNRESOLVED`]. A proxy profile can leave names to
//! the proxy (`resolves_names`); the proxy the environment names cannot (owner, 2026-10-16).

use std::{error::Error as StdError, fmt, io, sync::Arc};

use rd_core::{Failure, FailureKind};

use crate::{HostLookup, LookupFuture};

/// The code of a request behind a proxy whose target name this machine could not resolve.
pub const CODE_NAME_UNRESOLVED: &str = "proxy.name_unresolved";

/// A target name that did not resolve here, behind a proxy that is handed checked addresses.
#[derive(Debug)]
pub struct NameUnresolved {
    host: String,
    source: Option<io::Error>,
}

impl NameUnresolved {
    /// The name that did not resolve.
    #[must_use]
    pub fn host(&self) -> &str {
        &self.host
    }
}

impl fmt::Display for NameUnresolved {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} could not be resolved on this machine, which resolves every name behind the proxy",
            self.host
        )
    }
}

impl StdError for NameUnresolved {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        self.source
            .as_ref()
            .map(|error| error as &(dyn StdError + 'static))
    }
}

/// The lookup a pinned client's guarded resolver asks: the caller's, with a failed or empty
/// answer turned into [`NameUnresolved`], of the original error's kind.
pub(crate) struct ResolvedHere(pub(crate) Arc<dyn HostLookup>);

impl HostLookup for ResolvedHere {
    fn lookup<'a>(&'a self, host: &'a str) -> LookupFuture<'a> {
        Box::pin(async move {
            let (kind, source) = match self.0.lookup(host).await {
                Ok(addresses) if !addresses.is_empty() => return Ok(addresses),
                Ok(_) => (io::ErrorKind::NotFound, None),
                Err(error) => (error.kind(), Some(error)),
            };
            let unresolved = NameUnresolved {
                host: host.to_owned(),
                source,
            };
            Err(io::Error::new(kind, unresolved))
        })
    }
}

/// The [`NameUnresolved`] an error, or anything it was caused by, is or carries. An
/// [`io::Error`] hides its inner error from `source()`, so each one is opened as well.
#[must_use]
pub fn name_unresolved<'a>(error: &'a (dyn StdError + 'static)) -> Option<&'a NameUnresolved> {
    let mut current = Some(error);
    while let Some(error) = current {
        if let Some(unresolved) = error.downcast_ref::<NameUnresolved>() {
            return Some(unresolved);
        }
        let inner = error
            .downcast_ref::<io::Error>()
            .and_then(io::Error::get_ref)
            .and_then(|inner| inner.downcast_ref::<NameUnresolved>());
        if inner.is_some() {
            return inner;
        }
        current = error.source();
    }
    None
}

/// The failure a name that did not resolve behind a proxy is: [`CODE_NAME_UNRESOLVED`] with the
/// host, and transient, since a resolver that is down for a moment answers again. `None` when
/// `error` is no such failure.
#[must_use]
pub fn unresolved_failure(error: &(dyn StdError + 'static)) -> Option<Failure> {
    let unresolved = name_unresolved(error)?;
    Some(
        Failure::coded(
            FailureKind::Transient {
                retry_after_seconds: None,
            },
            CODE_NAME_UNRESOLVED,
            "The name could not be resolved on this machine; behind the proxy the service \
             resolves and checks every address itself",
        )
        .with_param("host", unresolved.host()),
    )
}
