//! The typed reason an NNTP exchange was refused, carried as a cause on the `anyhow` error.
//!
//! The same shape `rd-db`'s `StoreError` established: every function here returns
//! `anyhow::Result`, so a `thiserror` signature would rewrite call sites in four crates. What
//! `rd-api` needs is narrower — the server's *status*, so it can pick the documented HTTP
//! answer and the stable error code the web client translates — and `anyhow` keeps causes
//! downcastable.
//!
//! The failure this prevents: `rd-api/src/usenet_handlers.rs` recognised a refused connection
//! by searching the rendered message for the characters `"502"`. That is a substring of this
//! crate's own sentence, so rewording it would have turned a documented, specific answer into
//! a generic one — and, the other way round, any server line that merely contained `502`
//! anywhere (a message-id, a group name, a byte count) matched.
//!
//! The message stays exactly as it was, down to the quoting: it is still what a log and an
//! error body show, and it is simply no longer load-bearing.

/// A status line the server answered with, where another status was required.
///
/// The code is parsed once, here, so no caller has to know that an NNTP status is the first
/// three characters of the line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NntpStatusError {
    code: u16,
    line: String,
}

impl NntpStatusError {
    /// Builds a refusal from the raw status line and the code already parsed out of it.
    #[must_use]
    pub fn new(code: u16, line: impl Into<String>) -> Self {
        Self {
            code,
            line: line.into(),
        }
    }

    /// The three-digit NNTP status, e.g. `502` for "service unavailable".
    #[must_use]
    pub fn code(&self) -> u16 {
        self.code
    }

    /// What the server wrote after the status, trimmed.
    ///
    /// This is the *remote* server's own prose and stays a text match for any caller that
    /// needs it — a provider's wording is not ours to make typed. What this type removes is
    /// the match on **our** sentence, which is the part that could be reworded by a commit in
    /// this crate.
    #[must_use]
    pub fn text(&self) -> &str {
        self.line.get(3..).unwrap_or_default().trim()
    }

    /// The status line exactly as the server sent it.
    #[must_use]
    pub fn line(&self) -> &str {
        &self.line
    }
}

impl std::fmt::Display for NntpStatusError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Byte-identical to the `bail!` this replaced, quoting included.
        write!(formatter, "NNTP server returned {:?}", self.line)
    }
}

impl std::error::Error for NntpStatusError {}

/// The status behind an error, if the NNTP client put one there.
///
/// Walks the whole `anyhow` cause chain, so it survives the `.context(...)` the connect path
/// adds on the way out.
#[must_use]
pub fn nntp_status(error: &anyhow::Error) -> Option<&NntpStatusError> {
    error.downcast_ref::<NntpStatusError>()
}

#[cfg(test)]
mod tests {
    use anyhow::Context;

    use super::{NntpStatusError, nntp_status};

    #[test]
    fn a_status_survives_context_and_renders_as_it_always_did() {
        let error = anyhow::Error::new(NntpStatusError::new(502, "502 Access denied to your node"));
        assert_eq!(
            error.to_string(),
            r#"NNTP server returned "502 Access denied to your node""#
        );
        let wrapped = Err::<(), _>(error)
            .context("NNTP greeting")
            .expect_err("error");
        let status = nntp_status(&wrapped).expect("status");
        assert_eq!(status.code(), 502);
        assert_eq!(status.text(), "Access denied to your node");
    }

    #[test]
    fn an_untagged_error_has_no_status() {
        // A transport failure carries no status at all, and must not be read as one.
        assert!(nntp_status(&anyhow::anyhow!("NNTP TLS handshake")).is_none());
    }
}
