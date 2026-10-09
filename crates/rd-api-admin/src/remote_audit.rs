//! The server a remote login's credential belongs to, and the audit record of every change to a
//! login (RD-1200-06).
//!
//! A password or a private key is typed for one server. An update that names another protocol,
//! host or port gets none of the stored credentials and has to carry them again: a token that
//! can never read them must not point a login at a host of its own and receive them there.

use rd_core::{RemoteAuthMode, RemoteCredential, RemoteProtocol};

use crate::{
    ApiError, AppState,
    audit::{AuditContext, AuditEvent},
};

/// Whether an update still names the server the stored credentials were typed for. A changed
/// protocol counts too: `ftp` sends the password in the clear where `ftps` did not.
pub(super) fn same_server(
    stored: &RemoteCredential,
    protocol: RemoteProtocol,
    host: &str,
    port: u16,
) -> bool {
    stored.protocol == protocol && stored.host == host && stored.port == port
}

/// Whether a moved login would lose the credential its mode signs in with because the update
/// does not carry it again.
pub(super) fn drops_credential(
    stored: &RemoteCredential,
    secret_given: bool,
    key_given: bool,
) -> bool {
    match stored.auth_mode {
        RemoteAuthMode::Password => stored.secret_ref.is_some() && !secret_given,
        RemoteAuthMode::PrivateKey => stored.key_ref.is_some() && !key_given,
        RemoteAuthMode::Anonymous | RemoteAuthMode::Agent => false,
    }
}

pub(super) fn secret_host_changed() -> ApiError {
    ApiError::bad_request(
        "remote.secret_host_changed",
        "The server changed: enter the password or the key again, it is not sent to the new host",
    )
}

/// The names of the fields an update changed, the credentials among them; never a value.
pub(super) fn changed_fields(before: &RemoteCredential, after: &RemoteCredential) -> String {
    [
        ("name", before.name != after.name),
        ("protocol", before.protocol != after.protocol),
        ("host", before.host != after.host),
        ("port", before.port != after.port),
        ("username", before.username != after.username),
        ("auth_mode", before.auth_mode != after.auth_mode),
        ("passive", before.passive != after.passive),
        ("enabled", before.enabled != after.enabled),
        ("password", before.secret_ref != after.secret_ref),
        ("private_key", before.key_ref != after.key_ref),
        ("passphrase", before.passphrase_ref != after.passphrase_ref),
    ]
    .into_iter()
    .filter_map(|(name, changed)| changed.then_some(name))
    .collect::<Vec<_>>()
    .join(" ")
}

/// One audit record per login change: who, which login, what kind of change, the names of the
/// fields an update changed, and the server the login now signs in to. Never a credential.
pub(super) async fn record(
    state: &AppState,
    audit: &AuditContext,
    credential: &RemoteCredential,
    change: &str,
    fields: Option<&str>,
) {
    let server = format!(
        "{}://{}:{}",
        credential.protocol.as_str(),
        credential.host,
        credential.port
    );
    let mut event = AuditEvent::success(rd_core::AuditAction::RemoteCredentialChanged)
        .by(audit)
        .target("remote_credential", credential.id)
        .named(credential.name.clone())
        .detail("change", change)
        .detail("server", server);
    if let Some(fields) = fields {
        event = event.detail("fields", fields);
    }
    crate::audit::record(state, event).await;
}
