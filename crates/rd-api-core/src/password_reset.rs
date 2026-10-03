//! Setting a new administrator password without the current one, from the machine the service
//! runs on (RD-190-24): `rdownloader auth reset-password`.
//!
//! The password change (RD-120-22) needs the current password, and nothing reachable over the
//! network may do without it. Whoever can write the data directory, though, already holds the
//! installation: the database beside it carries the password hash, and a self-hosted program
//! is recovered there — SABnzbd and qBittorrent by editing their configuration file. This makes
//! that one step a checked, recorded path instead of an edit of the SQLite file. The trust
//! boundary is the data directory: the command reaches a running service only with the local
//! control token (`crate::local_control`, readable by the service account alone, honoured only
//! from this machine and without a forwarding header), and a stopped one only by opening its
//! database.
//!
//! Both ways run [`reset`]: the new password under the ordinary policy, every session ended,
//! the authenticator app (TOTP) removed only when asked — a lost phone is a separate case from a
//! forgotten password — and the passkeys, the API tokens and the password sign-in switch of the
//! identity provider left as they are. The running service also clears the sign-in limiter, which
//! the owner has usually tripped by the time they reach for this; a stopped one starts with an
//! empty limiter anyway.
//!
//! There is no "must change the password" state: the printed password is the password from then
//! on, and the command says to replace it in *Settings → Security*.

use crate::{ApiError, audit, auth, oidc_client};

// The command line judges a password before it travels, and its tests seed and read one back.
pub use crate::auth::{admin_password_matches, store_admin_password, validate_password};

/// What one reset did, for the audit record and the command's report.
#[derive(Debug, Default, Eq, PartialEq)]
pub struct Reset {
    /// How many sessions it ended.
    pub sessions_ended: u64,
    /// The vault references of the authenticator apps it removed; the caller deletes the
    /// material behind them. Empty unless `disable_totp` was asked.
    pub removed_material: Vec<String>,
    /// Whether the password sign-in is switched off for the identity provider (RD-190-15). The
    /// reset leaves it so; `rdownloader auth password-login on` turns it back on.
    pub password_login_off: bool,
}

/// Which way the reset reached the service, as the audit record names it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Path {
    /// The running service, over the local control token.
    Service,
    /// The database of a stopped service.
    Database,
}

impl Path {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Service => "service",
            Self::Database => "database",
        }
    }
}

/// A password for the owner to sign in with once and replace: 32 random bytes, base64url —
/// the generator behind the provider sign-in's nonces, far beyond the policy's minimum.
#[must_use]
pub fn one_time_password() -> String {
    rd_authn::oidc::random_value()
}

/// Sets `password` as the administrator password in `database` and ends every session; with
/// `disable_totp`, removes the authenticator apps and the recovery codes too.
///
/// # Errors
///
/// `auth.password_reset_setup_pending` when no password was ever set — the first one belongs
/// to the setup in the web interface — and the policy's refusals for `password`.
pub async fn reset(
    database: &rd_db::Database,
    password: &str,
    disable_totp: bool,
) -> Result<Reset, ApiError> {
    validate_password(password)?;
    if !auth::admin_password_configured(database).await? {
        return Err(ApiError::conflict(
            "auth.password_reset_setup_pending",
            "No administrator password is set yet; finish the setup in the web interface",
        ));
    }
    auth::store_admin_password(database, password).await?;
    let sessions_ended = database.revoke_all_sessions().await?;
    let removed_material = if disable_totp {
        database.clear_mfa(rd_core::MfaKind::Totp).await?
    } else {
        Vec::new()
    };
    let password_login_off = database
        .get_setting(oidc_client::PASSWORD_LOGIN_OFF_SETTING)
        .await?
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    Ok(Reset {
        sessions_ended,
        removed_material,
        password_login_off,
    })
}

/// The record of one reset. What it says is which way, what ended and what stayed — never the
/// password, for which `AuditEvent` has no field. The caller adds the actor.
#[must_use]
pub fn audit_event(
    reset: &Reset,
    path: Path,
    prompted: bool,
    disable_totp: bool,
) -> audit::AuditEvent {
    audit::AuditEvent::success(rd_core::AuditAction::PasswordResetLocal)
        .detail("via", "cli")
        .detail("path", path.as_str())
        // `source`, not `password`: a detail under a credential's name is replaced whole.
        .detail("source", if prompted { "prompted" } else { "generated" })
        .detail("sessions_ended", reset.sessions_ended)
        .detail("totp", if disable_totp { "removed" } else { "kept" })
        .detail("passkeys", "kept")
        .detail("api_tokens", "kept")
        .detail(
            "password_login",
            if reset.password_login_off {
                "off"
            } else {
                "on"
            },
        )
}
