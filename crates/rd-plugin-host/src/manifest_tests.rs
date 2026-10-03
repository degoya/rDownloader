use super::*;

/// A manifest for one of the six extension types.
fn extension_toml(plugin_type: &str, extra: &str) -> String {
    format!(
        r#"manifest_version = 3
plugin_type = "{plugin_type}"
api_version = "0.10.0"
id = "019d0000-0000-7000-8000-00000000abce"
name = "Fixture Extension"
version = "0.1.0"
key_id = "fixture-v1"
public_key = "5C0fhOCoSaW9Ucdh1x3lUw05IX8YfNzJcgXkgnwjzeY="

[capabilities.net_http]
domains = ["example.test"]

[metadata]
description = "A fixture extension"
author = "Fixture Author"

[extension]
slug = "fixture_extension"
{extra}
"#
    )
}

/// A manifest of `plugin_type`, with `extra` written above the first table so a
/// top-level key stays a top-level key.
///
/// The trap this exists to avoid is TOML's, not this parser's: a bare key written after
/// `[capabilities.net_http]` belongs to that table, so `oauth_flows` placed there would
/// silently become an unknown capability rather than a list of ways in.
fn toplevel_toml(plugin_type: &str, extra: &str) -> String {
    format!(
        r#"manifest_version = 3
plugin_type = "{plugin_type}"
api_version = "0.10.0"
id = "019d0000-0000-7000-8000-00000000abcf"
name = "Fixture Extension"
version = "0.1.0"
key_id = "fixture-v1"
public_key = "5C0fhOCoSaW9Ucdh1x3lUw05IX8YfNzJcgXkgnwjzeY="
{extra}

[capabilities.net_http]
domains = ["example.test"]

[metadata]
description = "A fixture extension"
author = "Fixture Author"

[extension]
slug = "fixture_extension"
"#
    )
}

/// An OAuth manifest written before RD-106-01 still means what it meant: the redirect.
///
/// The compatibility promise of the whole job sits in this default. If a silent manifest
/// were read as offering both, the host would call a device entrance nobody implemented
/// the first time somebody signed in.
#[test]
fn an_oauth_manifest_without_the_field_offers_the_redirect() {
    let manifest: PluginManifest = toml::from_str(&toplevel_toml("oauth", "")).expect("parses");
    validate_manifest(&manifest).expect("valid");
    assert_eq!(
        manifest.oauth_flows(),
        [OAuthFlowManifest::Redirect].as_slice()
    );
    assert!(manifest.serves_oauth_flow(OAuthFlowManifest::Redirect));
    assert!(!manifest.serves_oauth_flow(OAuthFlowManifest::Device));
}

#[test]
fn an_oauth_manifest_may_offer_both_ways_in_and_states_their_order() {
    let manifest: PluginManifest = toml::from_str(&toplevel_toml(
        "oauth",
        r#"oauth_flows = ["device", "redirect"]"#,
    ))
    .expect("parses");
    validate_manifest(&manifest).expect("valid");
    assert_eq!(
        manifest.oauth_flows(),
        [OAuthFlowManifest::Device, OAuthFlowManifest::Redirect].as_slice()
    );
}

/// Two refusals, both of them a mistake an author would otherwise never hear about: the
/// field on a type that has no ways in, and one entrance named twice.
#[test]
fn oauth_flows_is_refused_where_it_means_nothing() {
    let wrong_type: PluginManifest =
        toml::from_str(&toplevel_toml("auth", r#"oauth_flows = ["device"]"#)).expect("parses");
    let error = validate_manifest(&wrong_type).expect_err("refused");
    assert!(error.to_string().contains("oauth_flows"), "{error}");

    let repeated: PluginManifest = toml::from_str(&toplevel_toml(
        "oauth",
        r#"oauth_flows = ["device", "device"]"#,
    ))
    .expect("parses");
    let error = validate_manifest(&repeated).expect_err("refused");
    assert!(error.to_string().contains("twice"), "{error}");
}

/// The declaration that puts a link fragment in the vault (RD-110-38).
///
/// Written above the first table on purpose: a bare key after `[capabilities.net_http]`
/// belongs to that table, and this one silently became an unknown capability the first
/// time it was written there. The catch-all is refused, because `*` would turn every
/// fragment on the internet into key material.
#[test]
fn a_secret_fragment_declaration_names_hosts_and_never_everything() {
    let declared: PluginManifest = toml::from_str(&toplevel_toml(
        "stream-transform",
        r#"secret_fragment_domains = ["example.test", "*.example.test"]"#,
    ))
    .expect("parses");
    validate_manifest(&declared).expect("valid");
    assert_eq!(
        declared.secret_fragment_domains,
        ["example.test".to_owned(), "*.example.test".to_owned()]
    );

    let everything: PluginManifest = toml::from_str(&toplevel_toml(
        "stream-transform",
        r#"secret_fragment_domains = ["*"]"#,
    ))
    .expect("parses");
    validate_manifest(&everything).expect_err("the catch-all is refused");

    // Absent is the ordinary case, and it is what every manifest written so far means.
    let silent: PluginManifest =
        toml::from_str(&toplevel_toml("stream-transform", "")).expect("parses");
    validate_manifest(&silent).expect("valid");
    assert!(silent.secret_fragment_domains.is_empty());
}

/// A notification destination may say "wherever the destination points" (RD-130-15),
/// because the host narrows it to that one host per delivery. A type whose allowlist is
/// its boundary still may not.
#[test]
fn a_notifier_may_declare_the_catch_all_and_an_enricher_may_not() {
    let with_wildcard = |plugin_type: &str| -> PluginManifest {
        toml::from_str(&toplevel_toml(plugin_type, "").replace(
            r#"domains = ["example.test"]"#,
            r#"domains = ["example.test", "*"]"#,
        ))
        .expect("parses")
    };
    validate_manifest(&with_wildcard("notifier")).expect("a notifier narrows per delivery");
    let error = validate_manifest(&with_wildcard("enricher")).expect_err("refused");
    assert!(error.to_string().contains("catch-all"), "{error}");
}

#[test]
fn every_extension_type_is_accepted_with_its_section() {
    for plugin_type in [
        "intake",
        "auth",
        "enricher",
        "notifier",
        "postprocess",
        "storage",
    ] {
        let manifest: PluginManifest =
            toml::from_str(&extension_toml(plugin_type, "")).expect(plugin_type);
        assert_eq!(manifest.plugin_type.as_str(), plugin_type);
        assert_eq!(manifest.message_slug(), "fixture_extension");
        validate_manifest(&manifest).unwrap_or_else(|error| panic!("{plugin_type}: {error}"));
    }
}

#[test]
fn an_extension_manifest_without_its_section_is_refused() {
    // Without a slug there is no namespace for the plugin's messages, and the
    // alternative — inventing one — makes two plugins collide silently.
    let raw =
        extension_toml("notifier", "").replace("[extension]\nslug = \"fixture_extension\"\n", "");
    let manifest: PluginManifest = toml::from_str(&raw).expect("parse");
    assert!(validate_manifest(&manifest).is_err());
}

#[test]
fn an_extension_manifest_may_not_claim_a_provider_or_a_transfer() {
    // Each section grants a different thing. A manifest declaring two of them is either
    // confused or trying for both, and neither is something to resolve by guessing.
    for extra in [
        "\n[provider]\nslug = \"x\"\nkind = \"hoster\"\ncredentials = \"api_key\"\n",
        "\n[transfer]\nslug = \"x\"\nschemes = [\"x\"]\n",
    ] {
        let manifest: PluginManifest =
            toml::from_str(&extension_toml("intake", extra)).expect("parse");
        assert!(validate_manifest(&manifest).is_err(), "{extra}");
    }
}

#[test]
fn a_resolver_may_not_declare_an_extension_section() {
    let manifest: PluginManifest =
        toml::from_str(&manifest_toml("\n[extension]\nslug = \"sneaky\"\n")).expect("parse");
    assert!(validate_manifest(&manifest).is_err());
}

#[test]
fn a_storage_destination_must_ask_for_a_way_out() {
    // A destination with no outbound grant could not upload anything; such a manifest is
    // a mistake, not a plugin that stores to nowhere.
    let raw = extension_toml("storage", "").replace(
        "[capabilities.net_http]\ndomains = [\"example.test\"]\n",
        "",
    );
    let manifest: PluginManifest = toml::from_str(&raw).expect("parse");
    assert!(validate_manifest(&manifest).is_err());
}

fn manifest_toml(extra: &str) -> String {
    format!(
        r#"manifest_version = 3
plugin_type = "resolver"
api_version = "0.10.0"
id = "019d0000-0000-7000-8000-00000000abcd"
name = "Fixture"
version = "1.2.3"
key_id = "fixture-v1"
public_key = "5C0fhOCoSaW9Ucdh1x3lUw05IX8YfNzJcgXkgnwjzeY="
max_concurrent_downloads = 1

[capabilities]
cookies = true
captcha = true
secrets = ["fixture_api_key", "onefichier_api_key"]

[capabilities.net_http]
domains = ["example.test", "*.example.test"]

[metadata]
description = "A fixture resolver"
author = "Fixture Author"

[provider]
slug = "fixture"
kind = "hoster"
credentials = "api_key"
{extra}
"#
    )
}

fn parse(extra: &str) -> Result<PluginManifest> {
    let manifest: PluginManifest = toml::from_str(&manifest_toml(extra))?;
    validate_manifest(&manifest)?;
    Ok(manifest)
}

#[test]
fn minimal_v3_manifest_validates() {
    let manifest = parse("").expect("valid manifest");
    assert_eq!(manifest.message_slug(), "fixture");
    assert_eq!(manifest.metadata.author, "Fixture Author");
    assert!(manifest.verifying_key().is_ok());
}

#[test]
fn requires_account_defaults_to_true_and_can_be_disabled() {
    let manifest = parse("").expect("valid manifest");
    assert!(manifest.requires_account);
    let toml = manifest_toml("").replace(
        "max_concurrent_downloads = 1",
        "max_concurrent_downloads = 1\nrequires_account = false",
    );
    let manifest: PluginManifest = toml::from_str(&toml).expect("parse");
    validate_manifest(&manifest).expect("valid manifest");
    assert!(!manifest.requires_account);
}

#[test]
fn an_older_manifest_revision_is_refused_as_outdated() {
    let toml = manifest_toml("").replace("manifest_version = 3", "manifest_version = 2");
    let manifest: PluginManifest = toml::from_str(&toml).expect("parse");
    let error = validate_manifest(&manifest).expect_err("v2 is no longer accepted");
    let rejection = error
        .downcast_ref::<ManifestRejection>()
        .expect("typed rejection");
    assert_eq!(rejection.code(), "plugin.manifest_outdated");
}

#[test]
fn an_unknown_plugin_type_is_refused_rather_than_treated_as_a_resolver() {
    // Deliberately a name no future plugin type will take. The original example here
    // was "notifier", which stopped being unknown the moment that type was added — the
    // test then asserted the opposite of what it was written to check.
    let toml = manifest_toml("").replace(
        r#"plugin_type = "resolver""#,
        r#"plugin_type = "teleporter""#,
    );
    let manifest: PluginManifest = toml::from_str(&toml).expect("parse");
    assert_eq!(
        manifest.plugin_type,
        PluginType::Unknown("teleporter".into())
    );
    let error = validate_manifest(&manifest).expect_err("unknown type");
    assert_eq!(
        error
            .downcast_ref::<ManifestRejection>()
            .expect("typed rejection")
            .code(),
        "plugin.capability_unknown"
    );
}

#[test]
fn key_derivation_is_only_for_the_types_whose_world_imports_it() {
    // Three worlds import `key-derivation` (RD-120-20). On any other type the grant
    // would be an interface the world cannot name, which is silent nonsense.
    for plugin_type in ["auth", "crawler", "stream-transform"] {
        let toml = extension_toml(
                plugin_type,
                "",
            )
            .replace(
                "[capabilities.net_http]",
                "[capabilities]\nkey_derivation = true\nsecrets = [\"fixture_password\"]\n\n[capabilities.net_http]",
            );
        let manifest: PluginManifest = toml::from_str(&toml).expect("parse");
        validate_manifest(&manifest)
            .unwrap_or_else(|error| panic!("{plugin_type} should be allowed: {error}"));
    }
    let toml = extension_toml("notifier", "").replace(
            "[capabilities.net_http]",
            "[capabilities]\nkey_derivation = true\nsecrets = [\"fixture_password\"]\n\n[capabilities.net_http]",
        );
    let manifest: PluginManifest = toml::from_str(&toml).expect("parse");
    let error = validate_manifest(&manifest).expect_err("a notifier must not declare it");
    assert!(
        format!("{error}").contains("key_derivation"),
        "the refusal does not name the grant: {error}"
    );
}

#[test]
fn key_derivation_without_a_named_secret_is_refused() {
    // Computing over a credential without naming one is a grant that can never be used.
    let toml = extension_toml("auth", "").replace(
        "[capabilities.net_http]",
        "[capabilities]\nkey_derivation = true\n\n[capabilities.net_http]",
    );
    let manifest: PluginManifest = toml::from_str(&toml).expect("parse");
    let error = validate_manifest(&manifest).expect_err("a grant with nothing to reach");
    assert!(
        format!("{error}").contains("capabilities.secrets"),
        "{error}"
    );
    assert!(
        manifest
            .capabilities
            .granted()
            .contains(&"key_derivation".to_owned())
    );
}

#[test]
fn a_stream_transform_plugin_may_own_the_provider_it_claims() {
    // The one exception to "an extension type declares no [provider]" (RD-120-20): such
    // a plugin is a resolver in everything but the world it exports, and MEGA had no
    // provider row at all until it declared one.
    let provider = r#"
[provider]
slug = "fixture_provider"
kind = "hoster"
credentials = "username_password"
username_required = true
secret_reference = "fixture_password"
secret_domains = ["example.test"]
"#;
    let with_secret = |plugin_type: &str| {
        format!(
            "{}{provider}",
            extension_toml(plugin_type, "").replace(
                "[capabilities.net_http]",
                "[capabilities]\nsecrets = [\"fixture_password\"]\n\n[capabilities.net_http]",
            )
        )
    };
    let manifest: PluginManifest = toml::from_str(&with_secret("stream-transform")).expect("parse");
    validate_manifest(&manifest).expect("a stream-transform plugin may own its provider");
    // Every other extension type still may not.
    let manifest: PluginManifest = toml::from_str(&with_secret("crawler")).expect("parse");
    let error = validate_manifest(&manifest).expect_err("a crawler must not");
    assert!(format!("{error}").contains("[provider]"), "{error}");
}

#[test]
fn a_password_provider_may_keep_its_sign_in_s_session_in_a_slot_of_its_own() {
    // RD-120-30: MEGA's password has to survive its sign-in, so the session gets a flow
    // slot the way an OAuth token beside a registered application does. Any other kind of
    // credential still may not declare one.
    let slots = |credentials: &str| {
        [
                extension_toml("stream-transform", "").replace(
                    "[capabilities.net_http]",
                    "[capabilities]\nsecrets = [\"fixture_password\", \"fixture_session\"]\n\n[capabilities.net_http]",
                ),
                format!(
                    r#"
[provider]
slug = "fixture_provider"
kind = "hoster"
credentials = "{credentials}"

[[provider.secrets]]
reference = "fixture_password"
domains = ["example.test"]

[[provider.secrets]]
reference = "fixture_session"
domains = ["example.test"]
filled_by = "flow"
"#
                ),
            ]
            .concat()
    };
    let manifest: PluginManifest = toml::from_str(&slots("username_password")).expect("parse");
    validate_manifest(&manifest).expect("a password provider may keep a session beside it");
    let manifest: PluginManifest = toml::from_str(&slots("api_key")).expect("parse");
    let error = validate_manifest(&manifest).expect_err("an API key has no sign-in");
    assert!(format!("{error}").contains("filled_by"), "{error}");
}

#[test]
fn an_unknown_capability_is_refused_rather_than_ignored() {
    // A capability that reads like something a plugin might plausibly want, and that this
    // build has no interface for — exactly the case that must not be silently ignored.
    let toml = manifest_toml("").replace("cookies = true", "cookies = true\nfilesystem = true");
    let manifest: PluginManifest = toml::from_str(&toml).expect("parse");
    let error = validate_manifest(&manifest).expect_err("unknown capability");
    assert_eq!(
        error
            .downcast_ref::<ManifestRejection>()
            .expect("typed rejection")
            .code(),
        "plugin.capability_unknown"
    );
}

#[test]
fn a_manifest_spells_the_oauth_credential_kind_the_way_the_documentation_does() {
    // `rename_all = "snake_case"` would have made this `o_auth`. The documentation says
    // `oauth`, and a manifest author has only the documentation to go on.
    let toml = manifest_toml("").replace(r#"credentials = "api_key""#, r#"credentials = "oauth""#);
    assert!(
        toml.contains(r#"credentials = "oauth""#),
        "the fixture no longer spells its credential kind the way this test expects"
    );
    let manifest: PluginManifest = toml::from_str(&toml).expect("manifest parses");
    assert_eq!(
        manifest.provider.expect("provider").credentials,
        CredentialKindManifest::OAuth
    );
}

#[test]
fn an_unsupported_api_version_is_refused() {
    let toml = manifest_toml("").replace(r#"api_version = "0.10.0""#, r#"api_version = "0.5.0""#);
    let manifest: PluginManifest = toml::from_str(&toml).expect("parse");
    assert!(validate_manifest(&manifest).is_err());
}

/// RD-190-06: a package built for `0.9.0` is refused, and under a code the plugin manager
/// names, rather than failing at the linker. The release note quotes this code. (RD-120-36 and
/// RD-130-11 asked the same of `0.7.0` and `0.8.0`; the contract moved again for the
/// post-processing step's removed files and warnings.)
#[test]
fn a_package_built_for_the_previous_contract_is_refused_by_name() {
    let toml = manifest_toml("").replace(r#"api_version = "0.10.0""#, r#"api_version = "0.9.0""#);
    let manifest: PluginManifest = toml::from_str(&toml).expect("parse");
    let error = validate_manifest(&manifest).expect_err("0.9.0 no longer links");
    let rejection = error
        .downcast_ref::<ManifestRejection>()
        .expect("a rejection the plugin manager can name");
    assert_eq!(rejection.code(), "plugin.capability_unknown");
}

#[test]
fn a_cookie_scope_without_the_cookies_grant_is_refused() {
    let toml =
        manifest_toml(r#"cookie_scope = "https://example.test/""#).replace("cookies = true\n", "");
    let manifest: PluginManifest = toml::from_str(&toml).expect("parse");
    assert!(validate_manifest(&manifest).is_err());
}

#[test]
fn secret_reference_must_be_granted_and_a_plain_identifier() {
    assert!(parse(r#"secret_reference = "fixture_api_key""#).is_ok());
    // A provider whose slug is not a valid identifier prefix (like `1fichier`) may still
    // name its reference freely; ownership is enforced at registration.
    assert!(parse(r#"secret_reference = "onefichier_api_key""#).is_ok());
    // Declared but never granted: the manifest would promise a credential the runtime
    // does not hand over, and the plugin manager would show a grant nobody enforces.
    assert!(parse(r#"secret_reference = "fixture_other_key""#).is_err());
    assert!(parse(r#"secret_reference = "Fixture-Key""#).is_err());
}

#[test]
fn a_wildcard_domain_does_not_cover_its_bare_suffix() {
    // The same reading `domain_allowed` applies at request time: `*.example.test`
    // grants sub-domains, not `example.test` itself.
    assert!(host_covered(
        "api.example.test",
        &["*.example.test".to_owned()]
    ));
    assert!(!host_covered(
        "example.test",
        &["*.example.test".to_owned()]
    ));
    assert!(host_covered("example.test", &["example.test".to_owned()]));
}

#[test]
fn secret_domains_must_stay_inside_the_sandbox() {
    assert!(
        parse(
            r#"secret_reference = "fixture_api_key"
secret_domains = ["api.example.test"]"#
        )
        .is_ok()
    );
    assert!(
        parse(
            r#"secret_reference = "fixture_api_key"
secret_domains = ["evil.invalid"]"#
        )
        .is_err()
    );
}

/// `transfer_auth = "basic"` hands the engine one credential and the hosts it may reach
/// (RD-120-38), so a manifest that cannot name both is refused rather than guessed at.
#[test]
fn transfer_auth_basic_needs_one_secret_with_hosts() {
    let row = |extra: &str| {
        provider_spec_from_manifest(&parse(extra).expect("valid manifest"))
            .expect("a provider row")
            .spec
            .transfer_auth
    };
    assert_eq!(
        row(r#"secret_reference = "fixture_api_key"
secret_domains = ["api.example.test"]"#),
        rd_provider_registry::TransferAuth::None,
        "absent means the transfer carries nothing"
    );
    assert_eq!(
        row(r#"transfer_auth = "basic"
secret_reference = "fixture_api_key"
secret_domains = ["api.example.test"]"#),
        rd_provider_registry::TransferAuth::Basic
    );
    // No hosts, no secret, or a kind whose secret is not the credential: refused.
    for extra in [
        r#"transfer_auth = "basic"
secret_reference = "fixture_api_key""#,
        r#"transfer_auth = "basic""#,
    ] {
        assert!(parse(extra).is_err(), "{extra}");
    }
    let cookies = manifest_toml(
        r#"transfer_auth = "basic"
secret_reference = "fixture_api_key"
secret_domains = ["api.example.test"]"#,
    )
    .replace(
        r#"credentials = "api_key""#,
        r#"credentials = "api_key_or_cookies""#,
    );
    let manifest: PluginManifest = toml::from_str(&cookies).expect("parse");
    assert!(validate_manifest(&manifest).is_err());
}

#[test]
fn cookie_scope_must_be_https_and_inside_the_sandbox() {
    assert!(parse(r#"cookie_scope = "https://example.test/""#).is_ok());
    assert!(parse(r#"cookie_scope = "http://example.test/""#).is_err());
    assert!(parse(r#"cookie_scope = "https://evil.invalid/""#).is_err());
}

#[test]
fn slug_charset_is_enforced() {
    for slug in ["Fixture", "fix-ture", "f", "fix ture"] {
        let toml = manifest_toml("").replace(r#"slug = "fixture""#, &format!(r#"slug = "{slug}""#));
        let manifest: PluginManifest = toml::from_str(&toml).expect("parse");
        assert!(
            validate_manifest(&manifest).is_err(),
            "{slug} should be rejected"
        );
    }
}

#[test]
fn metadata_urls_must_be_https() {
    let toml = manifest_toml("").replace(
        r#"author = "Fixture Author""#,
        r#"author = "Fixture Author"
homepage = "http://example.test""#,
    );
    let manifest: PluginManifest = toml::from_str(&toml).expect("parse");
    assert!(validate_manifest(&manifest).is_err());
}

#[test]
fn min_app_version_gates_installation() {
    let toml = manifest_toml("").replace(
        r#"author = "Fixture Author""#,
        r#"author = "Fixture Author"
min_app_version = "0.9.0""#,
    );
    let manifest: PluginManifest = toml::from_str(&toml).expect("parse");
    validate_manifest(&manifest).expect("valid");
    assert!(check_app_version(&manifest, "0.8.0").is_err());
    assert!(check_app_version(&manifest, "0.9.0").is_ok());
    assert!(check_app_version(&manifest, "1.0.0").is_ok());
}

#[test]
fn derived_spec_uses_the_manifest_as_the_only_authority() {
    let toml = manifest_toml(
        r#"secret_reference = "fixture_api_key"
secret_domains = ["api.example.test"]
cookie_scope = "https://example.test/""#,
    )
    .replace(
        "max_concurrent_downloads = 1",
        r#"match_domains = ["example.test"]
max_concurrent_downloads = 1"#,
    );
    let manifest: PluginManifest = toml::from_str(&toml).expect("parse");
    validate_manifest(&manifest).expect("valid");

    let row = provider_spec_from_manifest(&manifest).expect("resolver contributes a row");
    assert_eq!(row.plugin_id, manifest.id.to_string());
    assert_eq!(row.spec.slug, "fixture");
    assert_eq!(row.spec.display_name, "Fixture");
    assert_eq!(
        row.spec.source,
        rd_provider_registry::ProviderSource::Plugin
    );
    // The sandbox allowlist is what the provider may talk to.
    assert_eq!(row.spec.request_domains, manifest.domains());
    assert_eq!(row.spec.secret_reference(), Some("fixture_api_key"));
    assert_eq!(row.spec.match_hosts, vec!["example.test".to_owned()]);
}

#[test]
fn a_multihoster_never_claims_urls() {
    let toml = manifest_toml("")
        .replace(r#"kind = "hoster""#, r#"kind = "multihoster""#)
        .replace(
            "max_concurrent_downloads = 1",
            r#"match_domains = ["*"]
max_concurrent_downloads = 1"#,
        );
    let manifest: PluginManifest = toml::from_str(&toml).expect("parse");
    validate_manifest(&manifest).expect("valid");
    assert!(
        provider_spec_from_manifest(&manifest)
            .expect("resolver contributes a row")
            .spec
            .match_hosts
            .is_empty()
    );
}

#[test]
fn match_hosts_drops_wildcard_intake_patterns() {
    let toml = manifest_toml("").replace(
        "max_concurrent_downloads = 1",
        r#"match_domains = ["example.test", "*", "*.example.test"]
max_concurrent_downloads = 1"#,
    );
    let manifest: PluginManifest = toml::from_str(&toml).expect("parse");
    validate_manifest(&manifest).expect("valid");
    assert_eq!(manifest.match_hosts(), vec!["example.test".to_owned()]);
}

/// The resolver fixture with a provider that takes no account at all (RD-098-01).
fn account_less_toml(extra: &str) -> String {
    manifest_toml(extra).replace(r#"credentials = "api_key""#, r#"credentials = "none""#)
}

#[test]
fn a_provider_that_takes_no_account_reaches_the_registry() {
    let manifest: PluginManifest = toml::from_str(&account_less_toml("")).expect("parse");
    validate_manifest(&manifest).expect("valid");
    assert_eq!(
        manifest.provider.as_ref().expect("provider").credentials,
        CredentialKindManifest::NoneRequired
    );

    let row = provider_spec_from_manifest(&manifest).expect("resolver contributes a row");
    assert_eq!(
        row.spec.credentials,
        rd_provider_registry::CredentialKind::NoneRequired
    );
    assert!(row.spec.secret_reference().is_none());
    assert!(row.spec.cookie_scope.is_none());
}

#[test]
fn a_provider_that_takes_no_account_may_not_describe_one() {
    // Each of these would be a credential nobody can enter: the accounts list leaves such a
    // provider out, so the field would sit in the manifest granting reach for nothing.
    for extra in [
        "secret_reference = \"fixture_api_key\"",
        "cookie_scope = \"https://example.test/\"",
        "username_required = true",
    ] {
        let manifest: PluginManifest = toml::from_str(&account_less_toml(extra)).expect("parse");
        assert!(validate_manifest(&manifest).is_err(), "{extra}");
    }
}

/// RD-150-09: a provider that signs in with a code or takes a pasted API key. The shipped
/// Real-Debrid row is the example; each rule below refuses one way of getting it wrong.
#[test]
fn a_code_or_api_key_provider_describes_each_mode_completely() {
    const REAL_DEBRID: &str = include_str!("../../../plugins/realdebrid/manifest.toml");
    const GRANTS: &str = "secrets = [\"realdebrid_access_token\", \"realdebrid_api_token\"]";
    /// Every slot granted, so a rule of the provider row is what answers and not the grant.
    const ALL_GRANTED: &str = "secrets = [\"realdebrid_access_token\", \"realdebrid_api_token\", \"realdebrid_client_id\", \"realdebrid_client_secret\"]";
    let manifest: PluginManifest = toml::from_str(REAL_DEBRID).expect("parse");
    validate_manifest(&manifest).expect("the Real-Debrid row is valid");
    // The resolver is not granted the parts the sign-in keeps for itself.
    assert!(
        !manifest
            .capabilities
            .secrets
            .iter()
            .any(|reference| reference.starts_with("realdebrid_client_")),
    );

    let refused = |changes: &[(&str, &str)], expected: &str| {
        let mut changed = REAL_DEBRID.to_owned();
        for (from, to) in changes {
            assert!(changed.contains(from), "{from} is not in the manifest");
            changed = changed.replacen(from, to, 1);
        }
        let manifest: PluginManifest = toml::from_str(&changed).expect("parse");
        let error = validate_manifest(&manifest).expect_err(expected);
        assert!(
            format!("{error}").contains(expected),
            "{changes:?}: {error}"
        );
    };
    // The typed token belongs to the key mode and is typed by the person.
    refused(
        &[(
            "mode = \"api_key\"",
            "mode = \"api_key\"\nfilled_by = \"flow\"",
        )],
        "exactly one api_key entry",
    );
    // Nothing in the sign-in mode is typed.
    refused(
        &[
            (GRANTS, ALL_GRANTED),
            (
                "reference = \"realdebrid_client_secret\"\ndomains = [\"api.real-debrid.com\"]\nmode = \"oauth\"\nfilled_by = \"flow\"",
                "reference = \"realdebrid_client_secret\"\ndomains = [\"api.real-debrid.com\"]\nmode = \"oauth\"",
            ),
        ],
        "filled_by = \"flow\"",
    );
    // No third way in, and no slot without a mode.
    refused(
        &[("mode = \"api_key\"", "mode = \"login\"")],
        "oauth and api_key only",
    );
    refused(&[("mode = \"api_key\"\n", "")], "requires a mode");
    // The sign-in mode is this kind's alone.
    refused(
        &[
            (GRANTS, ALL_GRANTED),
            (
                "credentials = \"oauth_or_api_key\"",
                "credentials = \"login_or_api_key\"",
            ),
        ],
        "login_or_api_key",
    );
    // The resolver must be granted the token it sends in either mode.
    refused(
        &[(GRANTS, "secrets = [\"realdebrid_api_token\"]")],
        "realdebrid_access_token is not granted",
    );
}

/// A notification destination may offer settings (RD-170-09); nothing else may, and a setting
/// has to be something a person can pick and a plugin can be told.
#[test]
fn only_a_notifier_declares_settings_and_each_is_a_real_choice() {
    let setting = |body: &str| format!("\n[[extension.settings]]\n{body}\n");
    let priority = setting("name = \"priority\"\nchoices = [\"1\", \"2\", \"3\"]\ndefault = \"2\"");
    let manifest: PluginManifest =
        toml::from_str(&extension_toml("notifier", &priority)).expect("parses");
    validate_manifest(&manifest).expect("a notifier offers a setting");
    let settings = &manifest.extension.as_ref().expect("extension").settings;
    assert_eq!(settings.len(), 1);
    assert_eq!(settings[0].default.as_deref(), Some("2"));

    let manifest: PluginManifest =
        toml::from_str(&extension_toml("enricher", &priority)).expect("parses");
    let error = validate_manifest(&manifest).expect_err("an enricher has no target to set");
    assert!(error.to_string().contains("extension.settings"), "{error}");

    for (body, why) in [
        (
            "name = \"priority\"\nchoices = [\"1\", \"2\"]\ndefault = \"5\"".to_owned(),
            "a default it does not offer",
        ),
        ("name = \"priority\"\nchoices = []".to_owned(), "no choices"),
        (
            "name = \"Priority\"\nchoices = [\"1\"]".to_owned(),
            "a name that is no key",
        ),
        (
            "name = \"priority\"\nchoices = [\"1\", \"1\"]".to_owned(),
            "a choice offered twice",
        ),
        (
            format!(
                "name = \"priority\"\nchoices = [\"1\"]\n{}",
                setting("name = \"priority\"\nchoices = [\"2\"]")
            ),
            "a name declared twice",
        ),
    ] {
        let manifest: PluginManifest =
            toml::from_str(&extension_toml("notifier", &setting(&body))).expect("parses");
        validate_manifest(&manifest).expect_err(why);
    }
}
