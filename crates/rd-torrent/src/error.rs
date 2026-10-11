//! The typed reason a torrent control call refused, carried as a cause on the `anyhow` error.
//!
//! The same shape `rd-db`'s `StoreError` established, and for the same reason: every function
//! here returns `anyhow::Result`, so giving them a `thiserror` signature would rewrite call
//! sites outside this crate. What `rd-api` needs is narrower than an error type — the *reason*,
//! so it can pick the documented HTTP status and the stable error code the web client
//! translates — and `anyhow` keeps causes downcastable.
//!
//! The failure this prevents: `rd-api-queue/src/torrent_trackers.rs` chose `429` against `400` by
//! searching the rendered message for the word `"wait"`. Rewording the rate-limit `bail!` in
//! this crate therefore turned a documented `429` into a `400` — nothing to compile-check, and
//! no test that would notice, because the tests assert the code the mapping produced rather
//! than the sentence it matched. A tracker-facing message that merely happened to contain
//! "wait" would have been mis-reported the other way.
//!
//! Messages stay exactly as they were. They are still what a log or an error body shows; they
//! are simply no longer load-bearing.

/// Why a torrent control call refused, at the granularity the REST layer distinguishes.
///
/// Deliberately **not** `#[non_exhaustive]`: adding a reason here should stop the build in
/// `rd-api` until somebody decides what it means over HTTP.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TorrentErrorKind {
    /// The call is allowed, but not this soon. Trackers ban clients that announce in a tight
    /// loop, so the interval is enforced here rather than trusted to the caller.
    RateLimited,
    /// The torrent is not running in the engine right now, so there is no handle to act on.
    NotActive,
    /// Another program holds the fixed listen port, so the session was not started on it
    /// (RD-1250-04). [`TorrentError::port`] names the port.
    PortInUse,
}

/// A refusal from a torrent control call, with the message it has always carried.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TorrentError {
    kind: TorrentErrorKind,
    message: String,
    port: Option<u16>,
}

impl TorrentError {
    /// Builds a refusal with an explicit reason.
    pub fn new(kind: TorrentErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            port: None,
        }
    }

    /// The reason, for a caller deciding how to report it.
    #[must_use]
    pub fn kind(&self) -> TorrentErrorKind {
        self.kind
    }

    /// The port a [`TorrentErrorKind::PortInUse`] refusal is about.
    #[must_use]
    pub fn port(&self) -> Option<u16> {
        self.port
    }

    /// Too soon since the last call of its kind.
    pub fn rate_limited(message: impl Into<String>) -> Self {
        Self::new(TorrentErrorKind::RateLimited, message)
    }

    /// The torrent is not in the running session.
    pub fn not_active(message: impl Into<String>) -> Self {
        Self::new(TorrentErrorKind::NotActive, message)
    }

    /// Another program holds the fixed listen port.
    #[must_use]
    pub fn port_in_use(port: u16) -> Self {
        Self {
            port: Some(port),
            ..Self::new(
                TorrentErrorKind::PortInUse,
                format!("the torrent listen port {port} is in use by another program"),
            )
        }
    }
}

impl std::fmt::Display for TorrentError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for TorrentError {}

/// The reason behind an error, if this crate put one there.
///
/// Walks the whole `anyhow` cause chain, so it finds the reason whether the refusal was built
/// outright (`bail!`) or attached with `.context(...)`.
#[must_use]
pub fn torrent_kind(error: &anyhow::Error) -> Option<TorrentErrorKind> {
    error.downcast_ref::<TorrentError>().map(TorrentError::kind)
}

/// The port another program holds, when that is why the session did not start.
#[must_use]
pub fn port_in_use(error: &anyhow::Error) -> Option<u16> {
    error
        .downcast_ref::<TorrentError>()
        .filter(|refusal| refusal.kind == TorrentErrorKind::PortInUse)
        .and_then(TorrentError::port)
}

#[cfg(test)]
mod tests {
    use anyhow::Context;

    use super::{TorrentError, TorrentErrorKind, port_in_use, torrent_kind};

    #[test]
    fn a_reason_survives_being_bailed_and_being_attached_as_context() {
        let bailed = anyhow::Error::new(TorrentError::rate_limited(
            "wait 12 seconds before announcing again",
        ));
        assert_eq!(torrent_kind(&bailed), Some(TorrentErrorKind::RateLimited));
        // The sentence is unchanged, so logs and error bodies read exactly as before.
        assert_eq!(
            bailed.to_string(),
            "wait 12 seconds before announcing again"
        );

        let attached = None::<()>
            .context(TorrentError::not_active("torrent is not active"))
            .expect_err("error");
        assert_eq!(torrent_kind(&attached), Some(TorrentErrorKind::NotActive));
        assert_eq!(attached.to_string(), "torrent is not active");
    }

    #[test]
    fn a_held_port_is_named_through_a_context() {
        let refused = Err::<(), _>(anyhow::Error::new(TorrentError::port_in_use(51_413)))
            .context("rebuild torrent session")
            .expect_err("error");
        assert_eq!(torrent_kind(&refused), Some(TorrentErrorKind::PortInUse));
        assert_eq!(port_in_use(&refused), Some(51_413));
        let other = anyhow::Error::new(TorrentError::not_active("torrent is not active"));
        assert_eq!(port_in_use(&other), None);
    }

    #[test]
    fn an_untagged_error_has_no_reason() {
        // The engine's own failures stay untyped; the caller keeps whatever it did with them.
        assert_eq!(torrent_kind(&anyhow::anyhow!("pause for reannounce")), None);
    }
}
