//! The target of each connection a guarded client makes, for the checks that need its port
//! (RD-1420-01).
//!
//! reqwest's connector layers cannot read the destination, and its resolver is handed a name
//! without a port. What reqwest does hand over is the target URL, to each proxy it asks inside
//! every connect call, in order. [`recorder`] is such a proxy: it notes the target and declines,
//! so the proxy behind it — or none — is used exactly as before. [`noting`] arms the note around
//! a connect call, and [`LocalPortLayer`] uses it: on a port the address rule refuses on this
//! machine only (a reverse proxy's in front of the service), it runs the connection inside
//! [`this_machine_refused`], which the guarded resolver reads while resolving the target, so an
//! answer naming this machine refuses the name — checked on the addresses that are dialled.

use std::{
    cell::RefCell,
    error::Error as StdError,
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

use reqwest::{ClientBuilder, Proxy};
use url::Url;

use crate::AddressPolicy;

type BoxError = Box<dyn StdError + Send + Sync>;

thread_local! {
    /// While a connect call runs on this thread under [`noting`]: `Some`, holding the target the
    /// recorder was asked for once it has been.
    static ASKED: RefCell<Option<Option<Url>>> = const { RefCell::new(None) };
}

tokio::task_local! {
    /// Whether the connection being made may not reach this machine.
    static THIS_MACHINE_REFUSED: bool;
}

/// The proxy that notes each connect call's target and declines it, for the next proxy.
pub(crate) fn recorder() -> Proxy {
    Proxy::custom(|target: &Url| {
        ASKED.with(|asked| {
            if let Some(slot) = asked.borrow_mut().as_mut() {
                *slot = Some(target.clone());
            }
        });
        None::<Url>
    })
}

/// Runs `call` — a connect call — and returns what it returned with the target the recorder
/// noted during it. reqwest asks its proxies synchronously inside the call; the connection
/// itself is made only when the returned future is polled.
pub(crate) fn noting<T>(call: impl FnOnce() -> T) -> (T, Option<Url>) {
    let previous = ASKED.with(|asked| asked.replace(Some(None)));
    let result = call();
    let target = ASKED.with(|asked| asked.replace(previous)).flatten();
    (result, target)
}

/// Whether the connection being made — the resolver asks — may not reach this machine.
pub(crate) fn this_machine_refused() -> bool {
    THIS_MACHINE_REFUSED
        .try_with(|refused| *refused)
        .unwrap_or(false)
}

/// Gives a guarded client the check of [`AddressPolicy::refusing_redirects_on_this_machine`]
/// at the connection: nothing when the rule names no such port.
pub(crate) fn refusing_this_machine(
    builder: ClientBuilder,
    policy: &AddressPolicy,
) -> ClientBuilder {
    if !policy.has_local_refused_ports() {
        return builder;
    }
    builder
        .proxy(recorder())
        .connector_layer(LocalPortLayer(policy.clone()))
}

/// The connector layer that marks a connection on a port refused on this machine.
#[derive(Clone)]
pub(crate) struct LocalPortLayer(pub(crate) AddressPolicy);

impl<S> tower::Layer<S> for LocalPortLayer {
    type Service = LocalPortService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        LocalPortService {
            inner,
            policy: self.0.clone(),
        }
    }
}

#[derive(Clone)]
pub(crate) struct LocalPortService<S> {
    inner: S,
    policy: AddressPolicy,
}

impl<S, R> tower::Service<R> for LocalPortService<S>
where
    S: tower::Service<R, Error = BoxError>,
    S::Future: Send + 'static,
    S::Response: Send + 'static,
{
    type Response = S::Response;
    type Error = BoxError;
    type Future = Pin<Box<dyn Future<Output = Result<S::Response, BoxError>> + Send>>;

    fn poll_ready(&mut self, context: &mut Context<'_>) -> Poll<Result<(), BoxError>> {
        self.inner.poll_ready(context)
    }

    fn call(&mut self, request: R) -> Self::Future {
        let (connecting, target) = noting(|| self.inner.call(request));
        // A call whose target went unnoted is held to the stricter rule, never let through.
        let refused = target.is_none_or(|target| {
            self.policy
                .refuses_this_machine_on(target.port_or_known_default())
        });
        Box::pin(THIS_MACHINE_REFUSED.scope(refused, connecting))
    }
}

#[cfg(test)]
#[path = "connect_target_tests.rs"]
mod tests;
