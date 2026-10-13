//! The sign-in rows: the four cloud drives whose OAuth plugin runs `plugin-guest-oauth`'s
//! redirect flow. pCloud's sign-in has no PKCE, no grant type and two installations to try a
//! code at, so its module keeps its own.

use super::{BOX_OAUTH, DROPBOX_OAUTH, GOOGLE_OAUTH, ONEDRIVE_OAUTH};
use crate::support::storage::sign_in::{
    Case::{Denied, InvalidClient, InvalidGrant},
    DeviceAnswers, SignIn, Tokens,
};

/// Box: a confidential client, because its token endpoint has no public entrance and accepts
/// no PKCE; the secret travels on every grant, as a marker too.
pub const BOX: SignIn = SignIn {
    component: "rd-plugin-box-oauth",
    manifest: BOX_OAUTH,
    tokens: Tokens {
        granted: include_str!("../../../../fixtures/box/token_success.json"),
        renewed: include_str!("../../../../fixtures/box/token_renewed.json"),
        denied: include_str!("../../../../fixtures/box/token_denied.json"),
        invalid_grant: include_str!("../../../../fixtures/box/token_invalid_grant.json"),
        invalid_client: Some(include_str!(
            "../../../../fixtures/box/token_invalid_client.json"
        )),
        rate_limited: include_str!("../../../../fixtures/box/token_rate_limited.json"),
        device: None,
    },
    authorize: "https://account.box.com/api/oauth2/authorize?",
    token_endpoint: "https://api.box.com/oauth2/token",
    access_reference: "box_access_token",
    client_secret: Some("box_client_secret"),
    pkce: false,
    authorize_parameters: &[],
    renewal_fields: &[],
    expires_in_seconds: 4245,
    renewal_keeps_refresh: true,
    refusals: &[
        (Denied, "access_denied"),
        (InvalidGrant, "invalid_grant"),
        (InvalidClient, "invalid_client"),
    ],
    prose: &[
        "Refresh token has expired",
        "denied access to your application",
        "client credentials are invalid",
    ],
};

/// Dropbox: PKCE, and `token_access_type=offline`, without which it issues a short-lived token
/// and nothing to renew it from. Its access tokens last four hours.
pub const DROPBOX: SignIn = SignIn {
    component: "rd-plugin-dropbox-oauth",
    manifest: DROPBOX_OAUTH,
    tokens: Tokens {
        granted: include_str!("../../../../fixtures/dropbox/token_success.json"),
        renewed: include_str!("../../../../fixtures/dropbox/token_renewed.json"),
        denied: include_str!("../../../../fixtures/dropbox/token_denied.json"),
        invalid_grant: include_str!("../../../../fixtures/dropbox/token_invalid_grant.json"),
        invalid_client: None,
        rate_limited: include_str!("../../../../fixtures/dropbox/token_rate_limited.json"),
        device: None,
    },
    authorize: "https://www.dropbox.com/oauth2/authorize?",
    token_endpoint: "https://api.dropboxapi.com/oauth2/token",
    access_reference: "dropbox_access_token",
    client_secret: None,
    pkce: true,
    // Least privilege: read what the account can see, and nothing else. The same token is spent
    // by two other plugins, so a wider scope would widen all three at once.
    authorize_parameters: &[
        (
            "scope",
            "account_info.read files.metadata.read files.content.read sharing.read",
        ),
        ("token_access_type", "offline"),
    ],
    renewal_fields: &[],
    expires_in_seconds: 14_400,
    renewal_keeps_refresh: false,
    refusals: &[(Denied, "access_denied"), (InvalidGrant, "invalid_grant")],
    prose: &["code has expired", "chose not to"],
};

/// Google: PKCE, and `access_type=offline` with `prompt=consent`, without which Google issues
/// refresh material never, or exactly once. Its access tokens last an hour.
pub const GOOGLE_DRIVE: SignIn = SignIn {
    component: "rd-plugin-google-drive-oauth",
    manifest: GOOGLE_OAUTH,
    tokens: Tokens {
        granted: include_str!("../../../../fixtures/google_drive/token_success.json"),
        renewed: include_str!("../../../../fixtures/google_drive/token_renewed.json"),
        denied: include_str!("../../../../fixtures/google_drive/token_denied.json"),
        invalid_grant: include_str!("../../../../fixtures/google_drive/token_invalid_grant.json"),
        invalid_client: None,
        rate_limited: include_str!("../../../../fixtures/google_drive/token_rate_limited.json"),
        device: None,
    },
    authorize: "https://accounts.google.com/o/oauth2/v2/auth?",
    token_endpoint: "https://oauth2.googleapis.com/token",
    access_reference: "google_drive_access_token",
    client_secret: None,
    pkce: true,
    authorize_parameters: &[
        ("scope", "https://www.googleapis.com/auth/drive.readonly"),
        ("access_type", "offline"),
        ("prompt", "consent"),
    ],
    renewal_fields: &[],
    expires_in_seconds: 3599,
    renewal_keeps_refresh: false,
    refusals: &[(Denied, "access_denied"), (InvalidGrant, "invalid_grant")],
    prose: &["Token has been expired or revoked", "did not approve"],
};

/// Microsoft: PKCE, `Files.Read.All` because a sharing link goes through `/shares`, which Graph
/// grants to nothing narrower, and `offline_access` for the refresh token, which it rotates on
/// every renewal. The scope is repeated on a renewal, or the answer is a narrower token.
pub const ONEDRIVE: SignIn = SignIn {
    component: "rd-plugin-onedrive-oauth",
    manifest: ONEDRIVE_OAUTH,
    tokens: Tokens {
        granted: include_str!("../../../../fixtures/onedrive/token_success.json"),
        renewed: include_str!("../../../../fixtures/onedrive/token_renewed.json"),
        denied: include_str!("../../../../fixtures/onedrive/token_denied.json"),
        invalid_grant: include_str!("../../../../fixtures/onedrive/token_invalid_grant.json"),
        invalid_client: None,
        rate_limited: include_str!("../../../../fixtures/onedrive/token_rate_limited.json"),
        device: Some(DeviceAnswers {
            code: include_str!("../../../../fixtures/onedrive/device_code.json"),
            pending: include_str!("../../../../fixtures/onedrive/device_pending.json"),
            declined: include_str!("../../../../fixtures/onedrive/device_declined.json"),
        }),
    },
    authorize: "https://login.microsoftonline.com/common/oauth2/v2.0/authorize?",
    token_endpoint: "https://login.microsoftonline.com/common/oauth2/v2.0/token",
    access_reference: "onedrive_access_token",
    client_secret: None,
    pkce: true,
    authorize_parameters: &[("scope", "Files.Read.All offline_access")],
    renewal_fields: &[("scope", "Files.Read.All offline_access")],
    expires_in_seconds: 3599,
    renewal_keeps_refresh: true,
    refusals: &[(Denied, "access_denied"), (InvalidGrant, "invalid_grant")],
    prose: &["AADSTS", "Trace ID"],
};
