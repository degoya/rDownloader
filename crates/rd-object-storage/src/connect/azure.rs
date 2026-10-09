//! Azure Blob Storage (RD-150-05).
//!
//! A profile names the storage account; a link names the container and the blob. Four ways to
//! sign: the account key, a shared access signature, the machine's identity (service
//! principal, workload identity, managed identity), or nothing for a public container.
//!
//! A shared access signature expires on the date its issuer chose, and nothing here can renew
//! it. The runner opens the store again at every start, so a signature replaced on the profile
//! is the one the retry uses, and the partial file with its validators is continued rather
//! than downloaded again.

use base64::{Engine as _, engine::general_purpose::STANDARD};

use super::OpenError;
#[cfg(feature = "azure")]
use super::{Opening, Store};

/// Whether `text` decodes as an account key: the portal hands out 64 bytes in base64.
pub(crate) fn is_account_key(text: &str) -> bool {
    STANDARD
        .decode(text.trim())
        .is_ok_and(|key| !key.is_empty())
}

/// The query part of a shared access signature, pasted as the bare token, with its leading
/// `?`, or as the whole SAS URL the portal shows. `None` when it lacks the version (`sv`) or
/// the signature (`sig`) every kind of SAS carries.
pub(crate) fn sas_query(text: &str) -> Option<&str> {
    let text = text.trim();
    let query = text.split_once('?').map_or(text, |(_, query)| query);
    let keys: Vec<&str> = query
        .split('&')
        .filter_map(|pair| pair.split_once('=').map(|(key, _)| key))
        .collect();
    (keys.contains(&"sv") && keys.contains(&"sig")).then_some(query)
}

#[cfg(feature = "azure")]
pub(crate) fn open(opening: Opening<'_>) -> Result<Store, OpenError> {
    use std::sync::Arc;

    use object_store::azure::MicrosoftAzureBuilder;
    use rd_core::ObjectCredentialSource;
    use secrecy::ExposeSecret;

    let profile = opening.profile;
    let account = profile.account.as_deref().ok_or(OpenError::Credentials)?;
    let mut builder = MicrosoftAzureBuilder::new()
        .with_account(account)
        .with_container_name(opening.bucket)
        .with_client_options(super::client_options(&opening)?)
        .with_http_connector(super::connector(&opening))
        .with_retry(super::retry(opening.timeout));
    // Not `with_use_emulator`: that reads `AZURITE_BLOB_STORAGE_URL` from the environment, and
    // the address requests go to is the profile's, nobody else's.
    if let Some(endpoint) = profile.endpoint.as_deref() {
        builder = builder.with_endpoint(super::service_endpoint(endpoint)?);
    }
    builder = match profile.credential_source {
        ObjectCredentialSource::Static => {
            let key = opening.secret.as_ref().ok_or(OpenError::Credentials)?;
            if !is_account_key(key.expose_secret()) {
                return Err(OpenError::Credentials);
            }
            builder
                .with_access_key(key.expose_secret().trim())
                .with_credential_type("access_key")
        }
        ObjectCredentialSource::SharedAccessSignature => {
            let sas = opening.secret.as_ref().ok_or(OpenError::Credentials)?;
            let query = sas_query(sas.expose_secret()).ok_or(OpenError::Credentials)?;
            let pairs =
                object_store::azure::split_sas(query).map_err(|_| OpenError::Credentials)?;
            builder
                .with_sas_authorization(pairs)
                .with_credential_type("sas_token")
        }
        ObjectCredentialSource::Ambient => ambient_config(|name| std::env::var(name).ok())
            .into_iter()
            .fold(builder, |builder, (key, value)| {
                builder.with_config(key, value)
            }),
        ObjectCredentialSource::Anonymous => builder.with_skip_signature(true),
    };
    let store = Arc::new(builder.build().map_err(|_| OpenError::Other)?);
    Ok(Store {
        objects: store.clone(),
        parts: store,
    })
}

#[cfg(not(feature = "azure"))]
pub(crate) fn open(_opening: super::Opening<'_>) -> Result<super::Store, OpenError> {
    Err(OpenError::Unsupported)
}

/// The machine's identity, from the variables the Azure SDKs read.
///
/// Only the identity variables are taken — a service principal (client id, secret, tenant),
/// a workload identity (its federated token file) and the managed identity endpoint. The
/// account, an account key or a SAS in the environment would sign for something the settings
/// page does not show. With none of these set the builder asks the instance's managed
/// identity, the last step of the chain the Azure tools walk.
#[cfg(feature = "azure")]
pub(crate) fn ambient_config(
    variable: impl Fn(&str) -> Option<String>,
) -> Vec<(object_store::azure::AzureConfigKey, String)> {
    use object_store::azure::AzureConfigKey;
    [
        ("AZURE_CLIENT_ID", AzureConfigKey::ClientId),
        ("AZURE_CLIENT_SECRET", AzureConfigKey::ClientSecret),
        ("AZURE_TENANT_ID", AzureConfigKey::AuthorityId),
        ("AZURE_AUTHORITY_HOST", AzureConfigKey::AuthorityHost),
        (
            "AZURE_FEDERATED_TOKEN_FILE",
            AzureConfigKey::FederatedTokenFile,
        ),
        ("IDENTITY_ENDPOINT", AzureConfigKey::MsiEndpoint),
    ]
    .into_iter()
    .filter_map(|(name, key)| {
        variable(name)
            .filter(|value| !value.trim().is_empty())
            .map(|value| (key, value))
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::{is_account_key, sas_query};

    #[test]
    fn a_signature_is_taken_bare_with_its_question_mark_or_as_the_whole_url() {
        let token = "sv=2024-11-04&ss=b&srt=co&sp=rl&se=2026-10-01T00:00:00Z&sig=abc%2Bdef%3D";
        assert_eq!(sas_query(token), Some(token));
        assert_eq!(sas_query(&format!("?{token}")), Some(token));
        assert_eq!(
            sas_query(&format!(
                "https://media.blob.core.windows.net/shows?{token}"
            )),
            Some(token)
        );
        // An account key, a connection string or half a token is not a signature.
        assert_eq!(sas_query("c2VjcmV0"), None);
        assert_eq!(sas_query("sv=2024-11-04&sp=r"), None);
    }

    #[test]
    fn an_account_key_is_base64() {
        assert!(is_account_key(
            "Eby8vdM02xNOcqFlqUwJPLlmEtlCDXJ1OUzFT50uSRZ6IFsuFq2UVErCz4I6tq/K1SZFPTOtr/KBHBeksoGMGw=="
        ));
        assert!(!is_account_key("not a key!"));
        assert!(!is_account_key(""));
    }

    #[cfg(feature = "azure")]
    #[test]
    fn ambient_credentials_take_only_the_identity_variables() {
        use std::collections::HashMap;

        use object_store::azure::AzureConfigKey;

        let environment: HashMap<&str, &str> = [
            ("AZURE_CLIENT_ID", "client"),
            ("AZURE_TENANT_ID", "tenant"),
            ("AZURE_CLIENT_SECRET", " "),
            ("AZURE_STORAGE_ACCOUNT_KEY", "a2V5"),
            ("AZURE_STORAGE_SAS_TOKEN", "sv=1&sig=x"),
            ("AZURE_STORAGE_ENDPOINT", "https://elsewhere.example"),
        ]
        .into_iter()
        .collect();
        let config =
            super::ambient_config(|name| environment.get(name).map(|value| (*value).to_owned()));
        let keys: Vec<AzureConfigKey> = config.iter().map(|(key, _)| *key).collect();
        assert_eq!(
            keys,
            vec![AzureConfigKey::ClientId, AzureConfigKey::AuthorityId]
        );
    }
}
