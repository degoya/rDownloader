//! Validation of an object storage profile request (RD-150-04, RD-150-05): what each
//! provider needs, what it cannot use, and the shape of the secret it signs with.

use rd_api_core::input_checks::optional_text;
use rd_core::{
    MAX_OBJECT_ACCESS_KEY, MAX_OBJECT_ENDPOINT, MAX_OBJECT_SECRET, ObjectAddressing,
    ObjectCredentialSource, ObjectStorageProfile, ObjectStorageProvider,
};

use crate::{
    ApiError,
    config_fields::{validate_name, validate_secret_value},
};

/// The request fields every profile has, before validation.
pub(super) struct Draft<'a> {
    pub(super) name: &'a str,
    pub(super) provider: ObjectStorageProvider,
    pub(super) endpoint: Option<String>,
    pub(super) region: Option<String>,
    pub(super) bucket: Option<String>,
    pub(super) addressing: Option<ObjectAddressing>,
    pub(super) source: ObjectCredentialSource,
    pub(super) access_key_id: Option<String>,
    pub(super) account: Option<String>,
    pub(super) ambient_custom_endpoint: bool,
}

/// The same, validated and normalised. A field the provider does not use is dropped rather
/// than stored: the page shows what signs, nothing left over from another provider.
#[derive(Debug)]
pub(super) struct Fields {
    pub(super) name: String,
    pub(super) provider: ObjectStorageProvider,
    pub(super) endpoint: Option<String>,
    pub(super) region: Option<String>,
    pub(super) bucket: Option<String>,
    pub(super) addressing: ObjectAddressing,
    pub(super) source: ObjectCredentialSource,
    pub(super) access_key_id: Option<String>,
    pub(super) account: Option<String>,
    /// Only an `ambient` profile with an endpoint keeps the yes; every other drops it.
    pub(super) ambient_custom_endpoint: bool,
}

impl Fields {
    pub(super) fn validate(draft: Draft<'_>) -> Result<Self, ApiError> {
        validate_name(draft.name)?;
        let provider = draft.provider;
        if !rd_object_storage::provider_available(provider) {
            return Err(ApiError::bad_request(
                rd_object_storage::PROVIDER_UNSUPPORTED,
                "This build has no connector for this object storage provider",
            ));
        }
        if !provider.supports(draft.source) {
            return Err(ApiError::bad_request(
                "object_storage.source_unsupported",
                "The provider cannot sign with this credential source",
            ));
        }
        let s3 = provider == ObjectStorageProvider::S3;
        let endpoint = optional_text(draft.endpoint)
            .map(|value| parse_endpoint(&value))
            .transpose()?;
        let region = optional_text(draft.region).filter(|_| s3);
        if let Some(region) = &region
            && (region.len() > 64
                || !region
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'))
        {
            return Err(ApiError::bad_request(
                "object_storage.region_invalid",
                "The region is not valid",
            ));
        }
        let bucket = optional_text(draft.bucket);
        if let Some(bucket) = &bucket
            && !provider.is_valid_bucket(bucket)
        {
            return Err(ApiError::bad_request(
                "object_storage.bucket_invalid",
                "The bucket name is not valid",
            ));
        }
        let access_key_id = optional_text(draft.access_key_id)
            .filter(|_| s3 && draft.source == ObjectCredentialSource::Static);
        if s3 && draft.source == ObjectCredentialSource::Static {
            let valid = access_key_id.as_deref().is_some_and(|key| {
                key.len() <= MAX_OBJECT_ACCESS_KEY
                    && key.bytes().all(|byte| byte.is_ascii_graphic())
            });
            if !valid {
                return Err(ApiError::bad_request(
                    "object_storage.access_key_required",
                    "An access key id is required",
                ));
            }
        }
        let account =
            optional_text(draft.account).filter(|_| provider == ObjectStorageProvider::Azure);
        if provider == ObjectStorageProvider::Azure {
            match account.as_deref() {
                None => {
                    return Err(ApiError::bad_request(
                        "object_storage.account_required",
                        "An Azure storage account is required",
                    ));
                }
                Some(account) if !is_storage_account(account) => {
                    return Err(ApiError::bad_request(
                        "object_storage.account_invalid",
                        "The storage account name is not valid",
                    ));
                }
                Some(_) => {}
            }
        }
        // The machine's instance role, managed identity or Google token would go to whatever
        // host the endpoint names: only on the profile's explicit yes (RD-1190-20).
        let ambient_endpoint =
            draft.source == ObjectCredentialSource::Ambient && endpoint.is_some();
        if ambient_endpoint && !draft.ambient_custom_endpoint {
            return Err(ApiError::bad_request(
                rd_object_storage::AMBIENT_ENDPOINT_UNCONFIRMED,
                "Machine credentials go to a custom endpoint only when the profile allows it",
            ));
        }
        let addressing = if s3 {
            draft
                .addressing
                .unwrap_or_else(|| ObjectAddressing::default_for(endpoint.as_deref()))
        } else {
            ObjectAddressing::Path
        };
        Ok(Self {
            name: draft.name.trim().to_owned(),
            provider,
            addressing,
            endpoint,
            region,
            bucket,
            source: draft.source,
            access_key_id,
            account,
            ambient_custom_endpoint: ambient_endpoint,
        })
    }

    /// Whether a stored secret may go on signing for these fields: the same provider and
    /// source, and the same host — the endpoint, and the Azure account that names the host when
    /// there is none. A secret typed for one host is never sent to another (RD-1190-20); a
    /// change of host asks for it again.
    pub(super) fn keeps_secret_of(&self, stored: &ObjectStorageProfile) -> bool {
        stored.provider == self.provider
            && stored.credential_source == self.source
            && stored.endpoint == self.endpoint
            && stored.account == self.account
    }

    /// Only S3 has temporary credentials made of a key pair and a session token.
    pub(super) fn takes_session_token(&self) -> bool {
        self.provider == ObjectStorageProvider::S3 && self.source == ObjectCredentialSource::Static
    }

    pub(super) fn into_input(
        self,
        secret_ref: Option<String>,
        session_token_ref: Option<String>,
        checksums: bool,
        enabled: bool,
    ) -> rd_db::NewObjectStorageProfile {
        rd_db::NewObjectStorageProfile {
            name: self.name,
            // Per-request checksums are an S3 header; the others verify uploads their own way.
            checksums: checksums && self.provider == ObjectStorageProvider::S3,
            provider: self.provider,
            endpoint: self.endpoint,
            region: self.region,
            bucket: self.bucket,
            addressing: self.addressing,
            credential_source: self.source,
            access_key_id: self.access_key_id,
            account: self.account,
            secret_ref,
            session_token_ref,
            enabled,
            ambient_custom_endpoint: self.ambient_custom_endpoint,
        }
    }
}

/// The names of the fields an update changed, for the audit record: names only, and the secrets
/// as `secret` and `session_token` when their reference changed.
pub(super) fn changed_fields(
    before: &ObjectStorageProfile,
    after: &ObjectStorageProfile,
) -> String {
    [
        ("name", before.name != after.name),
        ("provider", before.provider != after.provider),
        ("endpoint", before.endpoint != after.endpoint),
        ("region", before.region != after.region),
        ("bucket", before.bucket != after.bucket),
        ("addressing", before.addressing != after.addressing),
        (
            "credential_source",
            before.credential_source != after.credential_source,
        ),
        ("access_key_id", before.access_key_id != after.access_key_id),
        ("account", before.account != after.account),
        ("secret", before.secret_ref != after.secret_ref),
        (
            "session_token",
            before.session_token_ref != after.session_token_ref,
        ),
        ("checksums", before.checksums != after.checksums),
        ("enabled", before.enabled != after.enabled),
        (
            "ambient_custom_endpoint",
            before.ambient_custom_endpoint != after.ambient_custom_endpoint,
        ),
    ]
    .into_iter()
    .filter_map(|(name, changed)| changed.then_some(name))
    .collect::<Vec<_>>()
    .join(" ")
}

/// Azure's storage account names: 3 to 24 lowercase letters and digits. The name becomes a
/// host label (`<account>.blob.core.windows.net`), so nothing else may reach the builder.
fn is_storage_account(name: &str) -> bool {
    (3..=24).contains(&name.len())
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

pub(super) fn secret_required() -> ApiError {
    ApiError::bad_request("object_storage.secret_required", "A secret is required")
}

/// An endpoint is an `http(s)` origin, optionally with a path: no credentials, no query.
pub(super) fn parse_endpoint(value: &str) -> Result<String, ApiError> {
    let invalid = || {
        ApiError::bad_request(
            "object_storage.endpoint_invalid",
            "The endpoint must be an http or https address",
        )
    };
    if value.len() > MAX_OBJECT_ENDPOINT {
        return Err(invalid());
    }
    let url = url::Url::parse(value).map_err(|_| invalid())?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid());
    }
    // The rule of an address the person entered (RD-1190-18): their network and a MinIO beside
    // the service are fine, a literal link-local address or one of our own listeners is refused
    // here; a name is judged when the store is opened.
    let policy = rd_plugin_host::entered_address_policy(&url);
    if rd_http::literal_address(&url).is_some_and(|address| !policy.permits(address)) {
        return Err(ApiError::bad_request(
            rd_object_storage::ENDPOINT_REFUSED,
            "The endpoint points at a link-local address or at one of rDownloader's own services",
        ));
    }
    Ok(url.as_str().trim_end_matches('/').to_owned())
}

pub(super) fn validate_secrets(
    fields: &Fields,
    secret: Option<&str>,
    token: Option<&str>,
) -> Result<(), ApiError> {
    validate_secret_value(
        secret,
        MAX_OBJECT_SECRET,
        "object_storage.secret_length",
        "secret",
    )?;
    validate_secret_value(
        token,
        MAX_OBJECT_SECRET,
        "object_storage.secret_length",
        "session token",
    )?;
    // Checked on the way in, so a pasted connection string or the wrong file is named here
    // and not at the first transfer. The value itself never reaches the answer.
    if let Some(secret) = secret.filter(|_| fields.source.stores_secret())
        && let Some(code) =
            rd_object_storage::secret_problem(fields.provider, fields.source, secret)
    {
        return Err(ApiError::bad_request(
            code,
            "The secret does not have the form this provider signs with",
        ));
    }
    Ok(())
}
