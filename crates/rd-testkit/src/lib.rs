//! What the workspace's tests share (RD-1280-04).
//!
//! - `temp_database` (feature `db`, on by default): a migrated database in a directory the
//!   test owns, usually a `tempfile::TempDir`.
//! - [`serve`]: a router on a free local port; the returned [`TestServer`] stops the server
//!   when it drops, so a test keeps it bound for as long as the server has to answer.
//! - [`lock`], [`respond`], [`payload`]: the helpers the fake object-storage, backup and
//!   transfer servers of several crates each carried a copy of.
//!
//! A dev-dependency only. `rd-db`'s own unit tests cannot use it — their `Database` is a
//! different crate instance from the one this crate links — and share
//! `crate::tests::temp_database` among themselves; its integration tests use this one.

use std::net::SocketAddr;
use std::sync::{Mutex, MutexGuard, PoisonError};

use axum::Router;
use axum::body::Body;
use axum::http::StatusCode;
use axum::response::Response;
use tokio::task::JoinHandle;

#[cfg(feature = "db")]
mod database;
#[cfg(feature = "db")]
pub use database::{DATABASE_FILE, temp_database};

/// A router served on `127.0.0.1` by [`serve`]; aborted when this handle drops.
#[derive(Debug)]
pub struct TestServer {
    addr: SocketAddr,
    task: JoinHandle<()>,
}

impl TestServer {
    /// The address the server listens on.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// `http://<addr><path>`; `path` is empty or starts with `/`.
    pub fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Serves `router` on a free port of `127.0.0.1` until the returned handle drops.
///
/// The router is served as it is: a fake server that takes bodies over axum's default 2 MiB
/// adds `DefaultBodyLimit::disable()` itself.
///
/// # Panics
///
/// When no local port can be bound.
pub async fn serve(router: Router) -> TestServer {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a local port");
    let addr = listener.local_addr().expect("the bound address");
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    TestServer { addr, task }
}

/// Locks `mutex`, taking the value over from a holder that panicked: a fake server's record
/// stays readable after a failed assertion in another task.
pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A response with `status`, `headers` and `body`; an invalid header leaves an empty `200`.
pub fn respond(status: StatusCode, headers: &[(&str, String)], body: impl Into<Body>) -> Response {
    let mut builder = Response::builder().status(status);
    for (name, value) in headers {
        builder = builder.header(*name, value);
    }
    builder
        .body(body.into())
        .unwrap_or_else(|_| Response::new(Body::empty()))
}

/// `length` bytes of a pattern that repeats every 251 bytes, a prime, so no power-of-two part
/// or block boundary lines up with it and a misplaced range shows in the content.
pub fn payload(length: usize) -> Vec<u8> {
    (0..length).map(|index| (index % 251) as u8).collect()
}

#[cfg(test)]
mod tests;
