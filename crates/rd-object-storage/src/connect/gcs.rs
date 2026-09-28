//! Google Cloud Storage (RD-150-05).
//!
//! Three ways to sign: a service account key (the JSON file the console hands out), the
//! machine's application default credentials or metadata server, or nothing for a public
//! bucket. Access tokens are short-lived; `object_store` fetches a new one before the old one
//! runs out, through the same client options, so the proxy and the custom CA apply to that
//! request as well.

use serde_json::Value;

use super::OpenError;
#[cfg(feature = "gcs")]
use super::{Opening, Store};

/// Google's own service, where requests go when the profile names no endpoint.
#[cfg(feature = "gcs")]
const GOOGLE_STORAGE: &str = "https://storage.googleapis.com";

/// Whether `text` is a service account key `object_store` can sign with.
///
/// A key that switches OAuth off (`disable_oauth`, meant for local emulators) is refused: the
/// profile would say "stored key" and send every request unsigned.
pub(crate) fn is_service_account_key(text: &str) -> bool {
    let Ok(Value::Object(key)) = serde_json::from_str::<Value>(text.trim()) else {
        return false;
    };
    let text_field = |name: &str| {
        key.get(name)
            .and_then(Value::as_str)
            .is_some_and(|value| !value.trim().is_empty())
    };
    let kind = key.get("type").and_then(Value::as_str);
    matches!(kind, None | Some("service_account"))
        && text_field("client_email")
        && text_field("private_key_id")
        && key
            .get("private_key")
            .and_then(Value::as_str)
            .is_some_and(|pem| pem.contains("PRIVATE KEY"))
        && !key
            .get("disable_oauth")
            .and_then(Value::as_bool)
            .unwrap_or(false)
}

#[cfg(feature = "gcs")]
pub(crate) fn open(opening: Opening<'_>) -> Result<Store, OpenError> {
    use std::sync::Arc;

    use object_store::{
        StaticCredentialProvider,
        gcp::{GcpCredential, GoogleCloudStorageBuilder},
    };
    use rd_core::ObjectCredentialSource;
    use secrecy::ExposeSecret;

    let profile = opening.profile;
    // Always set, because a service account key may carry a `gcs_base_url` of its own, and
    // requests go where the settings page says they go.
    let base_url = match profile.endpoint.as_deref() {
        Some(endpoint) => super::service_endpoint(endpoint)?,
        None => GOOGLE_STORAGE.to_owned(),
    };
    let mut builder = GoogleCloudStorageBuilder::new()
        .with_bucket_name(opening.bucket)
        .with_base_url(&base_url)
        .with_client_options(super::client_options(&opening)?)
        .with_retry(super::retry(opening.timeout));
    builder = match profile.credential_source {
        ObjectCredentialSource::Static => {
            let key = opening.secret.as_ref().ok_or(OpenError::Credentials)?;
            if !is_service_account_key(key.expose_secret()) {
                return Err(OpenError::Credentials);
            }
            builder.with_service_account_key(key.expose_secret().trim())
        }
        // `GOOGLE_APPLICATION_CREDENTIALS` first, then the file `gcloud auth
        // application-default login` writes, then the metadata server: the order of the
        // Google tools. The builder looks for the latter two itself.
        ObjectCredentialSource::Ambient => match std::env::var("GOOGLE_APPLICATION_CREDENTIALS")
            .ok()
            .filter(|path| !path.trim().is_empty())
        {
            Some(path) => builder.with_application_credentials(path),
            None => builder,
        },
        // An empty static credential keeps the builder from reading the machine's application
        // default credentials for a profile that signs nothing.
        ObjectCredentialSource::Anonymous => builder.with_skip_signature(true).with_credentials(
            Arc::new(StaticCredentialProvider::new(GcpCredential {
                bearer: String::new(),
            })),
        ),
        ObjectCredentialSource::SharedAccessSignature => return Err(OpenError::Credentials),
    };
    // The key or the credentials file is all the builder reads that can be wrong; the bucket
    // and the endpoint were checked above.
    let store = Arc::new(builder.build().map_err(|_| OpenError::Credentials)?);
    Ok(Store {
        objects: store.clone(),
        parts: store,
    })
}

#[cfg(not(feature = "gcs"))]
pub(crate) fn open(_opening: super::Opening<'_>) -> Result<super::Store, OpenError> {
    Err(OpenError::Unsupported)
}

#[cfg(test)]
mod tests {
    use super::is_service_account_key;

    const KEY: &str = r#"{
        "type": "service_account",
        "project_id": "media",
        "private_key_id": "0123456789abcdef",
        "private_key": "-----BEGIN PRIVATE KEY-----\nMIIE\n-----END PRIVATE KEY-----\n",
        "client_email": "downloader@media.iam.gserviceaccount.com"
    }"#;

    #[test]
    fn a_service_account_key_is_the_json_file_the_console_hands_out() {
        assert!(is_service_account_key(KEY));
        assert!(!is_service_account_key("-----BEGIN PRIVATE KEY-----"));
        assert!(!is_service_account_key(
            &KEY.replace("service_account", "authorized_user")
        ));
        assert!(!is_service_account_key(
            &KEY.replace("\"client_email\"", "\"email\"")
        ));
        // A key for an emulator would switch signing off behind the profile's back.
        assert!(!is_service_account_key(
            &KEY.replace("\"type\"", "\"disable_oauth\": true, \"type\"")
        ));
    }
}
