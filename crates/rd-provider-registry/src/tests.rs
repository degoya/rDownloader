use super::*;

fn url(value: &str) -> Url {
    value.parse().expect("url")
}

/// Serialises the tests that mutate the process-wide dynamic layer.
static DYNAMIC_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn dynamic_row(plugin_id: &str, slug: &str) -> DynamicProvider {
    DynamicProvider {
        plugin_id: plugin_id.to_owned(),
        spec: ProviderSpec {
            slug: slug.to_owned(),
            display_name: "Fixture".to_owned(),
            kind: ProviderKind::Hoster,
            credentials: CredentialKind::ApiKey,
            username_required: false,
            transfer_auth: TransferAuth::None,
            secrets: vec![SecretSlot {
                reference: format!("{slug}_api_key"),
                domains: vec!["api.fixture.test".to_owned()],
                mode: None,
                filled_by: SecretFilledBy::Person,
            }],
            request_domains: vec!["fixture.test".to_owned(), "*.fixture.test".to_owned()],
            cookie_scope: Some("https://fixture.test/".to_owned()),
            match_hosts: vec!["fixture.test".to_owned()],
            host_aliases: vec![("fixture.example".to_owned(), "fixture.test".to_owned())],
            source: ProviderSource::Plugin,
            plugin_id: Some(plugin_id.to_owned()),
            plugin_version: Some("1.0.0".to_owned()),
        },
    }
}

/// A multihoster fixture: it accepts any host and therefore claims none.
fn multihoster_row(plugin_id: &str, slug: &str) -> DynamicProvider {
    let mut row = dynamic_row(plugin_id, slug);
    row.spec.kind = ProviderKind::Multihoster;
    row.spec.match_hosts = Vec::new();
    row
}

// -- lookup basics --------------------------------------------------
//
// These used to be asserted against the compiled-in hoster rows. With the table gone
// (RD-101-13) they are stated against fixtures, which is what they were always about:
// how the lookup treats a slug and a host, not which hosters happen to ship.

#[test]
fn by_slug_is_trimmed_and_case_insensitive() {
    let _guard = DYNAMIC_LOCK.lock().expect("lock");
    replace_dynamic(vec![dynamic_row("plugin-a", "fixture")]);
    assert_eq!(by_slug(" Fixture ").expect("spec").slug, "fixture");
    assert_eq!(by_slug("FIXTURE").expect("spec").slug, "fixture");
    assert!(by_slug("nonexistent").is_none());
    replace_dynamic(Vec::new());
}

#[test]
fn provider_for_url_strips_www_and_resolves_aliases() {
    let _guard = DYNAMIC_LOCK.lock().expect("lock");
    replace_dynamic(vec![dynamic_row("plugin-a", "fixture")]);
    for address in [
        "https://fixture.test/abc",
        "https://www.fixture.test/abc",
        "https://fixture.example/abc",
    ] {
        assert_eq!(
            provider_for_url(&url(address)).expect("spec").slug,
            "fixture",
            "{address}"
        );
    }
    assert!(provider_for_url(&url("https://example.com/file")).is_none());
    replace_dynamic(Vec::new());
}

#[test]
fn multihosters_never_claim_a_url() {
    let _guard = DYNAMIC_LOCK.lock().expect("lock");
    replace_dynamic(vec![multihoster_row("plugin-m", "fixture")]);
    assert!(provider_for_url(&url("https://fixture.test/")).is_none());
    for spec in all() {
        if spec.kind == ProviderKind::Multihoster {
            assert!(spec.match_hosts.is_empty(), "{} has match_hosts", spec.slug);
        }
    }
    replace_dynamic(Vec::new());
}

/// Nothing is a provider until a plugin says so.
///
/// The eleven compiled-in rows are gone; an installation without plugins therefore offers
/// no providers at all, instead of offering hosters it cannot resolve.
#[test]
fn without_plugins_there_are_no_providers() {
    let _guard = DYNAMIC_LOCK.lock().expect("lock");
    replace_dynamic(Vec::new());
    assert!(all().is_empty());
    assert!(by_slug("ddownload").is_none());
    assert!(!request_domain_allowed("ddownload.com"));
}

#[test]
fn every_row_comes_from_a_plugin() {
    let _guard = DYNAMIC_LOCK.lock().expect("lock");
    replace_dynamic(vec![dynamic_row("plugin-a", "fixture")]);
    assert!(
        all()
            .iter()
            .all(|spec| spec.source == ProviderSource::Plugin)
    );
    replace_dynamic(Vec::new());
}

// -- dynamic layer --------------------------------------------------

#[test]
fn plugin_rows_extend_every_gate() {
    let _guard = DYNAMIC_LOCK.lock().expect("lock");
    assert!(by_slug("fixture").is_none());
    assert!(!request_domain_allowed("fixture.test"));

    let rejected = replace_dynamic(vec![dynamic_row("plugin-a", "fixture")]);
    assert!(rejected.is_empty());

    let spec = by_slug("fixture").expect("dynamic provider is visible");
    assert_eq!(spec.source, ProviderSource::Plugin);
    assert!(request_domain_allowed("fixture.test"));
    assert!(request_domain_allowed("cdn.fixture.test"));
    assert!(secret_reference_allowed("fixture", "fixture_api_key"));
    assert!(secret_domain_allowed(
        "fixture_api_key",
        &url("https://api.fixture.test/account")
    ));
    assert_eq!(
        provider_for_url(&url("https://fixture.test/file"))
            .expect("spec")
            .slug,
        "fixture"
    );
    assert_eq!(
        cookie_scope("fixture").expect("scope").as_str(),
        "https://fixture.test/"
    );
    assert!(
        host_aliases()
            .iter()
            .any(|(alias, _)| alias == "fixture.example")
    );

    replace_dynamic(Vec::new());
    assert!(by_slug("fixture").is_none());
    assert!(!request_domain_allowed("fixture.test"));
}

#[test]
fn a_slug_belongs_to_the_plugin_that_claimed_it_first() {
    let _guard = DYNAMIC_LOCK.lock().expect("lock");
    replace_dynamic(Vec::new());
    try_register_dynamic(dynamic_row("plugin-a", "fixture")).expect("first claim wins");

    let error = try_register_dynamic(dynamic_row("plugin-b", "fixture"))
        .expect_err("a second plugin cannot take the slug");
    assert!(matches!(error, RegisterError::SlugTaken { .. }));

    // The same plugin replacing its own row is an upgrade, not a conflict.
    let mut upgraded = dynamic_row("plugin-a", "fixture");
    upgraded.spec.display_name = "Fixture 2".to_owned();
    try_register_dynamic(upgraded).expect("self-replacement is allowed");
    assert_eq!(by_slug("fixture").expect("spec").display_name, "Fixture 2");

    replace_dynamic(Vec::new());
}

/// The rule that carries the weight now that no slug is reserved.
///
/// A plugin cannot claim a credential another provider already owns. That is what stops it
/// from widening the set of hosts an existing secret may be sent to — the concrete danger
/// the built-in reservation used to cover, and the only half of it that was load-bearing.
/// The rest is signature trust: an untrusted package does not load at all.
#[test]
fn a_plugin_cannot_claim_another_providers_secret_reference() {
    let _guard = DYNAMIC_LOCK.lock().expect("lock");
    replace_dynamic(vec![dynamic_row("plugin-a", "fixture")]);

    let mut thief = dynamic_row("plugin-evil", "lookalike");
    // The owner's credential would otherwise become sendable to this plugin's hosts.
    thief.spec.secrets[0].reference = "fixture_api_key".to_owned();
    thief.spec.secrets[0].domains = vec!["evil.test".to_owned()];
    let error = try_register_dynamic(thief).expect_err("the credential is taken");
    assert!(matches!(error, RegisterError::SecretReferenceTaken { .. }));

    // The owner keeps its own hosts, and the thief's never became valid for the slot.
    assert!(secret_domain_allowed(
        "fixture_api_key",
        &url("https://api.fixture.test/account")
    ));
    assert!(!secret_domain_allowed(
        "fixture_api_key",
        &url("https://evil.test/account")
    ));
    replace_dynamic(Vec::new());
}

#[test]
fn older_versions_of_the_same_plugin_do_not_conflict_with_themselves() {
    let _guard = DYNAMIC_LOCK.lock().expect("lock");
    let mut newest = dynamic_row("plugin-a", "fixture");
    newest.spec.display_name = "Fixture 2".to_owned();
    let older = dynamic_row("plugin-a", "fixture");

    let rejected = replace_dynamic(vec![newest, older]);
    assert!(
        rejected.is_empty(),
        "a plugin's own older version is not a conflict"
    );
    assert_eq!(by_slug("fixture").expect("spec").display_name, "Fixture 2");
    replace_dynamic(Vec::new());
}

// -- gate behaviour --------------------------------------------------
//
// What each gate does with a pattern, stated against fixtures. The parity assertions for
// the actual hosters moved to `rd-plugin-host/tests/bundled_providers.rs` when the built-in
// table went (RD-101-13): this crate no longer knows a single hoster, and the facts it used
// to assert are now the manifests' to keep.

#[test]
fn request_domain_allowed_matches_exact_and_wildcard_suffixes() {
    let _guard = DYNAMIC_LOCK.lock().expect("lock");
    replace_dynamic(vec![dynamic_row("plugin-a", "fixture")]);
    assert!(request_domain_allowed("fixture.test"));
    assert!(request_domain_allowed("cdn123.fixture.test"));
    // A suffix match must not be a substring match.
    assert!(!request_domain_allowed("notfixture.test"));
    assert!(!request_domain_allowed("evil.com"));
    replace_dynamic(Vec::new());
}

#[test]
fn host_aliases_unions_every_provider() {
    let _guard = DYNAMIC_LOCK.lock().expect("lock");
    let mut second = dynamic_row("plugin-b", "other");
    second.spec.host_aliases = vec![("other.example".to_owned(), "other.test".to_owned())];
    second.spec.secrets[0].reference = "other_api_key".to_owned();
    replace_dynamic(vec![dynamic_row("plugin-a", "fixture"), second]);

    let aliases = host_aliases();
    assert!(
        aliases
            .iter()
            .any(|(alias, canonical)| alias == "fixture.example" && canonical == "fixture.test")
    );
    assert!(
        aliases
            .iter()
            .any(|(alias, canonical)| alias == "other.example" && canonical == "other.test")
    );
    replace_dynamic(Vec::new());
}

/// A host is declared, and everything else on earth is not (RD-110-38).
#[test]
fn only_a_declared_host_has_a_secret_fragment() {
    let _guard = DYNAMIC_LOCK.lock().expect("lock");
    replace_secret_fragment_hosts(vec!["MEGA.nz".to_owned(), "*.share.test".to_owned()]);

    let declared = |address: &str| fragment_is_secret(&address.parse::<Url>().expect("an address"));
    // Case and a leading `www.` are read the way every other host lookup here reads them.
    assert!(declared("https://mega.nz/file/abc#key"));
    assert!(declared("https://www.mega.nz/file/abc#key"));
    // A fragment is not required to answer: the question is about the host.
    assert!(declared("https://mega.nz/file/abc"));
    // `*.suffix` covers sub-domains only, never the bare suffix -- the same reading the
    // sandbox allowlist uses.
    assert!(declared("https://one.share.test/x#k"));
    assert!(!declared("https://share.test/x#k"));
    // And a suffix match is not a substring match.
    assert!(!declared("https://notmega.nz/file/abc#key"));
    assert!(!declared("https://example.com/a#b"));
    assert!(!declared("magnet:?xt=urn:btih:0123456789abcdef"));

    // Removing the plugin removes the declaration with it.
    replace_secret_fragment_hosts(Vec::new());
    assert!(!declared("https://mega.nz/file/abc#key"));
}

/// The reach half of ADR 0020 point 7, as patterns (RD-120-30). A wildcard reach is
/// covered only by a wildcard at least as wide; an exact entry covers exactly its host.
#[test]
fn a_wildcard_reach_needs_a_wildcard_slot_and_never_its_bare_suffix() {
    assert!(pattern_covers("api.example.test", "api.example.test"));
    assert!(pattern_covers(
        "*.nodes.example.test",
        "*.nodes.example.test"
    ));
    assert!(pattern_covers("*.example.test", "*.nodes.example.test"));
    assert!(pattern_covers("*.example.test", "a.example.test"));
    // The hole the old apex test had: `*.nodes.example.test` reaches every node, and an
    // entry for the bare `nodes.example.test` answers for none of them.
    assert!(!pattern_covers(
        "nodes.example.test",
        "*.nodes.example.test"
    ));
    assert!(!pattern_covers("*.nodes.example.test", "*.example.test"));
    assert!(!pattern_covers("*.example.test", "example.test"));
    assert!(!pattern_covers("*.example.test", "*"));
    // Sending follows the same reading: a sub-domain, never the bare suffix.
    assert!(slot_domain_matches(
        "*.nodes.example.test",
        "n1.nodes.example.test"
    ));
    assert!(!slot_domain_matches(
        "*.nodes.example.test",
        "nodes.example.test"
    ));
    assert!(!slot_domain_matches(
        "*.nodes.example.test",
        "evilnodes.example.test"
    ));
}
