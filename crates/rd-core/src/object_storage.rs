//! Object storage as a download source and an upload target (RD-150-04, RD-150-05).
//!
//! A *profile* is where a bucket lives and how to sign for it: the endpoint, the region, the
//! addressing style and the credentials. A *link* (`s3://bucket/key`) names only the bucket
//! and the object, never a host, so a link pasted from a web page cannot point the transfer at
//! an address nobody configured — the endpoint always comes from a profile somebody created.
//!
//! Azure Blob Storage (`az://container/blob`) and Google Cloud Storage (`gs://bucket/object`)
//! have the same shape: another scheme, another profile provider, the same link, listing and
//! resume contract. An Azure container lives in a storage account, which the profile names;
//! the link still names only the container.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use url::Url;
use utoipa::ToSchema;

use crate::ObjectStorageProfileId;

/// Provider name stored on LinkGrabber candidates for every object storage link.
pub const OBJECT_STORAGE_PROVIDER: &str = "object_storage";
/// Longest object key S3 accepts, in bytes.
pub const MAX_OBJECT_KEY: usize = 1024;
/// Longest secret accepted: an S3 secret key or session token, an Azure account key or shared
/// access signature, a Google service account key (a JSON document of about 2.4 KiB).
pub const MAX_OBJECT_SECRET: usize = 8 * 1024;
/// Longest access key id accepted.
pub const MAX_OBJECT_ACCESS_KEY: usize = 256;
/// Longest endpoint URL accepted.
pub const MAX_OBJECT_ENDPOINT: usize = 2048;

/// Which object storage service a profile talks to.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Hash, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ObjectStorageProvider {
    /// Amazon S3 and every service that speaks its API (MinIO, Ceph RGW, Garage, R2, …).
    #[default]
    S3,
    /// Azure Blob Storage, and Azurite for tests.
    Azure,
    /// Google Cloud Storage.
    Gcs,
}

impl ObjectStorageProvider {
    /// Stable snake_case name used in storage and on the wire.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::S3 => "s3",
            Self::Azure => "azure",
            Self::Gcs => "gcs",
        }
    }

    /// The URL scheme of this provider's links: the ones `az`, `gsutil` and `aws` print.
    #[must_use]
    pub const fn scheme(self) -> &'static str {
        match self {
            Self::S3 => "s3",
            Self::Azure => "az",
            Self::Gcs => "gs",
        }
    }

    /// The provider a link scheme names.
    #[must_use]
    pub fn from_scheme(scheme: &str) -> Option<Self> {
        match scheme {
            "s3" => Some(Self::S3),
            "az" => Some(Self::Azure),
            "gs" => Some(Self::Gcs),
            _ => None,
        }
    }

    /// Whether the service would accept `name` for a bucket (a container, on Azure).
    #[must_use]
    pub fn is_valid_bucket(self, name: &str) -> bool {
        match self {
            Self::S3 => is_valid_bucket_name(name),
            Self::Azure => is_valid_container_name(name),
            Self::Gcs => is_valid_gcs_bucket_name(name),
        }
    }

    /// Whether a profile of this provider can sign with credentials from `source`.
    #[must_use]
    pub const fn supports(self, source: ObjectCredentialSource) -> bool {
        match source {
            ObjectCredentialSource::Static
            | ObjectCredentialSource::Ambient
            | ObjectCredentialSource::Anonymous => true,
            ObjectCredentialSource::SharedAccessSignature => matches!(self, Self::Azure),
        }
    }
}

/// How the bucket appears in the request URL.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ObjectAddressing {
    /// `https://endpoint/bucket/key` — what MinIO and most self-hosted services expect.
    Path,
    /// `https://bucket.endpoint/key` — what AWS prefers and new AWS buckets require.
    VirtualHost,
}

impl ObjectAddressing {
    /// The style a profile gets when the request names none: AWS wants virtual hosts, a
    /// custom endpoint is almost always a self-hosted service that wants paths.
    #[must_use]
    pub const fn default_for(endpoint: Option<&str>) -> Self {
        if endpoint.is_some() {
            Self::Path
        } else {
            Self::VirtualHost
        }
    }
}

/// Where a profile's credentials come from.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ObjectCredentialSource {
    /// A key in the vault: an S3 access key id and secret key (and optionally a session
    /// token), an Azure account key, or a Google service account key.
    Static,
    /// The machine's own credentials, nothing stored: for S3 the `AWS_*` environment, a web
    /// identity token, the container (ECS/EKS) endpoint or the instance metadata service; for
    /// Azure a service principal, workload or managed identity; for Google the application
    /// default credentials or the metadata server.
    Ambient,
    /// Unsigned requests, for public buckets.
    Anonymous,
    /// An Azure shared access signature in the vault, scoped and time-limited by whoever
    /// issued it. Replacing an expired one on the profile keeps every partial download.
    SharedAccessSignature,
}

impl ObjectCredentialSource {
    /// Stable snake_case name used in storage.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Static => "static",
            Self::Ambient => "ambient",
            Self::Anonymous => "anonymous",
            Self::SharedAccessSignature => "shared_access_signature",
        }
    }

    /// Whether the source signs with a secret stored on the profile.
    #[must_use]
    pub const fn stores_secret(self) -> bool {
        matches!(self, Self::Static | Self::SharedAccessSignature)
    }
}

/// A configured object storage endpoint with its credentials.
///
/// Secret values live in the secret store; this struct carries only opaque `vault://`
/// references, and those are never serialized.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct ObjectStorageProfile {
    pub id: ObjectStorageProfileId,
    pub name: String,
    pub provider: ObjectStorageProvider,
    /// `None` is the provider's own service: AWS S3 for the region, the account's
    /// `blob.core.windows.net` host, `storage.googleapis.com`.
    pub endpoint: Option<String>,
    /// S3 only.
    pub region: Option<String>,
    /// A bucket this profile is bound to: links into it use this profile.
    pub bucket: Option<String>,
    /// S3 only; the other providers have one addressing style.
    pub addressing: ObjectAddressing,
    pub credential_source: ObjectCredentialSource,
    /// Not a secret — AWS documents the key id as an identifier, like a user name. S3 only.
    pub access_key_id: Option<String>,
    /// The Azure storage account the containers live in; an identifier, not a secret.
    pub account: Option<String>,
    #[serde(skip_serializing)]
    #[schema(ignore)]
    pub secret_ref: Option<String>,
    #[serde(skip_serializing)]
    #[schema(ignore)]
    pub session_token_ref: Option<String>,
    pub has_secret: bool,
    pub has_session_token: bool,
    /// Whether uploads carry a SHA-256 checksum per request, which the service verifies on
    /// receipt. Off for the few compatible services that refuse the header. S3 only.
    pub checksums: bool,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Why no profile could be chosen for a link.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProfileChoiceError {
    /// Nothing is configured that could serve the bucket.
    None,
    /// More than one profile could, and guessing would sign with the wrong credentials.
    Ambiguous,
    /// The link named a profile that is switched off.
    Disabled,
}

/// Picks the profile that serves `address`.
///
/// In this order: the profile the link names in its user part (by id or by name); the
/// enabled profile bound to the link's bucket; the one enabled profile bound to no bucket.
/// Two candidates at the same step are refused rather than decided by order, because the
/// wrong choice sends one account's credentials to another account's endpoint.
pub fn select_profile<'a>(
    profiles: &'a [ObjectStorageProfile],
    address: &ObjectAddress,
) -> Result<&'a ObjectStorageProfile, ProfileChoiceError> {
    let same_provider = || {
        profiles
            .iter()
            .filter(move |profile| profile.provider == address.provider)
    };
    if let Some(hint) = address.profile.as_deref() {
        let named = same_provider()
            .find(|profile| profile.id.to_string() == hint)
            .or_else(|| same_provider().find(|profile| profile.name == hint))
            .ok_or(ProfileChoiceError::None)?;
        return if named.enabled {
            Ok(named)
        } else {
            Err(ProfileChoiceError::Disabled)
        };
    }
    let bound: Vec<_> = same_provider()
        .filter(|profile| {
            profile.enabled && profile.bucket.as_deref() == Some(address.bucket.as_str())
        })
        .collect();
    match bound.as_slice() {
        [only] => return Ok(*only),
        [] => {}
        _ => return Err(ProfileChoiceError::Ambiguous),
    }
    let general: Vec<_> = same_provider()
        .filter(|profile| profile.enabled && profile.bucket.is_none())
        .collect();
    match general.as_slice() {
        [only] => Ok(*only),
        [] => Err(ProfileChoiceError::None),
        _ => Err(ProfileChoiceError::Ambiguous),
    }
}

/// What an object storage link addresses.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObjectAddress {
    pub provider: ObjectStorageProvider,
    /// The profile the link names in its user part, percent-decoded. A password in the same
    /// place is never read.
    pub profile: Option<String>,
    pub bucket: String,
    /// Percent-decoded key without a leading `/`. Empty or ending in `/` addresses a prefix.
    pub key: String,
}

impl ObjectAddress {
    /// Parses `s3://[profile@]bucket[/key]`, and the same with `az://` and `gs://`.
    ///
    /// Refuses a port, a bucket name the service would refuse (an IP-address-shaped name in
    /// particular, so `s3://169.254.169.254/` is not a way to name the metadata service), and
    /// a key with empty, `.` or `..` segments, which no store can hold under that name and
    /// which would otherwise turn into a path on this disk.
    #[must_use]
    pub fn parse(url: &Url) -> Option<Self> {
        let provider = ObjectStorageProvider::from_scheme(url.scheme())?;
        if url.port().is_some() || url.query().is_some() {
            return None;
        }
        let bucket = decode(url.host_str()?)?;
        if !provider.is_valid_bucket(&bucket) {
            return None;
        }
        let profile = decode(url.username())?;
        let key = decode(url.path())?;
        let key = key.strip_prefix('/').unwrap_or(&key).to_owned();
        if !is_valid_key(&key) {
            return None;
        }
        Some(Self {
            provider,
            profile: (!profile.is_empty()).then_some(profile),
            bucket,
            key,
        })
    }

    /// Whether the link names a prefix rather than one object.
    #[must_use]
    pub fn is_prefix(&self) -> bool {
        self.key.is_empty() || self.key.ends_with('/')
    }

    /// Last key segment, the default file name.
    #[must_use]
    pub fn file_name(&self) -> Option<String> {
        self.key
            .rsplit('/')
            .find(|segment| !segment.is_empty())
            .map(str::to_owned)
    }

    /// The link with `relative` appended to the key, for one file of a reviewed listing.
    #[must_use]
    pub fn child(&self, relative: &str) -> Self {
        let mut key = self.key.clone();
        if !key.is_empty() && !key.ends_with('/') {
            key.push('/');
        }
        key.push_str(relative.trim_start_matches('/'));
        Self {
            key,
            ..self.clone()
        }
    }

    /// The canonical link: no password, the key percent-encoded segment by segment.
    #[must_use]
    pub fn url(&self) -> Option<Url> {
        let mut url = Url::parse(&format!("{}://{}", self.provider.scheme(), self.bucket)).ok()?;
        if let Some(profile) = &self.profile {
            url.set_username(profile).ok()?;
        }
        {
            let mut segments = url.path_segments_mut().ok()?;
            segments.clear();
            for segment in self.key.split('/') {
                segments.push(segment);
            }
        }
        Some(url)
    }
}

fn decode(value: &str) -> Option<String> {
    percent_encoding::percent_decode_str(value)
        .decode_utf8()
        .ok()
        .map(std::borrow::Cow::into_owned)
}

/// S3's bucket naming rules, which the compatible services follow.
#[must_use]
pub fn is_valid_bucket_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    if !(3..=63).contains(&bytes.len()) {
        return false;
    }
    let allowed = |byte: &u8| byte.is_ascii_lowercase() || byte.is_ascii_digit();
    if !bytes.first().is_some_and(allowed) || !bytes.last().is_some_and(allowed) {
        return false;
    }
    if !bytes
        .iter()
        .all(|byte| allowed(byte) || *byte == b'.' || *byte == b'-')
    {
        return false;
    }
    if name.contains("..") || name.starts_with("xn--") {
        return false;
    }
    // `192.168.1.10` is refused by S3 and by every virtual-host resolver.
    name.parse::<std::net::Ipv4Addr>().is_err()
}

/// Azure's container naming rules: 3 to 63 lowercase letters, digits and single hyphens,
/// beginning and ending with a letter or digit.
#[must_use]
pub fn is_valid_container_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    (3..=63).contains(&bytes.len())
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
        && !name.starts_with('-')
        && !name.ends_with('-')
        && !name.contains("--")
}

/// Google Cloud Storage's bucket naming rules: lowercase letters, digits, `-`, `_` and `.`,
/// beginning and ending with a letter or digit; 3 to 63 characters, or up to 222 when dots
/// split the name into DNS labels of at most 63; never an IP address, never `goog…`.
#[must_use]
pub fn is_valid_gcs_bucket_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    let limit = if name.contains('.') { 222 } else { 63 };
    let alphanumeric = |byte: &u8| byte.is_ascii_lowercase() || byte.is_ascii_digit();
    (3..=limit).contains(&bytes.len())
        && bytes.first().is_some_and(alphanumeric)
        && bytes.last().is_some_and(alphanumeric)
        && bytes
            .iter()
            .all(|byte| alphanumeric(byte) || matches!(*byte, b'-' | b'_' | b'.'))
        && name
            .split('.')
            .all(|label| !label.is_empty() && label.len() <= 63)
        && !name.starts_with("goog")
        && !name.contains("google")
        && name.parse::<std::net::Ipv4Addr>().is_err()
}

fn is_valid_key(key: &str) -> bool {
    if key.len() > MAX_OBJECT_KEY || key.contains('\0') || key.contains('\\') {
        return false;
    }
    let body = key.strip_suffix('/').unwrap_or(key);
    body.is_empty()
        || body
            .split('/')
            .all(|segment| !matches!(segment, "" | "." | ".."))
}

#[cfg(test)]
#[path = "object_storage_tests.rs"]
mod tests;
