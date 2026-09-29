//! Read-only provider catalogue served to the accounts settings UI.
//!
//! Backed by `rd_provider_registry::all()`: adding a provider plugin means adding a row to
//! that registry, not touching this handler or any client-side provider list. One field is
//! not the registry's to answer — whether an authentication plugin can sign a provider in is
//! a fact about what is installed right now, so it is filled in here.

use axum::{Json, extract::State};

use crate::{AppState, dto::ProviderResponse};

#[utoipa::path(get, path = "/api/v1/providers", tag = "configuration", responses((status = 200, body = [ProviderResponse])))]
pub async fn list_providers(State(state): State<AppState>) -> Json<Vec<ProviderResponse>> {
    let device = state.auth_flows.providers().await;
    let oauth = state.auth_flows.oauth_providers().await;
    // One flag for the form, two plugin types behind it. What the accounts form needs to know
    // is whether there is a sign-in to offer instead of a field to type in; which of the two
    // worlds runs it is the service's business, not the client's.
    Json(catalogue(|slug| {
        device.supports(slug) || oauth.supports(slug)
    }))
}

/// The catalogue, with `device_flow` answered by whatever is installed.
///
/// Split out so the shape of the response can be tested without an application state: what
/// these tests are about is the mapping from the registry, not what is installed.
fn catalogue(supports: impl Fn(&str) -> bool) -> Vec<ProviderResponse> {
    rd_provider_registry::all()
        .iter()
        .map(|spec| {
            let mut response = ProviderResponse::from(spec);
            response.device_flow = supports(&spec.slug);
            response
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    use rd_provider_registry::{
        CredentialKind, CredentialMode, DynamicProvider, ProviderKind, ProviderSource,
        ProviderSpec, SecretFilledBy, SecretSlot, replace_dynamic,
    };

    /// Serialises the tests that write the process-wide registry.
    static REGISTRY_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// A provider row, the way an installed plugin's manifest contributes one.
    ///
    /// Stated as a fixture rather than reaching for a real hoster: since RD-101-13 nothing is
    /// compiled in, so `all()` is empty until a plugin is installed — and what this handler is
    /// responsible for is the mapping from a row to its response, not which hosters ship.
    fn row(slug: &str, kind: ProviderKind, credentials: CredentialKind) -> DynamicProvider {
        DynamicProvider {
            plugin_id: format!("plugin-{slug}"),
            spec: ProviderSpec {
                slug: slug.to_owned(),
                display_name: format!("Fixture {slug}"),
                kind,
                credentials,
                username_required: false,
                transfer_auth: rd_provider_registry::TransferAuth::None,
                secrets: match credentials {
                    CredentialKind::LoginOrApiKey => vec![
                        SecretSlot {
                            reference: format!("{slug}_api_key"),
                            domains: vec![format!("api.{slug}.test")],
                            mode: Some(CredentialMode::ApiKey),
                            filled_by: SecretFilledBy::Person,
                        },
                        SecretSlot {
                            reference: format!("{slug}_password"),
                            domains: vec![format!("{slug}.test")],
                            mode: Some(CredentialMode::Login),
                            filled_by: SecretFilledBy::Person,
                        },
                    ],
                    _ => vec![SecretSlot {
                        reference: format!("{slug}_api_key"),
                        domains: vec![format!("api.{slug}.test")],
                        mode: None,
                        filled_by: SecretFilledBy::Person,
                    }],
                },
                request_domains: vec![format!("{slug}.test")],
                cookie_scope: None,
                match_hosts: match kind {
                    ProviderKind::Hoster => vec![format!("{slug}.test")],
                    ProviderKind::Multihoster => Vec::new(),
                },
                host_aliases: Vec::new(),
                source: ProviderSource::Plugin,
                plugin_id: Some(format!("plugin-{slug}")),
                plugin_version: Some("2.1.0".to_owned()),
            },
        }
    }

    fn fixtures() -> Vec<DynamicProvider> {
        vec![
            row(
                "twoways",
                ProviderKind::Hoster,
                CredentialKind::LoginOrApiKey,
            ),
            row("oneway", ProviderKind::Multihoster, CredentialKind::ApiKey),
        ]
    }

    #[tokio::test]
    async fn lists_every_registry_row_with_its_fields() {
        let _guard = REGISTRY_LOCK.lock().expect("lock");
        replace_dynamic(fixtures());

        let providers = catalogue(|_| false);
        assert_eq!(providers.len(), rd_provider_registry::all().len());

        let hoster = providers
            .iter()
            .find(|provider| provider.slug == "twoways")
            .expect("present");
        assert_eq!(hoster.display_name, "Fixture twoways");
        assert!(matches!(
            hoster.kind,
            crate::dto::ProviderKindResponse::Hoster
        ));
        assert!(matches!(
            hoster.credentials,
            crate::dto::ProviderCredentialsResponse::LoginOrApiKey
        ));
        assert!(!hoster.username_required);
        // The form needs the choices, and their order: the first is the account editor's
        // default, and it is the one an account written before the choice existed falls back to.
        assert_eq!(
            hoster.credential_modes,
            vec![CredentialMode::ApiKey, CredentialMode::Login]
        );

        let multihoster = providers
            .iter()
            .find(|provider| provider.slug == "oneway")
            .expect("present");
        assert!(matches!(
            multihoster.kind,
            crate::dto::ProviderKindResponse::Multihoster
        ));
        assert!(multihoster.credential_modes.is_empty());

        // Which plugin, and which of its installed versions, has to reach the form: two versions
        // can sit installed side by side and the dropdown used to show the bare name either way.
        assert_eq!(hoster.plugin_id.as_deref(), Some("plugin-twoways"));
        assert_eq!(hoster.plugin_version.as_deref(), Some("2.1.0"));
        replace_dynamic(Vec::new());
    }

    /// An installation without plugins offers nothing, rather than hosters it cannot resolve.
    #[tokio::test]
    async fn without_plugins_the_catalogue_is_empty() {
        let _guard = REGISTRY_LOCK.lock().expect("lock");
        replace_dynamic(Vec::new());
        assert!(catalogue(|_| false).is_empty());
    }

    #[tokio::test]
    async fn serializes_kind_and_credentials_as_snake_case() {
        let _guard = REGISTRY_LOCK.lock().expect("lock");
        replace_dynamic(fixtures());

        let value = serde_json::to_value(catalogue(|_| false)).expect("serializable");
        let entries = value.as_array().expect("array");
        let hoster = entries
            .iter()
            .find(|entry| entry["slug"] == "twoways")
            .expect("present");
        assert_eq!(hoster["kind"], "hoster");
        assert_eq!(hoster["credentials"], "login_or_api_key");
        assert_eq!(
            hoster["credential_modes"],
            serde_json::json!(["api_key", "login"])
        );

        let multihoster = entries
            .iter()
            .find(|entry| entry["slug"] == "oneway")
            .expect("present");
        assert_eq!(multihoster["kind"], "multihoster");
        assert_eq!(multihoster["credentials"], "api_key");
        // A provider with one way to sign in offers no choice, and the field is left out.
        assert!(multihoster.get("credential_modes").is_none());
        replace_dynamic(Vec::new());
    }

    /// The accounts form offers "take over from browser" only where a plugin declares the one
    /// site whose session may be handed over, and names that site (RD-120-45).
    #[tokio::test]
    async fn a_declared_cookie_scope_reaches_the_form_as_its_host() {
        let _guard = REGISTRY_LOCK.lock().expect("lock");
        let mut scoped = row(
            "twoways",
            ProviderKind::Hoster,
            CredentialKind::LoginOrApiKey,
        );
        scoped.spec.cookie_scope = Some("https://twoways.test/".to_owned());
        replace_dynamic(vec![
            scoped,
            row("oneway", ProviderKind::Multihoster, CredentialKind::ApiKey),
        ]);

        let providers = catalogue(|_| false);
        let host = |slug: &str| {
            providers
                .iter()
                .find(|provider| provider.slug == slug)
                .and_then(|provider| provider.cookie_scope_host.clone())
        };
        assert_eq!(host("twoways").as_deref(), Some("twoways.test"));
        assert_eq!(host("oneway"), None);
        replace_dynamic(Vec::new());
    }
}
