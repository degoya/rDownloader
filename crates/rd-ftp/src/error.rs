//! Mapping of FTP failures onto the queue's stable error codes.
//!
//! Every message is built here rather than by formatting the library error into the queue,
//! because an `FtpError` carries the server's reply verbatim and those replies routinely
//! echo the user name and the full remote path.

use rd_core::{Failure, FailureKind};
use suppaftp::{FtpError, Status};

/// Connect, DNS or socket failure before anything was negotiated.
pub const CONNECT_FAILED: &str = "ftp.connect_failed";
/// The server rejected the login.
pub const AUTH_FAILED: &str = "ftp.auth_failed";
/// The credential demands TLS but the server does not offer `AUTH TLS`.
pub const TLS_REQUIRED: &str = "ftp.tls_required";
/// TLS negotiation or certificate validation failed.
pub const TLS_HANDSHAKE_FAILED: &str = "ftp.tls_handshake_failed";
/// The remote path does not exist or is not readable.
pub const PATH_NOT_FOUND: &str = "ftp.path_not_found";
/// Size or timestamp of the remote file changed since the partial download was written.
pub const FILE_CHANGED: &str = "ftp.file_changed";
/// The server refused `REST`, so an interrupted transfer cannot be continued.
pub const RESUME_UNSUPPORTED: &str = "ftp.resume_unsupported";
/// The directory holds more entries than one review can carry.
pub const LISTING_TOO_LARGE: &str = "ftp.listing_too_large";
/// A listing entry names a path that would escape the download folder.
pub const UNSAFE_PATH: &str = "ftp.unsafe_path";
/// No stored login matches the server the link points at.
pub const NO_CREDENTIAL: &str = "ftp.no_credential";

/// Turns a library error into a coded queue failure.
///
/// Status codes in the `4xx` range are transient by definition of RFC 959 ("try again"),
/// `5xx` are permanent, and the two login-specific codes become `AuthRequired` so the queue
/// stops retrying a password the server keeps rejecting.
#[must_use]
pub fn classify(error: &FtpError) -> Failure {
    match error {
        FtpError::ConnectionError(io) => Failure::coded(
            FailureKind::Transient {
                retry_after_seconds: None,
            },
            CONNECT_FAILED,
            "The FTP server could not be reached",
        )
        .with_param("reason", io.kind().to_string()),
        FtpError::SecureError(_) => Failure::coded(
            FailureKind::Permanent,
            TLS_HANDSHAKE_FAILED,
            "The TLS handshake with the FTP server failed",
        ),
        FtpError::UnexpectedResponse(response) => from_status(response.status),
        FtpError::BadResponse => Failure::coded(
            FailureKind::Transient {
                retry_after_seconds: None,
            },
            CONNECT_FAILED,
            "The FTP server sent a malformed reply",
        ),
        FtpError::InvalidAddress(_) => Failure::coded(
            FailureKind::Permanent,
            CONNECT_FAILED,
            "The FTP server address is not valid",
        ),
        // A leaked data connection is our bug, not the server's; retrying on a fresh
        // control connection is the correct recovery.
        FtpError::DataConnectionAlreadyOpen => Failure::coded(
            FailureKind::Transient {
                retry_after_seconds: None,
            },
            CONNECT_FAILED,
            "The FTP data connection was left open",
        ),
    }
}

fn from_status(status: Status) -> Failure {
    let code = status as u32;
    match code {
        // 530 not logged in, 532 need account for storing files.
        530 | 532 => Failure::coded(
            FailureKind::AuthRequired,
            AUTH_FAILED,
            "The FTP server rejected the login",
        ),
        // 550 covers "no such file", "permission denied" and "not a plain file" alike; the
        // server does not distinguish them and neither can we.
        550 => Failure::coded(
            FailureKind::Permanent,
            PATH_NOT_FOUND,
            "The remote path does not exist or is not readable",
        ),
        // 421 is the standard "too many connections, try later" reply.
        421 => Failure::coded(
            FailureKind::RateLimited {
                retry_after_seconds: None,
            },
            CONNECT_FAILED,
            "The FTP server is not accepting more connections right now",
        ),
        400..=499 => Failure::coded(
            FailureKind::Transient {
                retry_after_seconds: None,
            },
            CONNECT_FAILED,
            "The FTP server reported a temporary problem",
        )
        .with_param("status", code),
        _ => Failure::coded(
            FailureKind::Permanent,
            PATH_NOT_FOUND,
            "The FTP server refused the request",
        )
        .with_param("status", code),
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

/// The failure raised when the server cannot continue an interrupted transfer.
#[must_use]
pub fn resume_unsupported() -> Failure {
    Failure::coded(
        FailureKind::Permanent,
        RESUME_UNSUPPORTED,
        "The FTP server does not support resuming a transfer",
    )
}

/// The failure raised when no stored login covers the server.
#[must_use]
pub fn no_credential(host: &str) -> Failure {
    Failure::coded(
        FailureKind::AuthRequired,
        NO_CREDENTIAL,
        "No stored login matches this FTP server",
    )
    .with_param("host", host)
}

#[cfg(test)]
mod tests {
    use rd_core::FailureKind;
    use suppaftp::{FtpError, Status, types::Response};

    use super::{AUTH_FAILED, CONNECT_FAILED, PATH_NOT_FOUND, classify};

    fn response(status: Status, body: &str) -> FtpError {
        FtpError::UnexpectedResponse(Response::new(status, body.as_bytes().to_vec()))
    }

    #[test]
    fn a_rejected_login_stops_retrying() {
        let failure = classify(&response(Status::NotLoggedIn, "530 Login incorrect"));
        assert_eq!(failure.code.as_deref(), Some(AUTH_FAILED));
        assert_eq!(failure.category, FailureKind::AuthRequired);
        assert!(!failure.category.is_retryable());
    }

    #[test]
    fn a_busy_server_is_retried_later() {
        let failure = classify(&response(Status::NotAvailable, "421 Too many users"));
        assert_eq!(failure.code.as_deref(), Some(CONNECT_FAILED));
        assert!(failure.category.is_retryable());
    }

    #[test]
    fn a_missing_file_is_permanent() {
        let failure = classify(&response(Status::FileUnavailable, "550 Not found"));
        assert_eq!(failure.code.as_deref(), Some(PATH_NOT_FOUND));
        assert!(!failure.category.is_retryable());
    }

    #[test]
    fn the_server_reply_is_never_echoed_into_the_queue() {
        // Replies routinely quote the user name and the full remote path; the queue text
        // has to be ours, not the server's.
        let failure = classify(&response(
            Status::FileUnavailable,
            "550 /home/bob/secret-folder/file.bin: Permission denied",
        ));
        assert!(!failure.message.contains("bob"));
        assert!(!failure.message.contains("secret-folder"));
    }
}
