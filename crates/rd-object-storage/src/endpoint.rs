//! The address rule a profile's endpoint keeps to (RD-1190-18).
//!
//! An endpoint is an address the person entered, and their own MinIO in the local network or
//! beside the service is an ordinary setup. So it keeps to the rule a webhook and a plugin
//! request to an entered address keep to (`rd_plugin_host::entered_address_policy`): the
//! person's network and this machine's loopback are fine, link-local — a cloud's metadata
//! endpoint among it — never, and neither is one of rDownloader's own listeners. The endpoint
//! is checked before a store is opened, against its literal address or every address its name
//! resolves to, and again when a connection is made: the store's client resolves through
//! [`GuardedDns`], so a name that answers differently the second time is refused there.
//!
//! What stays open: through a proxy the proxy resolves the name, and `object_store` follows a
//! redirect to a literal address without asking the resolver.

use std::sync::Arc;

use object_store::client::{DnsError, DnsFuture, DnsResolver};
use rd_core::{Failure, FailureKind, ObjectStorageProfile};

/// Stable code of an endpoint the address rule refuses.
pub const ENDPOINT_REFUSED: &str = "object_storage.endpoint_refused";

/// The resolver a profile's store connects through, once its endpoint passed the rule; `None`
/// for a profile on its provider's own service, or with an endpoint that does not parse, which
/// opening the store reports.
pub(crate) async fn guard(
    profile: &ObjectStorageProfile,
) -> Result<Option<Arc<dyn DnsResolver>>, Failure> {
    let Some(url) = profile
        .endpoint
        .as_deref()
        .and_then(|endpoint| url::Url::parse(endpoint).ok())
    else {
        return Ok(None);
    };
    let policy = rd_plugin_host::entered_address_policy(&url);
    // A name without an address is no refusal: the connection fails on its own, as it did.
    if let Err(rd_http::TargetRefusal::Refused(_)) =
        rd_http::check_target(&policy, &rd_http::SystemLookup, &url).await
    {
        return Err(refused());
    }
    Ok(Some(Arc::new(GuardedDns(policy))))
}

/// The refusal of an endpoint the rule does not permit.
fn refused() -> Failure {
    Failure::coded(
        FailureKind::Permanent,
        ENDPOINT_REFUSED,
        "The endpoint points at a link-local address or at one of rDownloader's own services",
    )
}

/// Resolves like the system does and refuses a name with any address the rule does not permit,
/// at the moment the connection is made.
#[derive(Debug)]
pub(crate) struct GuardedDns(rd_http::AddressPolicy);

impl DnsResolver for GuardedDns {
    fn resolve(&self, host: &str) -> DnsFuture {
        let policy = self.0.clone();
        let host = host.to_owned();
        Box::pin(async move {
            // Port 0: the transport sets the endpoint's port on every address it is handed.
            let addresses =
                rd_http::connect_addresses(&policy, &rd_http::SystemLookup, &host, 0).await?;
            Ok::<_, DnsError>(addresses.into_iter().map(|address| address.ip()).collect())
        })
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use object_store::client::DnsResolver as _;
    use rd_core::{
        ObjectAddressing, ObjectCredentialSource, ObjectStorageProfile, ObjectStorageProfileId,
        ObjectStorageProvider,
    };

    use super::{ENDPOINT_REFUSED, GuardedDns, guard};

    fn profile(endpoint: &str) -> ObjectStorageProfile {
        ObjectStorageProfile {
            id: ObjectStorageProfileId::new(),
            name: "endpoint".to_owned(),
            provider: ObjectStorageProvider::S3,
            endpoint: Some(endpoint.to_owned()),
            region: None,
            bucket: Some("media-bucket".to_owned()),
            addressing: ObjectAddressing::Path,
            credential_source: ObjectCredentialSource::Anonymous,
            access_key_id: None,
            account: None,
            ambient_custom_endpoint: false,
            secret_ref: None,
            session_token_ref: None,
            has_secret: false,
            has_session_token: false,
            checksums: false,
            enabled: true,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    /// RD-1190-18: a profile pointed at a cloud's metadata endpoint, or at rDownloader's own
    /// Click'n'Load listener, is refused before a store exists; the person's own MinIO in the
    /// local network or beside the service is not.
    #[tokio::test]
    async fn the_metadata_endpoint_and_our_own_listeners_are_refused() {
        for refused in [
            "http://169.254.169.254",
            "http://169.254.169.254/latest/meta-data",
            "http://[fe80::1]:9000",
            "http://127.0.0.1:9666",
        ] {
            let answer = guard(&profile(refused)).await;
            assert_eq!(
                answer.err().and_then(|failure| failure.code).as_deref(),
                Some(ENDPOINT_REFUSED),
                "{refused}"
            );
        }
        for permitted in [
            "http://192.168.1.20:9000",
            "http://127.0.0.1:9000",
            "https://10.0.0.5",
        ] {
            assert!(
                matches!(guard(&profile(permitted)).await, Ok(Some(_))),
                "{permitted}"
            );
        }
    }

    /// The resolver a guarded store connects through refuses what the rule refuses, so a
    /// name that passed the check cannot answer with the metadata endpoint a moment later.
    #[tokio::test]
    async fn the_connection_time_resolver_keeps_to_the_rule() {
        let policy = rd_plugin_host::entered_address_policy(
            &url::Url::parse("http://minio.example:9000").expect("url"),
        );
        let resolver = GuardedDns(policy);
        assert!(resolver.resolve("169.254.169.254").await.is_err());
        assert_eq!(
            resolver.resolve("192.168.1.20").await.expect("permitted"),
            vec![std::net::IpAddr::from([192, 168, 1, 20])]
        );
    }
}
