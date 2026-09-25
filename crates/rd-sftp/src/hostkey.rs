//! Server host-key verification.
//!
//! An SSH client that accepts every host key is indistinguishable from one talking to an
//! attacker, and one that accepts none is unusable. The middle ground is a trust store:
//! a key is confirmed once by a person, and any later change is surfaced rather than
//! resolved automatically.
//!
//! The decision deliberately does **not** live in the `russh` handler callback. Returning
//! `false` there produces an opaque protocol error that cannot say *why* the connection was
//! refused, so the handler records what it saw and the caller turns that into a coded
//! failure carrying the fingerprint the user has to confirm.

use std::sync::{Arc, Mutex};

use rd_core::{Failure, FailureKind};
use rd_db::HostKeyVerdict;
use russh::keys::PublicKeyOrCertificate;

/// Connection refused because the server is not in the trust store yet.
pub const HOST_KEY_UNKNOWN: &str = "sftp.host_key_unknown";
/// Connection refused because the server presented a different key than the stored one.
pub const HOST_KEY_CHANGED: &str = "sftp.host_key_changed";
/// The server offered a certificate; certificate-based host trust is not implemented.
pub const HOST_KEY_UNSUPPORTED: &str = "sftp.host_key_unsupported";

/// What the handler observed during key exchange.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OfferedKey {
    /// Algorithm name as SSH spells it (`ssh-ed25519`, `rsa-sha2-512`, …).
    pub algorithm: String,
    /// `SHA256:<base64>` exactly as OpenSSH prints it, so it can be compared by eye with
    /// what `ssh-keyscan` or the server operator reports.
    pub fingerprint: String,
}

/// Why a host key was rejected.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Rejection {
    Unknown(OfferedKey),
    Changed {
        offered: OfferedKey,
        stored_fingerprint: String,
    },
    /// A certificate, which this trust store has no way to pin.
    Unsupported,
}

impl Rejection {
    /// The coded failure shown to the user, carrying what they need to decide.
    ///
    /// The fingerprint is not a secret — it is the public half's digest, and it is exactly
    /// the value a person has to compare against the server's — so it travels as a
    /// parameter rather than being redacted.
    #[must_use]
    pub fn into_failure(self, host: &str, port: u16) -> Failure {
        match self {
            Self::Unknown(key) => Failure::coded(
                FailureKind::AuthRequired,
                HOST_KEY_UNKNOWN,
                "This SSH server is not trusted yet; confirm its fingerprint to continue",
            )
            .with_param("host", host)
            .with_param("port", port)
            .with_param("algorithm", key.algorithm)
            .with_param("fingerprint", key.fingerprint),
            Self::Changed {
                offered,
                stored_fingerprint,
            } => Failure::coded(
                FailureKind::AuthRequired,
                HOST_KEY_CHANGED,
                "The SSH server presented a different host key than the one confirmed earlier",
            )
            .with_param("host", host)
            .with_param("port", port)
            .with_param("algorithm", offered.algorithm)
            .with_param("fingerprint", offered.fingerprint)
            .with_param("stored_fingerprint", stored_fingerprint),
            Self::Unsupported => Failure::coded(
                FailureKind::Permanent,
                HOST_KEY_UNSUPPORTED,
                "The SSH server authenticates with a certificate, which is not supported",
            )
            .with_param("host", host),
        }
    }
}

/// Shared slot the handler writes its observation into.
pub type Observed = Arc<Mutex<Option<Result<OfferedKey, Rejection>>>>;

/// Reads the algorithm and OpenSSH-style fingerprint off an offered key.
///
/// A certificate is refused rather than reduced to its signing key: pinning the CA is a
/// different trust model, and silently pinning the leaf would accept every future
/// certificate the same CA issues.
pub fn describe(offered: &PublicKeyOrCertificate) -> Result<OfferedKey, Rejection> {
    match offered {
        PublicKeyOrCertificate::PublicKey { key, .. } => Ok(OfferedKey {
            algorithm: key.algorithm().as_str().to_owned(),
            fingerprint: key.fingerprint(russh::keys::HashAlg::Sha256).to_string(),
        }),
        PublicKeyOrCertificate::Certificate(_) => Err(Rejection::Unsupported),
    }
}

/// Turns a trust-store verdict into the accept/reject decision.
///
/// `auto_trust` is the escape hatch for unattended setups and is off by default; it only
/// ever covers a *first* sighting. A key that changed is never accepted automatically,
/// because a rebuilt server and an interception look identical from here.
pub fn decide(
    verdict: &HostKeyVerdict,
    offered: OfferedKey,
    auto_trust: bool,
) -> Result<OfferedKey, Rejection> {
    match verdict {
        HostKeyVerdict::Trusted => Ok(offered),
        HostKeyVerdict::Unknown if auto_trust => Ok(offered),
        HostKeyVerdict::Unknown => Err(Rejection::Unknown(offered)),
        HostKeyVerdict::Changed { stored_fingerprint } => Err(Rejection::Changed {
            offered,
            stored_fingerprint: stored_fingerprint.clone(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use rd_db::HostKeyVerdict;

    use super::{OfferedKey, Rejection, decide};

    fn key() -> OfferedKey {
        OfferedKey {
            algorithm: "ssh-ed25519".to_owned(),
            fingerprint: "SHA256:AAAABBBBCCCC".to_owned(),
        }
    }

    #[test]
    fn a_trusted_key_connects() {
        assert!(decide(&HostKeyVerdict::Trusted, key(), false).is_ok());
    }

    #[test]
    fn an_unknown_key_blocks_and_names_its_fingerprint() {
        let rejected = decide(&HostKeyVerdict::Unknown, key(), false).expect_err("blocked");
        assert_eq!(rejected, Rejection::Unknown(key()));
        let failure = rejected.into_failure("box.example", 22);
        assert_eq!(failure.code.as_deref(), Some(super::HOST_KEY_UNKNOWN));
        // The user has to be able to read the fingerprint to confirm it.
        assert_eq!(
            failure.params.get("fingerprint").map(String::as_str),
            Some("SHA256:AAAABBBBCCCC")
        );
    }

    #[test]
    fn a_changed_key_is_never_accepted_automatically() {
        // The bug this locks in: treating a changed key like a first sighting turns an
        // active interception into a silent success.
        let verdict = HostKeyVerdict::Changed {
            stored_fingerprint: "SHA256:OLDOLDOLD".to_owned(),
        };
        for auto_trust in [false, true] {
            let rejected = decide(&verdict, key(), auto_trust).expect_err("blocked");
            assert!(matches!(rejected, Rejection::Changed { .. }));
        }
        let failure = decide(&verdict, key(), true)
            .expect_err("blocked")
            .into_failure("box.example", 22);
        assert_eq!(failure.code.as_deref(), Some(super::HOST_KEY_CHANGED));
        // Both fingerprints are shown so the change itself can be judged.
        assert_eq!(
            failure.params.get("stored_fingerprint").map(String::as_str),
            Some("SHA256:OLDOLDOLD")
        );
        assert_eq!(
            failure.params.get("fingerprint").map(String::as_str),
            Some("SHA256:AAAABBBBCCCC")
        );
    }

    #[test]
    fn auto_trust_only_covers_a_first_sighting() {
        assert!(decide(&HostKeyVerdict::Unknown, key(), true).is_ok());
    }

    #[test]
    fn a_blocked_host_key_is_not_retried_by_the_queue() {
        // Retrying cannot help: nothing changes until a person confirms the key.
        let failure = Rejection::Unknown(key()).into_failure("box.example", 22);
        assert!(!failure.category.is_retryable());
    }
}
