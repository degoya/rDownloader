//! Mapping of SSH and SFTP failures onto the queue's stable error codes.

use rd_core::{Failure, FailureKind};
use russh_sftp::protocol::StatusCode;

/// Connect, DNS or socket failure before the SSH transport came up.
pub const CONNECT_FAILED: &str = "sftp.connect_failed";
/// Every offered authentication method was rejected.
pub const AUTH_FAILED: &str = "sftp.auth_failed";
/// The stored private key could not be decoded, or its passphrase was wrong.
pub const KEY_INVALID: &str = "sftp.key_invalid";
/// No SSH agent was reachable, or it held no usable identity.
pub const AGENT_UNAVAILABLE: &str = "sftp.agent_unavailable";
/// The remote path does not exist or is not readable.
pub const PATH_NOT_FOUND: &str = "sftp.path_not_found";
/// Size or timestamp of the remote file changed since the partial download was written.
pub const FILE_CHANGED: &str = "sftp.file_changed";
/// The directory holds more entries than one review can carry.
pub const LISTING_TOO_LARGE: &str = "sftp.listing_too_large";
/// A listing entry names a path that would escape the download folder.
pub const UNSAFE_PATH: &str = "sftp.unsafe_path";
/// No stored login matches the server the link points at.
pub const NO_CREDENTIAL: &str = "sftp.no_credential";

/// Turns a transport-level `russh` error into a coded queue failure.
///
/// The error's own text is not carried into the queue: it can contain the remote path and
/// the user name, and the queue message is translated by the client anyway.
#[must_use]
pub fn classify_transport(error: &russh::Error) -> Failure {
    match error {
        russh::Error::IO(io) => Failure::coded(
            FailureKind::Transient {
                retry_after_seconds: None,
            },
            CONNECT_FAILED,
            "The SSH server could not be reached",
        )
        .with_param("reason", io.kind().to_string()),
        russh::Error::NotAuthenticated | russh::Error::NoAuthMethod => Failure::coded(
            FailureKind::AuthRequired,
            AUTH_FAILED,
            "The SSH server rejected every configured authentication method",
        ),
        _ => Failure::coded(
            FailureKind::Transient {
                retry_after_seconds: None,
            },
            CONNECT_FAILED,
            "The SSH connection failed",
        ),
    }
}

/// Turns an SFTP status reply into a coded queue failure.
#[must_use]
pub fn classify_sftp(error: &russh_sftp::client::error::Error) -> Failure {
    use russh_sftp::client::error::Error;
    match error {
        Error::Status(status) => from_status(status.status_code),
        Error::UnexpectedBehavior(_) | Error::UnexpectedPacket => Failure::coded(
            FailureKind::Transient {
                retry_after_seconds: None,
            },
            CONNECT_FAILED,
            "The SFTP server sent an unexpected reply",
        ),
        _ => Failure::coded(
            FailureKind::Transient {
                retry_after_seconds: None,
            },
            CONNECT_FAILED,
            "The SFTP session failed",
        ),
    }
}

fn from_status(status: StatusCode) -> Failure {
    match status {
        StatusCode::NoSuchFile => Failure::coded(
            FailureKind::Permanent,
            PATH_NOT_FOUND,
            "The remote path does not exist",
        ),
        StatusCode::PermissionDenied => Failure::coded(
            FailureKind::AuthRequired,
            PATH_NOT_FOUND,
            "The remote path is not readable with this login",
        ),
        StatusCode::OpUnsupported => Failure::coded(
            FailureKind::Permanent,
            PATH_NOT_FOUND,
            "The SFTP server does not support this operation",
        ),
        _ => Failure::coded(
            FailureKind::Transient {
                retry_after_seconds: None,
            },
            CONNECT_FAILED,
            "The SFTP server refused the request",
        ),
    }
}

/// The failure raised when a resumed transfer no longer matches what was downloaded.
#[must_use]
pub fn file_changed() -> Failure {
    Failure::coded(
        FailureKind::Permanent,
        FILE_CHANGED,
        "The remote file changed since the partial download was written",
    )
}

/// The failure raised when a stored private key cannot be used.
#[must_use]
pub fn key_invalid() -> Failure {
    Failure::coded(
        FailureKind::AuthRequired,
        KEY_INVALID,
        "The stored private key could not be read, or its passphrase is wrong",
    )
}

/// The failure raised when the SSH agent cannot supply an identity.
#[must_use]
pub fn agent_unavailable() -> Failure {
    Failure::coded(
        FailureKind::AuthRequired,
        AGENT_UNAVAILABLE,
        "No SSH agent is reachable, or it holds no usable identity",
    )
}

/// The failure raised when no stored login covers the server.
#[must_use]
pub fn no_credential(host: &str) -> Failure {
    Failure::coded(
        FailureKind::AuthRequired,
        NO_CREDENTIAL,
        "No stored login matches this SFTP server",
    )
    .with_param("host", host)
}

#[cfg(test)]
mod tests {
    use rd_core::FailureKind;
    use russh_sftp::protocol::StatusCode;

    use super::{AUTH_FAILED, PATH_NOT_FOUND, classify_transport, from_status};

    #[test]
    fn a_rejected_login_stops_retrying() {
        let failure = classify_transport(&russh::Error::NotAuthenticated);
        assert_eq!(failure.code.as_deref(), Some(AUTH_FAILED));
        assert_eq!(failure.category, FailureKind::AuthRequired);
        assert!(!failure.category.is_retryable());
    }

    #[test]
    fn a_socket_failure_is_retried() {
        let io = std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "refused");
        let failure = classify_transport(&russh::Error::IO(io));
        assert!(failure.category.is_retryable());
        // The reason is a category name, never the server's own text.
        assert!(failure.params.contains_key("reason"));
    }

    #[test]
    fn a_missing_remote_path_is_permanent() {
        let failure = from_status(StatusCode::NoSuchFile);
        assert_eq!(failure.code.as_deref(), Some(PATH_NOT_FOUND));
        assert!(!failure.category.is_retryable());
    }

    #[test]
    fn a_permission_error_asks_for_a_different_login() {
        let failure = from_status(StatusCode::PermissionDenied);
        assert_eq!(failure.category, FailureKind::AuthRequired);
    }
}
