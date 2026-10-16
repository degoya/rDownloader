//! A proxy that refuses the tunnel to a checked address, told to the caller by its status
//! (RD-1320-07).
//!
//! The bridge (`pinned_proxy_bridge.rs`) learns the proxy's answer, but SOCKS5 carries only a
//! handful of reply codes back to reqwest, and none of them a status. So each connection gets a
//! ticket: [`TicketLayer`] draws one around the connect call and hands it to the bridge's
//! proxy function, which reqwest calls synchronously inside that same call, as the SOCKS user
//! name; the bridge files a refusal under it, and the layer turns the failed connect into a
//! [`TunnelRefused`] with the status. [`tunnel_failure`] makes that the stable code
//! [`CODE_TUNNEL_REFUSED`] for every caller.

use std::{
    cell::Cell,
    collections::HashMap,
    error::Error as StdError,
    fmt,
    future::Future,
    pin::Pin,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicU64, Ordering},
    },
    task::{Context, Poll},
};

use rd_core::{Failure, FailureKind};

/// The code of a request the proxy refused to open a tunnel for.
pub const CODE_TUNNEL_REFUSED: &str = "proxy.tunnel_refused";

/// Refusals a bridge keeps for tickets nobody collected (a connect that was given up on).
const MAX_UNCOLLECTED: usize = 64;

type BoxError = Box<dyn StdError + Send + Sync>;

thread_local! {
    /// The ticket of the connect call running on this thread, for the bridge's proxy function.
    static CURRENT: Cell<Option<u64>> = const { Cell::new(None) };
}

/// The ticket of the connect call reqwest is making right now on this thread, if any.
pub(crate) fn current_ticket() -> Option<u64> {
    CURRENT.with(Cell::get)
}

/// The tickets of one bridge and the refusals filed under them.
#[derive(Default)]
pub(crate) struct Tickets {
    next: AtomicU64,
    refused: Mutex<HashMap<u64, u16>>,
}

impl Tickets {
    fn draw(&self) -> u64 {
        self.next.fetch_add(1, Ordering::Relaxed)
    }

    /// Files the proxy's `status` for the connection that carried `ticket`.
    pub(crate) fn refuse(&self, ticket: u64, status: u16) {
        let mut refused = self.refused.lock().unwrap_or_else(PoisonError::into_inner);
        if refused.len() >= MAX_UNCOLLECTED {
            refused.clear();
        }
        refused.insert(ticket, status);
    }

    fn collect(&self, ticket: u64) -> Option<u16> {
        self.refused
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&ticket)
    }
}

/// The connector layer of a client behind a bridge.
#[derive(Clone)]
pub(crate) struct TicketLayer(pub(crate) Arc<Tickets>);

impl<S> tower::Layer<S> for TicketLayer {
    type Service = TicketService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        TicketService {
            inner,
            tickets: Arc::clone(&self.0),
        }
    }
}

#[derive(Clone)]
pub(crate) struct TicketService<S> {
    inner: S,
    tickets: Arc<Tickets>,
}

impl<S, R> tower::Service<R> for TicketService<S>
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
        let ticket = self.tickets.draw();
        let previous = CURRENT.with(|current| current.replace(Some(ticket)));
        let connecting = self.inner.call(request);
        CURRENT.with(|current| current.set(previous));
        let tickets = Arc::clone(&self.tickets);
        Box::pin(async move {
            let connected = connecting.await;
            match (connected, tickets.collect(ticket)) {
                (Err(source), Some(status)) => Err(TunnelRefused { status, source }.into()),
                (connected, _) => connected,
            }
        })
    }
}

/// The proxy refused the tunnel to a checked address with `status`.
#[derive(Debug)]
pub struct TunnelRefused {
    status: u16,
    source: BoxError,
}

impl TunnelRefused {
    /// The status the proxy answered the `CONNECT` with.
    #[must_use]
    pub fn status(&self) -> u16 {
        self.status
    }
}

impl fmt::Display for TunnelRefused {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "the proxy refused the tunnel with status {}",
            self.status
        )
    }
}

impl StdError for TunnelRefused {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        Some(self.source.as_ref())
    }
}

/// The status of the tunnel refusal an error, or anything it was caused by, is.
#[must_use]
pub fn tunnel_refusal(error: &(dyn StdError + 'static)) -> Option<u16> {
    let mut current = Some(error);
    while let Some(error) = current {
        if let Some(refused) = error.downcast_ref::<TunnelRefused>() {
            return Some(refused.status);
        }
        current = error.source();
    }
    None
}

/// The failure a refused tunnel is: [`CODE_TUNNEL_REFUSED`] with the proxy's `status`.
/// Permanent for a `4xx` — the proxy forbids it, or wants a login it was not given — and
/// transient otherwise. `None` when `error` is no such refusal.
#[must_use]
pub fn tunnel_failure(error: &(dyn StdError + 'static)) -> Option<Failure> {
    let status = tunnel_refusal(error)?;
    let kind = if (400..500).contains(&status) {
        FailureKind::Permanent
    } else {
        FailureKind::Transient {
            retry_after_seconds: None,
        }
    };
    Some(
        Failure::coded(
            kind,
            CODE_TUNNEL_REFUSED,
            format!("The proxy refused the connection with status {status}"),
        )
        .with_param("status", status),
    )
}
