//! Unit tests of the object storage profile validation (RD-150-04, RD-150-05).

use rd_core::{ObjectAddressing, ObjectCredentialSource, ObjectStorageProvider};

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
    let ambient = Fields::validate(draft(ObjectCredentialSource::Ambient)).expect("fields");
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
