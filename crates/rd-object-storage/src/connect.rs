//! Opening a store for one profile and one bucket.
//!
//! The only provider-specific part of the crate: everything else talks to the `object_store`
//! traits, which Azure Blob and Google Cloud Storage implement too. [`s3`] is here, the other
//! two are in [`azure`] and [`gcs`] (RD-150-05), each behind its cargo feature; all three
//! share [`client_options`], so the proxy, the custom CA and the timeouts reach every one.

pub(crate) mod azure;
pub(crate) mod gcs;

use std::{sync::Arc, time::Duration};

use object_store::{
    ClientOptions, ObjectStore, RetryConfig,
    aws::{AmazonS3Builder, AmazonS3ConfigKey, Checksum},
    multipart::MultipartStore,
};
use rd_core::{ObjectAddressing, ObjectCredentialSource, ObjectStorageProfile};
use secrecy::{ExposeSecret, SecretString};
use url::Url;

/// A store and its multipart half, which `object_store` keeps as two traits.
#[derive(Clone)]
pub(crate) struct Store {
    pub objects: Arc<dyn ObjectStore>,
    pub parts: Arc<dyn MultipartStore>,
}

/// What opening a store needs besides the profile, already read from the vault and the
/// network settings.
pub(crate) struct Opening<'a> {
    pub profile: &'a ObjectStorageProfile,
    pub bucket: &'a str,
    pub secret: Option<SecretString>,
    pub session_token: Option<SecretString>,
    /// The global proxy as a URL, credentials included; never logged.
    pub proxy: Option<SecretString>,
    pub custom_ca_pem: &'a [Vec<u8>],
    pub timeout: Duration,
}

/// Why a profile could not be turned into a store. Each is a configuration fault the
/// settings page can name, so the stable code is all that leaves this module.
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum OpenError {
    Endpoint,
    Certificate,
    Credentials,
    /// The provider's connector was left out of this build (`azure`, `gcs` features).
    #[cfg_attr(all(feature = "azure", feature = "gcs"), allow(dead_code))]
    Unsupported,
    Other,
}

pub(crate) fn open(opening: Opening<'_>) -> Result<Store, OpenError> {
    match opening.profile.provider {
        rd_core::ObjectStorageProvider::S3 => s3(opening),
        rd_core::ObjectStorageProvider::Azure => azure::open(opening),
        rd_core::ObjectStorageProvider::Gcs => gcs::open(opening),
    }
}

/// The queue retries a failed transfer on its own schedule; retrying for minutes underneath
/// it only hides the failure the person is waiting to see.
fn retry(timeout: Duration) -> RetryConfig {
    RetryConfig {
        max_retries: 3,
        retry_timeout: timeout,
        ..RetryConfig::default()
    }
}

fn s3(opening: Opening<'_>) -> Result<Store, OpenError> {
    let profile = opening.profile;
    let mut builder = AmazonS3Builder::new()
        .with_bucket_name(opening.bucket)
        .with_region(profile.region.as_deref().unwrap_or("us-east-1"))
        .with_client_options(client_options(&opening)?)
        .with_retry(retry(opening.timeout));
    let virtual_host = profile.addressing == ObjectAddressing::VirtualHost;
    builder = builder.with_virtual_hosted_style_request(virtual_host);
    if let Some(endpoint) = profile.endpoint.as_deref() {
        builder = builder.with_endpoint(bucket_endpoint(endpoint, opening.bucket, virtual_host)?);
    }
    if profile.checksums {
        builder = builder.with_checksum_algorithm(Checksum::SHA256);
    }
    builder = match profile.credential_source {
        ObjectCredentialSource::Static => {
            let key = profile
                .access_key_id
                .as_deref()
                .ok_or(OpenError::Credentials)?;
            let secret = opening.secret.as_ref().ok_or(OpenError::Credentials)?;
            let mut builder = builder
                .with_access_key_id(key)
                .with_secret_access_key(secret.expose_secret());
            if let Some(token) = &opening.session_token {
                builder = builder.with_token(token.expose_secret());
            }
            builder
        }
        ObjectCredentialSource::Anonymous => builder.with_skip_signature(true),
        ObjectCredentialSource::Ambient => ambient_config(|name| std::env::var(name).ok())
            .into_iter()
            .fold(builder, |builder, (key, value)| {
                builder.with_config(key, value)
            }),
        ObjectCredentialSource::SharedAccessSignature => return Err(OpenError::Credentials),
    };
    let store = Arc::new(builder.build().map_err(|_| OpenError::Other)?);
    Ok(Store {
        objects: store.clone(),
        parts: store,
    })
}

fn client_options(opening: &Opening<'_>) -> Result<ClientOptions, OpenError> {
    // No total request timeout: a download streams for as long as the object is large, and a
    // stalled one is caught by the read timeout here and by the staging's own per-read one.
    let mut options = ClientOptions::new()
        .with_timeout_disabled()
        .with_connect_timeout(opening.timeout)
        .with_read_timeout(opening.timeout)
        .with_user_agent(object_store::HeaderValue::from_static(concat!(
            "rDownloader/",
            env!("CARGO_PKG_VERSION")
        )));
    // Plain HTTP only when the person typed an `http://` endpoint; certificates are always
    // validated, against the system roots plus the custom CA every other transport trusts.
    let insecure = opening
        .profile
        .endpoint
        .as_deref()
        .and_then(|endpoint| Url::parse(endpoint).ok())
        .is_some_and(|url| url.scheme() == "http");
    options = options.with_allow_http(insecure);
    for pem in opening.custom_ca_pem {
        let certificates =
            object_store::Certificate::from_pem_bundle(pem).map_err(|_| OpenError::Certificate)?;
        for certificate in certificates {
            options = options.with_root_certificate(certificate);
        }
    }
    if let Some(proxy) = &opening.proxy {
        options = options.with_proxy_url(proxy.expose_secret());
    }
    Ok(options)
}

/// The endpoint `object_store` expects: with the bucket as the first host label for virtual
/// hosts, as given for paths (the bucket is appended there).
pub(crate) fn bucket_endpoint(
    endpoint: &str,
    bucket: &str,
    virtual_host: bool,
) -> Result<String, OpenError> {
    let mut url = Url::parse(endpoint).map_err(|_| OpenError::Endpoint)?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(OpenError::Endpoint);
    }
    if virtual_host {
        let host = url.host_str().ok_or(OpenError::Endpoint)?.to_owned();
        url.set_host(Some(&format!("{bucket}.{host}")))
            .map_err(|_| OpenError::Endpoint)?;
    }
    Ok(url.as_str().trim_end_matches('/').to_owned())
}

/// A profile's endpoint as the Azure and Google builders take it: the service root, which
/// may carry a path (Azurite's `http://127.0.0.1:10000/devstoreaccount1`).
#[cfg_attr(not(any(feature = "azure", feature = "gcs")), allow(dead_code))]
pub(crate) fn service_endpoint(endpoint: &str) -> Result<String, OpenError> {
    bucket_endpoint(endpoint, "", false)
}

/// The machine's own credentials, from the variables the AWS tools read.
///
/// Only the credential variables are taken. `AmazonS3Builder::from_env` would also take
/// `AWS_ENDPOINT` and `AWS_BUCKET`, and an endpoint that follows the environment instead of
/// the profile sends signed requests somewhere the settings page does not show. With none of
/// these set the builder asks the instance metadata service, which is the last step of the
/// same chain the AWS tools walk.
pub(crate) fn ambient_config(
    variable: impl Fn(&str) -> Option<String>,
) -> Vec<(AmazonS3ConfigKey, String)> {
    [
        ("AWS_ACCESS_KEY_ID", AmazonS3ConfigKey::AccessKeyId),
        ("AWS_SECRET_ACCESS_KEY", AmazonS3ConfigKey::SecretAccessKey),
        ("AWS_SESSION_TOKEN", AmazonS3ConfigKey::Token),
        (
            "AWS_WEB_IDENTITY_TOKEN_FILE",
            AmazonS3ConfigKey::WebIdentityTokenFile,
        ),
        ("AWS_ROLE_ARN", AmazonS3ConfigKey::RoleArn),
        ("AWS_ROLE_SESSION_NAME", AmazonS3ConfigKey::RoleSessionName),
        (
            "AWS_CONTAINER_CREDENTIALS_RELATIVE_URI",
            AmazonS3ConfigKey::ContainerCredentialsRelativeUri,
        ),
        (
            "AWS_CONTAINER_CREDENTIALS_FULL_URI",
            AmazonS3ConfigKey::ContainerCredentialsFullUri,
        ),
        (
            "AWS_CONTAINER_AUTHORIZATION_TOKEN_FILE",
            AmazonS3ConfigKey::ContainerAuthorizationTokenFile,
        ),
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
    use std::collections::HashMap;

    use object_store::aws::AmazonS3ConfigKey;

    use super::{OpenError, ambient_config, bucket_endpoint};

    #[test]
    fn a_virtual_host_endpoint_carries_the_bucket_in_its_host() {
        assert_eq!(
            bucket_endpoint("https://s3.example.com:9000/", "media", true).expect("endpoint"),
            "https://media.s3.example.com:9000"
        );
        assert_eq!(
            bucket_endpoint("http://127.0.0.1:9000", "media", false).expect("endpoint"),
            "http://127.0.0.1:9000"
        );
        assert_eq!(
            bucket_endpoint("ftp://files.example", "media", false),
            Err(OpenError::Endpoint)
        );
        assert_eq!(
            bucket_endpoint("not a url", "media", false),
            Err(OpenError::Endpoint)
        );
    }

    #[test]
    fn ambient_credentials_take_only_the_credential_variables() {
        let environment: HashMap<&str, &str> = [
            ("AWS_ACCESS_KEY_ID", "AKIDEXAMPLE"),
            ("AWS_SECRET_ACCESS_KEY", "secret"),
            ("AWS_SESSION_TOKEN", "  "),
            ("AWS_ENDPOINT", "https://elsewhere.example"),
            ("AWS_BUCKET", "other"),
        ]
        .into_iter()
        .collect();
        let config = ambient_config(|name| environment.get(name).map(|value| (*value).to_owned()));
        let keys: Vec<AmazonS3ConfigKey> = config.iter().map(|(key, _)| *key).collect();
        assert_eq!(
            keys,
            vec![
                AmazonS3ConfigKey::AccessKeyId,
                AmazonS3ConfigKey::SecretAccessKey
            ]
        );
    }

    #[test]
    fn a_container_endpoint_is_picked_up_for_ecs_and_eks() {
        let config = ambient_config(|name| {
            (name == "AWS_CONTAINER_CREDENTIALS_RELATIVE_URI")
                .then(|| "/v2/credentials/x".to_owned())
        });
        assert_eq!(
            config,
            vec![(
                AmazonS3ConfigKey::ContainerCredentialsRelativeUri,
                "/v2/credentials/x".to_owned()
            )]
        );
        // Nothing set is the instance metadata service, which the builder falls back to.
        assert!(ambient_config(|_| None).is_empty());
    }
}
