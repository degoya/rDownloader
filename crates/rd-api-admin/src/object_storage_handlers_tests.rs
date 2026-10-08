//! Unit tests of the object storage profile validation (RD-150-04, RD-150-05).

use chrono::Utc;
use rd_core::{
    ObjectAddressing, ObjectCredentialSource, ObjectStorageProfile, ObjectStorageProfileId,
    ObjectStorageProvider,
};

use super::fields::{Draft, Fields, parse_endpoint, validate_secrets};

fn draft(source: ObjectCredentialSource) -> Draft<'static> {
    Draft {
        name: "MinIO",
        provider: ObjectStorageProvider::S3,
        endpoint: Some("https://minio.example:9000/".to_owned()),
        region: Some("eu-central-1".to_owned()),
        bucket: Some("media-bucket".to_owned()),
        addressing: None,
        source,
        access_key_id: Some("AKIDEXAMPLE".to_owned()),
        account: Some("ignored".to_owned()),
        ambient_custom_endpoint: false,
    }
}

fn azure(source: ObjectCredentialSource) -> Draft<'static> {
    Draft {
        name: "Blob",
        provider: ObjectStorageProvider::Azure,
        endpoint: None,
        region: Some("westeurope".to_owned()),
        bucket: Some("media-2026".to_owned()),
        addressing: Some(ObjectAddressing::VirtualHost),
        source,
        access_key_id: Some("left-over".to_owned()),
        account: Some("mediaarchive".to_owned()),
        ambient_custom_endpoint: false,
    }
}

#[test]
fn an_endpoint_is_an_origin_without_credentials() {
    assert_eq!(
        parse_endpoint("https://minio.example:9000/").expect("endpoint"),
        "https://minio.example:9000"
    );
    assert!(parse_endpoint("http://127.0.0.1:9000").is_ok());
    // A key pair in the endpoint would be stored in clear and shown on the page.
    assert!(parse_endpoint("https://AKID:secret@minio.example").is_err());
    assert!(parse_endpoint("ftp://minio.example").is_err());
    assert!(parse_endpoint("https://minio.example/?x=1").is_err());
    assert!(parse_endpoint("minio.example").is_err());
}

#[test]
fn a_custom_endpoint_defaults_to_path_style() {
    let fields = Fields::validate(draft(ObjectCredentialSource::Static)).expect("fields");
    assert_eq!(fields.addressing, ObjectAddressing::Path);
    let mut aws = draft(ObjectCredentialSource::Static);
    aws.endpoint = None;
    let fields = Fields::validate(aws).expect("fields");
    assert_eq!(fields.addressing, ObjectAddressing::VirtualHost);
}

#[test]
fn a_static_profile_needs_its_key_id_and_others_drop_it() {
    let mut missing = draft(ObjectCredentialSource::Static);
    missing.access_key_id = None;
    assert!(Fields::validate(missing).is_err());
    let mut ambient = draft(ObjectCredentialSource::Ambient);
    ambient.ambient_custom_endpoint = true;
    let ambient = Fields::validate(ambient).expect("fields");
    assert_eq!(ambient.access_key_id, None);
}

#[test]
fn an_azure_profile_needs_its_account_and_keeps_nothing_of_s3() {
    let fields =
        Fields::validate(azure(ObjectCredentialSource::SharedAccessSignature)).expect("fields");
    assert_eq!(fields.account.as_deref(), Some("mediaarchive"));
    assert_eq!(fields.region, None);
    assert_eq!(fields.access_key_id, None);
    assert_eq!(fields.addressing, ObjectAddressing::Path);
    assert!(!fields.takes_session_token());

    let mut missing = azure(ObjectCredentialSource::Anonymous);
    missing.account = None;
    assert!(Fields::validate(missing).is_err());
    let mut host = azure(ObjectCredentialSource::Anonymous);
    host.account = Some("evil.example.com/x".to_owned());
    assert!(Fields::validate(host).is_err());
    // Containers follow Azure's rules, not S3's.
    let mut dotted = azure(ObjectCredentialSource::Anonymous);
    dotted.bucket = Some("media.bucket".to_owned());
    assert!(Fields::validate(dotted).is_err());
}

#[test]
fn a_signature_source_belongs_to_azure_alone() {
    assert!(Fields::validate(draft(ObjectCredentialSource::SharedAccessSignature)).is_err());
    let mut google = draft(ObjectCredentialSource::SharedAccessSignature);
    google.provider = ObjectStorageProvider::Gcs;
    assert!(Fields::validate(google).is_err());
    let s3 = Fields::validate(draft(ObjectCredentialSource::Anonymous)).expect("fields");
    assert_eq!(s3.account, None);
}

#[test]
fn a_secret_of_the_wrong_shape_is_named_before_it_is_stored() {
    let fields =
        Fields::validate(azure(ObjectCredentialSource::SharedAccessSignature)).expect("fields");
    assert!(validate_secrets(&fields, Some("sv=2024-11-04&sp=r&sig=abc"), None).is_ok());
    assert!(validate_secrets(&fields, Some("DefaultEndpointsProtocol=https"), None).is_err());
    let mut google = draft(ObjectCredentialSource::Static);
    google.provider = ObjectStorageProvider::Gcs;
    google.bucket = Some("media_bucket".to_owned());
    let fields = Fields::validate(google).expect("fields");
    assert_eq!(fields.access_key_id, None);
    assert!(validate_secrets(&fields, Some("-----BEGIN PRIVATE KEY-----"), None).is_err());
}

#[test]
fn names_the_service_would_refuse_are_refused_here() {
    let mut bucket = draft(ObjectCredentialSource::Anonymous);
    bucket.bucket = Some("Not_A_Bucket".to_owned());
    assert!(Fields::validate(bucket).is_err());
    let mut region = draft(ObjectCredentialSource::Anonymous);
    region.region = Some("eu central".to_owned());
    assert!(Fields::validate(region).is_err());
}

fn code(draft: Draft<'_>) -> Option<String> {
    Fields::validate(draft)
        .err()
        .map(|error| error.code().to_owned())
}

/// RD-1190-20: the machine's own credentials go to a custom endpoint only on the profile's
/// explicit yes, and the yes is kept only where it means something.
#[test]
fn an_ambient_profile_names_an_endpoint_only_with_the_opt_in() {
    assert_eq!(
        code(draft(ObjectCredentialSource::Ambient)).as_deref(),
        Some(rd_object_storage::AMBIENT_ENDPOINT_UNCONFIRMED)
    );
    let mut confirmed = draft(ObjectCredentialSource::Ambient);
    confirmed.ambient_custom_endpoint = true;
    assert!(
        Fields::validate(confirmed)
            .expect("fields")
            .ambient_custom_endpoint
    );
    // The provider's own service needs no yes, and keeps none.
    let mut aws = draft(ObjectCredentialSource::Ambient);
    aws.endpoint = None;
    aws.ambient_custom_endpoint = true;
    assert!(
        !Fields::validate(aws)
            .expect("fields")
            .ambient_custom_endpoint
    );
    let mut keyed = draft(ObjectCredentialSource::Static);
    keyed.ambient_custom_endpoint = true;
    assert!(
        !Fields::validate(keyed)
            .expect("fields")
            .ambient_custom_endpoint
    );
}

fn stored(fields: &Fields) -> ObjectStorageProfile {
    ObjectStorageProfile {
        id: ObjectStorageProfileId::new(),
        name: fields.name.clone(),
        provider: fields.provider,
        endpoint: fields.endpoint.clone(),
        region: fields.region.clone(),
        bucket: fields.bucket.clone(),
        addressing: fields.addressing,
        credential_source: fields.source,
        access_key_id: fields.access_key_id.clone(),
        account: fields.account.clone(),
        secret_ref: Some("vault://secret".to_owned()),
        session_token_ref: Some("vault://token".to_owned()),
        has_secret: true,
        has_session_token: true,
        checksums: true,
        enabled: true,
        created_at: Utc::now(),
        updated_at: Utc::now(),
        ambient_custom_endpoint: false,
    }
}

/// RD-1190-20: a stored secret signs only for the host it was typed for. Before, an update
/// that left the secret out kept it whatever endpoint the profile now named.
#[test]
fn a_stored_secret_stays_with_its_host() {
    let fields = Fields::validate(draft(ObjectCredentialSource::Static)).expect("fields");
    let profile = stored(&fields);
    assert!(fields.keeps_secret_of(&profile), "nothing changed");
    let mut moved = draft(ObjectCredentialSource::Static);
    moved.endpoint = Some("https://collector.example".to_owned());
    assert!(
        !Fields::validate(moved)
            .expect("fields")
            .keeps_secret_of(&profile)
    );
    let mut own_service = draft(ObjectCredentialSource::Static);
    own_service.endpoint = None;
    assert!(
        !Fields::validate(own_service)
            .expect("fields")
            .keeps_secret_of(&profile)
    );
    let mut renamed = draft(ObjectCredentialSource::Static);
    renamed.name = "MinIO 2";
    renamed.bucket = Some("other-bucket".to_owned());
    assert!(
        Fields::validate(renamed)
            .expect("fields")
            .keeps_secret_of(&profile)
    );

    // Without an endpoint the Azure account names the host.
    let blob =
        Fields::validate(azure(ObjectCredentialSource::SharedAccessSignature)).expect("fields");
    let profile = stored(&blob);
    let mut other_account = azure(ObjectCredentialSource::SharedAccessSignature);
    other_account.account = Some("otheraccount".to_owned());
    assert!(
        !Fields::validate(other_account)
            .expect("fields")
            .keeps_secret_of(&profile)
    );
    let key = Fields::validate(azure(ObjectCredentialSource::Static)).expect("fields");
    assert!(
        !key.keeps_secret_of(&profile),
        "an account key is no signature"
    );
}
