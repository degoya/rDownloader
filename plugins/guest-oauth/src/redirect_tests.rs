//! The authorization URL every redirect plugin sends the person to, byte for byte as each one
//! built it by hand before RD-1120-10.

use super::{CLIENT_ID_MARKER, Client, Device, Provider, REDIRECT_URI, secret_template};
use crate::token::Waiting;
use plugin_common::pkce;

fn refused(_error: &str) -> &'static str {
    "refresh_refused"
}

const PUBLIC: Provider = Provider {
    slug: "dropbox_oauth",
    name: "dropbox",
    authorize_endpoint: "https://www.dropbox.com/oauth2/authorize",
    token_endpoint: "https://api.dropboxapi.com/oauth2/token",
    client_id: CLIENT_ID_MARKER,
    scope: Some("account_info.read files.metadata.read"),
    authorize_extra: "&token_access_type=offline",
    token_extra: &[],
    client: Client::Public,
    device: Device::BrowserOnly("dropbox"),
    waiting: Waiting {
        errors: &["slow_down"],
        fields: &["retry_after"],
    },
    refusal_code: refused,
};

const CONFIDENTIAL: Provider = Provider {
    slug: "box_oauth",
    name: "box",
    authorize_endpoint: "https://account.box.com/api/oauth2/authorize",
    token_endpoint: "https://api.box.com/oauth2/token",
    client_id: CLIENT_ID_MARKER,
    scope: None,
    authorize_extra: "",
    token_extra: &[],
    client: Client::Confidential {
        secret_reference: "box_client_secret",
        unregistered: "no application",
    },
    device: Device::BrowserOnly("box"),
    waiting: Waiting {
        errors: &["slow_down"],
        fields: &["retry_after"],
    },
    refusal_code: refused,
};

#[test]
fn a_public_client_carries_scope_state_challenge_and_its_extras_in_that_order() {
    let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
    let expected = format!(
        "https://www.dropbox.com/oauth2/authorize?response_type=code&client_id={}&redirect_uri={}\
         &scope={}&state={}&code_challenge={}&code_challenge_method=S256\
         &token_access_type=offline",
        CLIENT_ID_MARKER,
        pkce::percent_encode(REDIRECT_URI),
        pkce::percent_encode("account_info.read files.metadata.read"),
        pkce::percent_encode("the-state"),
        pkce::percent_encode(&pkce::challenge(verifier)),
    );
    assert_eq!(
        PUBLIC.authorization_url("the-state", Some(verifier)),
        expected
    );
}

/// Box: no scope, no challenge, and the marker left for the host rather than encoded.
#[test]
fn a_confidential_client_without_scope_carries_the_state_alone() {
    assert_eq!(
        CONFIDENTIAL.authorization_url("the-state", None),
        format!(
            "https://account.box.com/api/oauth2/authorize?response_type=code\
             &client_id={{{{client_id}}}}&redirect_uri={}&state=the-state",
            pkce::percent_encode(REDIRECT_URI),
        )
    );
}

#[test]
fn a_secret_is_named_as_the_marker_the_host_expands() {
    assert_eq!(
        secret_template("box_client_secret"),
        "{{secret:box_client_secret}}"
    );
}
