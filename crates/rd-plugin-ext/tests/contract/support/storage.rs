//! The storage harness: Box, Dropbox, Google Drive, OneDrive and pCloud, three packages each,
//! driven through one set of cases from one provider table (RD-1280-05).
//!
//! Each provider ships a resolver, a folder crawler and a sign-in. What the five do alike is
//! here: the crawler's claims, refusals and sandbox (`crawler`), the sign-in from the address
//! the person is sent to through the renewal (`sign_in`), and the shape of the three manifests
//! (`shape`). What each does in its own way -- Box's offsets, Dropbox's cursors, Graph's next
//! links, Drive's page tokens, pCloud's two installations -- stays in the provider's module
//! with the mock that speaks its protocol. The rows themselves are in `table`.

use rd_plugin_host::{OAuthFlowManifest, PluginManifest};

pub mod crawler;
pub mod sign_in;
pub mod table;

/// The template a plugin writes where the host substitutes a vault value.
pub fn marker(reference: &str) -> String {
    format!("{{{{secret:{reference}}}}}")
}

/// One `#[tokio::test]` per shared case, defined in the module that invokes it, so the test
/// keeps the provider's module path and its own name: `name => case;` runs `case(&row)`.
macro_rules! shared_cases {
    ($row:expr; $($(#[$attr:meta])* $name:ident => $case:path;)+) => {
        $(
            $(#[$attr])*
            #[tokio::test]
            async fn $name() {
                $case(&$row).await;
            }
        )+
    };
}
pub(crate) use shared_cases;

/// The shape RD-106-04 decided, as one provider's row: one resolver carrying the provider row,
/// one crawler and one sign-in claiming that row's slug, and no capability beyond HTTP to the
/// provider's own hosts.
pub struct Shape {
    pub slug: &'static str,
    pub resolver: &'static str,
    pub crawler: &'static str,
    pub sign_in: &'static str,
    pub resolver_domains: &'static [&'static str],
    pub crawler_domains: &'static [&'static str],
    pub sign_in_domains: &'static [&'static str],
    /// The ways in, browser first, stated rather than guessed at.
    pub flows: &'static [OAuthFlowManifest],
    /// The one vault reference of a provider with a single credential; a provider with two
    /// slots asserts them in its own module.
    pub secret_reference: Option<&'static str>,
    /// The exact hosts the account's token may be sent to, where the row names them.
    pub secret_domains: Option<&'static [&'static str]>,
    /// Where the bytes may come from, where the row names it.
    pub download_domains: Option<&'static [&'static str]>,
    /// What the sign-in may expand: nothing, or the application secret of a confidential client.
    pub sign_in_secrets: &'static [&'static str],
}

fn owned(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

/// Asserts the row against the three real manifests and hands them back, resolver, crawler and
/// sign-in, for what a provider asserts beyond the row.
pub fn shape(row: &Shape) -> (PluginManifest, PluginManifest, PluginManifest) {
    let resolver: PluginManifest = toml::from_str(row.resolver).expect("resolver manifest");
    let crawler: PluginManifest = toml::from_str(row.crawler).expect("crawler manifest");
    let sign_in: PluginManifest = toml::from_str(row.sign_in).expect("oauth manifest");

    // Only a resolver manifest may carry a provider row, so the account, its vault references
    // and the slug the other two claim all live in exactly one of the three.
    let provider = resolver.provider.as_ref().expect("the provider row");
    assert_eq!(provider.slug, row.slug);
    assert_eq!(
        provider.credentials,
        rd_plugin_host::CredentialKindManifest::OAuth
    );
    // The account's username field holds this installation's own client id, and it is
    // required: a sign-in without one cannot start (RD-106-04, rule 8).
    assert!(
        provider.username_required,
        "an OAuth provider whose client is registered per installation has to ask for it"
    );
    if let Some(reference) = row.secret_reference {
        assert_eq!(provider.secret_reference.as_deref(), Some(reference));
    }
    if let Some(domains) = row.secret_domains {
        assert_eq!(provider.secret_domains, owned(domains));
    }
    if let Some(domains) = row.download_domains {
        assert_eq!(resolver.download_domains, owned(domains));
    }
    assert!(crawler.provider.is_none() && sign_in.provider.is_none());

    for manifest in [&crawler, &sign_in] {
        let claims = manifest
            .extension
            .as_ref()
            .map(|extension| extension.claims.clone())
            .unwrap_or_default();
        assert_eq!(claims, vec![row.slug.to_owned()], "{}", manifest.name);
    }

    assert_eq!(
        resolver.capabilities.domains(),
        owned(row.resolver_domains).as_slice()
    );
    assert_eq!(
        crawler.capabilities.domains(),
        owned(row.crawler_domains).as_slice()
    );
    assert_eq!(
        sign_in.capabilities.domains(),
        owned(row.sign_in_domains).as_slice()
    );

    // No cookies, no captcha and no raw sockets anywhere: API clients and a sign-in.
    for manifest in [&resolver, &crawler, &sign_in] {
        assert!(!manifest.capabilities.cookies, "{}", manifest.name);
        assert!(!manifest.capabilities.captcha, "{}", manifest.name);
        assert!(
            manifest.capabilities.net_stream.is_none(),
            "{}",
            manifest.name
        );
    }
    assert_eq!(sign_in.capabilities.secrets, owned(row.sign_in_secrets));

    // The flows offered, so the host never calls one that is not.
    assert_eq!(sign_in.oauth_flows(), row.flows);
    assert_eq!(
        sign_in.serves_oauth_flow(OAuthFlowManifest::Device),
        row.flows.contains(&OAuthFlowManifest::Device)
    );
    (resolver, crawler, sign_in)
}
