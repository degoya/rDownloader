//! Credential encoding and the credential-mode gates: the two places where getting it
//! wrong sends the wrong secret to the wrong host.

use rd_plugin_api::RequestAuthority;

use super::*;

/// `expand_request` with one value standing for the fixture's single reference.
fn expand_single(
    request: &mut HostHttpRequest,
    secret: Option<&str>,
    username: Option<&str>,
    client_id: Option<&str>,
    username_optional: bool,
) -> Result<bool, Failure> {
    let secrets = crate::native::references::single_for_tests(request, secret);
    expand_request(request, &secrets, username, client_id, username_optional)
}

fn request(content_type: &str, body: &str) -> HostHttpRequest {
    crate::native::register_bundled_providers_for_tests();
    HostHttpRequest {
        method: "POST".to_owned(),
        url: "https://ddownload.com/".parse().expect("url"),
        query: Vec::new(),
        headers: vec![HostRequestValue {
            name: "content-type".to_owned(),
            value_template: content_type.to_owned(),
        }],
        body: body.as_bytes().to_vec(),
        authority: RequestAuthority::Provider,
        write_methods: false,
        granted_secret: None,
    }
}

fn body_of(request: &HostHttpRequest) -> String {
    String::from_utf8(request.body.clone()).expect("utf-8 body")
}

#[test]
fn a_form_body_percent_encodes_the_substituted_credential() {
    let mut value = request(
        "application/x-www-form-urlencoded",
        "op=login&login={{username}}&password={{secret:ddownload_password}}",
    );
    expand_single(
        &mut value,
        Some("p@ss w=rd&x"),
        Some("me@example.test"),
        None,
        false,
    )
    .expect("expansion");
    assert_eq!(
        body_of(&value),
        "op=login&login=me%40example.test&password=p%40ss+w%3Drd%26x"
    );
}

/// The reason the encoding is not cosmetic: a password may contain the separators of the
/// document it lands in, and substituting it verbatim would let it add form fields the
/// site then acts on.
#[test]
fn a_form_body_cannot_be_used_to_inject_extra_fields() {
    let mut value = request(
        "application/x-www-form-urlencoded",
        "op=login&password={{secret:ddownload_password}}",
    );
    expand_single(&mut value, Some("secret&op=logout"), None, None, false).expect("expansion");
    let expanded = body_of(&value);
    assert_eq!(expanded, "op=login&password=secret%26op%3Dlogout");
    assert!(!expanded.contains("&op=logout"));
}

#[test]
fn a_json_body_still_gets_json_escaping() {
    let mut value = request("application/json", r#"{"password":"{{secret}}"}"#);
    expand_single(&mut value, Some(r#"a"b\c"#), None, None, false).expect("expansion");
    assert_eq!(body_of(&value), r#"{"password":"a\"b\\c"}"#);
}

/// Query and header values are encoded by the HTTP layer itself; escaping them here would
/// double-encode the credential and the far end would reject it.
#[test]
fn query_and_header_values_are_substituted_verbatim() {
    let mut value = request("text/plain", "");
    value.query.push(HostRequestValue {
        name: "key".to_owned(),
        value_template: "{{secret:ddownload_api_key}}".to_owned(),
    });
    value.headers.push(HostRequestValue {
        name: "authorization".to_owned(),
        value_template: "Bearer {{secret}}".to_owned(),
    });
    expand_single(&mut value, Some("a b&c"), None, None, false).expect("expansion");
    assert_eq!(value.query[0].value_template, "a b&c");
    assert_eq!(value.headers[1].value_template, "Bearer a b&c");
}

// -- HTTP Basic, the one credential a guest cannot build (RD-120-04) ---

/// The marker is both credentials at once, so the host is the only place that can build it:
/// `{{secret:…}}` substitutes on the way out, and a guest therefore never holds either half.
#[test]
fn a_basic_marker_becomes_base64_of_the_pair() {
    let mut value = request("text/plain", "");
    value.headers.push(HostRequestValue {
        name: "authorization".to_owned(),
        value_template: "Basic {{basic:ddownload_password}}".to_owned(),
    });
    expand_single(&mut value, Some("password"), Some("user"), None, false).expect("expansion");
    // base64("user:password"), which is the whole of what a provider reads.
    assert_eq!(
        value.headers[1].value_template,
        "Basic dXNlcjpwYXNzd29yZA=="
    );
}

/// Both halves have to be there, and a half-built header is not a lesser answer: it reaches
/// the provider and comes back 401, which reads as an expired sign-in on an account that was
/// never complete.
#[test]
fn a_basic_marker_refuses_rather_than_sending_half_a_credential() {
    for (secret, username, code) in [
        (None, Some("user"), "plugin.secret_missing"),
        (Some("password"), None, "plugin.username_missing"),
        (Some("password"), Some(""), "plugin.username_missing"),
        (
            Some("password"),
            Some("a:b"),
            "plugin.basic_username_invalid",
        ),
    ] {
        let mut value = request("text/plain", "");
        value.headers.push(HostRequestValue {
            name: "authorization".to_owned(),
            value_template: "Basic {{basic:ddownload_password}}".to_owned(),
        });
        let failure = expand_single(&mut value, secret, username, None, false)
            .expect_err("a half pair refuses");
        assert_eq!(failure.code.as_deref(), Some(code));
    }
}

/// The other direction (RD-120-38): a provider row without `username_required` may build the
/// pair with an empty name, which is how Pixeldrain takes its API key. The allowance changes
/// nothing else — the secret is still required, and a colon in a name that *is* given is still
/// refused.
#[test]
fn a_basic_marker_takes_an_empty_name_only_where_the_row_allows_one() {
    for username in [None, Some("")] {
        let mut value = request("text/plain", "");
        value.headers.push(HostRequestValue {
            name: "authorization".to_owned(),
            value_template: "Basic {{basic:ddownload_password}}".to_owned(),
        });
        expand_single(&mut value, Some("api-key"), username, None, true).expect("expansion");
        // base64(":api-key")
        assert_eq!(value.headers[1].value_template, "Basic OmFwaS1rZXk=");
    }
    for (secret, username, code) in [
        (None, None, "plugin.secret_missing"),
        (Some("key"), Some("a:b"), "plugin.basic_username_invalid"),
    ] {
        let mut value = request("text/plain", "");
        value.headers.push(HostRequestValue {
            name: "authorization".to_owned(),
            value_template: "Basic {{basic:ddownload_password}}".to_owned(),
        });
        let failure =
            expand_single(&mut value, secret, username, None, true).expect_err("still refused");
        assert_eq!(failure.code.as_deref(), Some(code));
    }
}

/// Standard base64 spells `+`, `/` and `=`, and all three mean something else inside a form
/// body — so the encoded pair is escaped for the document it lands in like any other value.
#[test]
fn a_basic_marker_inside_a_form_body_is_escaped_for_it() {
    let mut value = request(
        "application/x-www-form-urlencoded",
        "auth={{basic:ddownload_password}}",
    );
    expand_single(&mut value, Some("pw"), Some("ee"), None, false).expect("expansion");
    // base64("ee:pw") is "ZWU6cHc=", whose `=` would end the field it sits in.
    assert_eq!(body_of(&value), "auth=ZWU6cHc%3D");
}

/// The marker names a vault reference exactly as `{{secret:…}}` does, so the host has to find
/// it through the same finder — or the domain gate and the credential-mode gate that hang off
/// that finder would simply not run for it.
#[test]
fn a_basic_marker_is_found_as_a_credential_and_as_a_username() {
    let mut value = request("text/plain", "");
    value.headers.push(HostRequestValue {
        name: "authorization".to_owned(),
        value_template: "Basic {{basic:ddownload_password}}".to_owned(),
    });
    assert_eq!(
        crate::native::references::secret_references(&value).expect("within the cap"),
        ["ddownload_password"]
    );
    assert!(has_username_marker(&value));
}

/// Where a request came from travels to third parties, so no credential may be expanded into
/// it. The basic marker is held to that rule like the two it is made of.
#[test]
fn a_basic_marker_is_refused_in_a_header_that_leaves_for_a_stranger() {
    let mut value = request("text/plain", "");
    value.headers.push(HostRequestValue {
        name: "referer".to_owned(),
        value_template: "{{basic:ddownload_password}}".to_owned(),
    });
    let failure = expand_single(&mut value, Some("pw"), Some("user"), None, false)
        .expect_err("refused header");
    assert_eq!(
        failure.code.as_deref(),
        Some("plugin.secret_target_not_allowed")
    );
}

/// A body that is neither a form nor JSON is content — a file, a container — and a file that
/// reads `{{secret}}` is sent as those bytes (RD-120-66). It used to be substituted verbatim.
#[test]
fn a_body_that_is_not_a_form_or_json_is_content_and_never_expanded() {
    for content_type in [
        "text/plain",
        "application/octet-stream",
        "multipart/form-data; boundary=x",
    ] {
        let mut value = request(content_type, "{{secret}} {{secret:ddownload_password}}");
        expand_single(&mut value, Some("a&b"), None, None, false).expect("expansion");
        assert_eq!(
            body_of(&value),
            "{{secret}} {{secret:ddownload_password}}",
            "{content_type}"
        );
    }
}

/// A `PUT` uploads content whatever it declares: a JSON file on its way to a storage
/// destination is that file, not a template (RD-120-66).
#[test]
fn a_put_body_is_never_expanded_even_when_it_says_json() {
    let mut value = request("application/json", r#"{"note":"{{secret}}"}"#);
    value.method = "PUT".to_owned();
    expand_single(&mut value, Some("pw"), None, None, false).expect("expansion");
    assert_eq!(body_of(&value), r#"{"note":"{{secret}}"}"#);
}

// -- credential modes -------------------------------------------------

#[test]
fn ddownload_admits_only_the_slot_its_account_mode_selects() {
    crate::native::register_bundled_providers_for_tests();
    for (mode, allowed, refused) in [
        (
            CredentialMode::Login,
            "ddownload_password",
            "ddownload_api_key",
        ),
        (
            CredentialMode::ApiKey,
            "ddownload_api_key",
            "ddownload_password",
        ),
    ] {
        assert!(reference_active_for_account(
            "ddownload",
            allowed,
            Some(mode)
        ));
        assert!(!reference_active_for_account(
            "ddownload",
            refused,
            Some(mode)
        ));
    }
}

/// An account written before DDownload grew a login mode stores no mode at all. It has to
/// keep behaving exactly as it did, which means the API key slot and nothing else.
#[test]
fn an_account_without_a_stored_mode_falls_back_to_the_first_declared_one() {
    crate::native::register_bundled_providers_for_tests();
    assert!(reference_active_for_account(
        "ddownload",
        "ddownload_api_key",
        None
    ));
    assert!(!reference_active_for_account(
        "ddownload",
        "ddownload_password",
        None
    ));
}

/// A provider with a single slot never declares a mode, so every account reaches it.
#[test]
fn a_single_slot_provider_is_unaffected_by_modes() {
    crate::native::register_bundled_providers_for_tests();
    assert!(reference_active_for_account(
        "rapidgator",
        "rapidgator_password",
        None
    ));
    assert!(!reference_active_for_account(
        "rapidgator",
        "ddownload_api_key",
        None
    ));
}

#[test]
fn the_username_follows_the_active_slots_hosts() {
    crate::native::register_bundled_providers_for_tests();
    let website = "https://ddownload.com/".parse().expect("url");
    let api = "https://api-v2.ddownload.com/api/account/info"
        .parse()
        .expect("url");
    assert!(username_domain_allowed(
        "ddownload",
        Some(CredentialMode::Login),
        &website
    ));
    assert!(!username_domain_allowed(
        "ddownload",
        Some(CredentialMode::Login),
        &api
    ));
    assert!(username_domain_allowed(
        "ddownload",
        Some(CredentialMode::ApiKey),
        &api
    ));
    assert!(!username_domain_allowed(
        "ddownload",
        Some(CredentialMode::ApiKey),
        &website
    ));
}

/// One provider row, built by hand so the decision below is driven without the process-wide
/// provider table taking part.
fn oauth_provider(
    credentials: rd_provider_registry::CredentialKind,
    domains: &[&str],
) -> rd_provider_registry::ProviderSpec {
    rd_provider_registry::ProviderSpec {
        slug: "google_drive".to_owned(),
        display_name: "Google Drive".to_owned(),
        kind: rd_provider_registry::ProviderKind::Hoster,
        credentials,
        username_required: false,
        transfer_auth: rd_provider_registry::TransferAuth::None,
        secrets: vec![rd_provider_registry::SecretSlot {
            reference: "google_drive_access_token".to_owned(),
            domains: domains.iter().map(|domain| (*domain).to_owned()).collect(),
            mode: None,
            // The sign-in stores this one; nobody types an access token in (RD-106-03).
            filled_by: rd_provider_registry::SecretFilledBy::Flow,
        }],
        request_domains: vec!["www.googleapis.com".to_owned()],
        cookie_scope: None,
        match_hosts: Vec::new(),
        host_aliases: Vec::new(),
        source: rd_provider_registry::ProviderSource::Plugin,
        plugin_id: None,
        plugin_version: None,
    }
}

/// Which of an account's two credentials the transfer is holding.
///
/// A provider whose person registered their own application carries both at once (RD-106-03):
/// the client secret they typed, in the account's own slot, and the access token the sign-in
/// obtained, beside the flow. Everything that turns a stored credential into a request has to
/// tell them apart — sending the first where the second belongs hands the provider the wrong
/// credential on every transfer, which is what Box would have done (RD-120-05).
#[test]
fn a_provider_that_registers_its_own_application_keeps_its_token_beside_the_flow() {
    use rd_provider_registry::{CredentialKind, SecretFilledBy, SecretSlot};
    // The ordinary arrangement: one slot, the token *is* the account's secret.
    let mut ordinary = oauth_provider(CredentialKind::OAuth, &["www.googleapis.com"]);
    ordinary.secrets[0].filled_by = SecretFilledBy::Person;
    assert!(!super::token_beside_the_flow(&ordinary));

    // Box and Real-Debrid: the person's client secret, and the token beside it.
    let mut two_slots = oauth_provider(CredentialKind::OAuth, &["api.box.com"]);
    two_slots.secrets.insert(
        0,
        SecretSlot {
            reference: "box_client_secret".to_owned(),
            domains: vec!["api.box.com".to_owned()],
            mode: None,
            filled_by: SecretFilledBy::Person,
        },
    );
    assert!(super::token_beside_the_flow(&two_slots));
    // And the bearer gate is unchanged by the second slot: still only that provider's hosts.
    assert!(bearer_allowed(
        &two_slots,
        &Url::parse("https://api.box.com/2.0/files/1/content").expect("url")
    ));
    assert!(!bearer_allowed(
        &two_slots,
        &Url::parse("https://dl.boxcloud.com/x").expect("url")
    ));
}

/// An OAuth-signed provider's access token rides along on the transfer, but only to the exact
/// hosts its own manifest named (RD-106-04).
///
/// The narrow case a cloud drive needs, and the one that must stay narrow: this is the only
/// place a stored credential is turned into a header the download engine sends, so every other
/// provider kind, every other host and every cleartext address has to fall out of it.
#[test]
fn an_oauth_providers_token_rides_only_to_the_hosts_its_manifest_named() {
    use rd_provider_registry::CredentialKind;
    let spec = oauth_provider(CredentialKind::OAuth, &["www.googleapis.com"]);
    let url = |value: &str| Url::parse(value).expect("url");

    assert!(bearer_allowed(
        &spec,
        &url("https://www.googleapis.com/drive/v3/files/abc?alt=media")
    ));

    // A resolver that answered with somebody else's host must not take the token there —
    // which is why the address checked is the transfer's, not the one it started from.
    assert!(!bearer_allowed(
        &spec,
        &url("https://drive.google.com/file/d/abc/view")
    ));
    assert!(!bearer_allowed(
        &spec,
        &url("https://www.googleapis.com.evil.test/x")
    ));
    // A token in a cleartext header is a token published.
    assert!(!bearer_allowed(
        &spec,
        &url("http://www.googleapis.com/drive/v3/files/abc")
    ));

    // And every credential kind that is not OAuth keeps the arrangement it already had: an
    // API key reaches a provider through `{{secret:…}}` inside the plugin, never as a header
    // the engine attaches by itself.
    for other in [
        CredentialKind::ApiKey,
        CredentialKind::UsernamePassword,
        CredentialKind::Cookies,
        CredentialKind::ApiKeyOrCookies,
        CredentialKind::LoginOrApiKey,
        CredentialKind::NoneRequired,
    ] {
        let spec = oauth_provider(other, &["www.googleapis.com"]);
        assert!(
            !bearer_allowed(&spec, &url("https://www.googleapis.com/drive/v3/files/abc")),
            "{other:?}"
        );
    }
}

/// A provider that declares no host for its secret reaches none.
#[test]
fn an_oauth_provider_without_secret_domains_carries_its_token_nowhere() {
    let spec = oauth_provider(rd_provider_registry::CredentialKind::OAuth, &[]);
    assert!(!bearer_allowed(
        &spec,
        &Url::parse("https://www.googleapis.com/drive/v3/files/abc").expect("url")
    ));
}

/// A `{{client_id}}` marker is expanded from what the installation registered, and refuses
/// rather than vanishing when it registered nothing (RD-106-04).
///
/// The marker exists because a guest cannot read a configured value at all, and because a
/// client id compiled into this repository would put every installation on one shared quota.
/// What it must never become is a quietly empty string: the request that followed would be a
/// sign-in with no client, and the provider's answer to that names nothing anybody can act on.
#[test]
fn a_client_id_marker_is_expanded_or_refused_but_never_dropped() {
    assert_eq!(
        expand_client_id(
            "{{client_id}}",
            Some("1234-abc.apps.googleusercontent.com"),
            Escape::None
        )
        .expect("expanded"),
        "1234-abc.apps.googleusercontent.com"
    );
    // A value with nothing to expand is left exactly as it was.
    assert_eq!(
        expand_client_id("authorization_code", None, Escape::None).expect("unchanged"),
        "authorization_code"
    );
    let refusal = expand_client_id("{{client_id}}", None, Escape::None)
        .expect_err("an unregistered client is a refusal");
    assert_eq!(refusal.code.as_deref(), Some("oauth.client_not_configured"));
    // And the message says what to do, not only that something is absent.
    assert!(
        refusal.message.contains("Register one"),
        "{}",
        refusal.message
    );
}

/// The marker is form- and JSON-escaped like every other substitution, because a client id
/// lands in the same bodies the others do.
#[test]
fn a_client_id_is_encoded_for_the_document_it_lands_in() {
    assert_eq!(
        expand_client_id("id={{client_id}}", Some("a b&c"), Escape::Form).expect("expanded"),
        "id=a+b%26c"
    );
    assert_eq!(
        expand_client_id(r#"{"id":"{{client_id}}"}"#, Some("a\"b"), Escape::Json)
            .expect("expanded"),
        r#"{"id":"a\"b"}"#
    );
}

/// A client id is not credential material, so it does not narrow a redirect the way a secret
/// does — and it is expanded in query, headers and body alike.
#[test]
fn a_client_id_is_expanded_everywhere_without_making_the_request_carry_credentials() {
    let mut value = request(
        "application/x-www-form-urlencoded",
        "client_id={{client_id}}",
    );
    value.query.push(rd_plugin_api::HostRequestValue {
        name: "client_id".to_owned(),
        value_template: "{{client_id}}".to_owned(),
    });
    value.headers.push(rd_plugin_api::HostRequestValue {
        name: "X-Client".to_owned(),
        value_template: "{{client_id}}".to_owned(),
    });
    assert!(has_client_id_marker(&value));
    let carries =
        expand_single(&mut value, None, None, Some("client-42"), false).expect("expanded");
    assert!(
        !carries,
        "a client id is published by the provider; it must not narrow a redirect"
    );
    assert_eq!(value.query[0].value_template, "client-42");
    assert_eq!(
        value.headers.last().expect("header").value_template,
        "client-42"
    );
    assert_eq!(
        String::from_utf8(value.body).expect("utf-8"),
        "client_id=client-42"
    );
}
