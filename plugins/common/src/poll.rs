//! Driving an `async fn` to completion where there is no runtime.
//!
//! The WebAssembly guest is single-threaded and every host call it makes is synchronous: the
//! canonical ABI blocks until the host answers. So the futures its adapter produces are ready
//! the moment they are created, and the whole call graph above them completes on the first
//! poll.
//!
//! That is what makes one shared piece of logic possible at all. It can be written as ordinary
//! `async` code — which the native build runs on Tokio, awaiting real I/O — and the guest runs
//! the same code by polling it once. A future that is *not* ready here would mean the adapter
//! started something it cannot finish, so the panic is the honest outcome: there is no thread
//! to wait on and nothing that will ever wake it.

use std::{
    future::Future,
    pin::pin,
    task::{Context, Poll, Waker},
};

/// Runs `future` to completion, assuming it never yields.
///
/// # Panics
///
/// If the future is not ready on the first poll. See the module documentation: in a guest that
/// is a bug in the adapter, not a condition to handle.
pub fn block_on<F: Future>(future: F) -> F::Output {
    let waker = Waker::noop();
    let mut context = Context::from_waker(waker);
    match pin!(future).poll(&mut context) {
        Poll::Ready(value) => value,
        Poll::Pending => {
            panic!("a plugin host call yielded, but the guest has nothing to wait with")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::block_on;

    #[test]
    fn a_chain_of_ready_futures_completes_in_one_poll() {
        async fn inner(value: u32) -> u32 {
            value + 1
        }
        async fn outer() -> u32 {
            let first = inner(1).await;
            let second = inner(first).await;
            inner(second).await
        }
        assert_eq!(block_on(outer()), 4);
    }

    #[test]
    #[should_panic(expected = "nothing to wait with")]
    fn a_future_that_yields_is_a_bug_rather_than_a_wait() {
        assert_eq!(block_on(std::future::pending::<u32>()), 0);
    }
}
