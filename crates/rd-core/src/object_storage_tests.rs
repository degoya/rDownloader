use chrono::Utc;
use url::Url;

use super::{
    ObjectAddress, ObjectAddressing, ObjectCredentialSource, ObjectStorageProfile,
    ObjectStorageProvider, ProfileChoiceError, is_valid_bucket_name, is_valid_container_name,
    is_valid_gcs_bucket_name, select_profile,
};
use crate::ObjectStorageProfileId;

fn address(input: &str) -> Option<ObjectAddress> {
    ObjectAddress::parse(&Url::parse(input).expect("url"))
}

fn profile(name: &str, bucket: Option<&str>) -> ObjectStorageProfile {
    ObjectStorageProfile {
        id: ObjectStorageProfileId::new(),
        name: name.to_owned(),
        provider: ObjectStorageProvider::S3,
        endpoint: None,
        region: None,
        bucket: bucket.map(str::to_owned),
        addressing: ObjectAddressing::VirtualHost,
        credential_source: ObjectCredentialSource::Anonymous,
        access_key_id: None,
        account: None,
        secret_ref: Some("vault://secret".to_owned()),
        session_token_ref: None,
        has_secret: true,
        has_session_token: false,
        checksums: true,
        enabled: true,
        created_at: Utc::now(),
        updated_at: Utc::now(),
        ambient_custom_endpoint: false,
    }
}

#[test]
fn a_link_splits_into_bucket_key_and_profile() {
    let parsed = address("s3://Archive%20Box@media-bucket/shows/a%20b.mkv").expect("link");
    assert_eq!(parsed.bucket, "media-bucket");
    assert_eq!(parsed.key, "shows/a b.mkv");
    assert_eq!(parsed.profile.as_deref(), Some("Archive Box"));
    assert!(!parsed.is_prefix());
    assert_eq!(parsed.file_name().as_deref(), Some("a b.mkv"));
    assert!(
        address("s3://media-bucket/shows/")
            .expect("prefix")
            .is_prefix()
    );
    assert!(address("s3://media-bucket").expect("bucket").is_prefix());
}

#[test]
fn a_password_in_the_link_never_survives_the_canonical_form() {
    let parsed = address("s3://AKIDEXAMPLE:wJalrXUtnFEMI@media-bucket/a.bin").expect("link");
    let canonical = parsed.url().expect("url");
    assert!(!canonical.as_str().contains("wJalrXUtnFEMI"));
    assert_eq!(canonical.password(), None);
}

#[test]
fn a_link_cannot_name_a_host_a_port_or_a_traversal() {
    // The endpoint comes from a profile; a link that could name a host or a port would
    // be a way to aim the signed request somewhere nobody configured.
    assert!(address("s3://169.254.169.254/latest/meta-data").is_none());
    assert!(address("s3://media-bucket:9000/a.bin").is_none());
    // A `..` never reaches the key: the URL parser resolves dot segments, percent-encoded
    // ones included, so these are the key `etc/passwd` inside the same bucket — not a way
    // out of it, nor out of the download folder the key later names a file in.
    for traversal in [
        "s3://media-bucket/a/../../etc/passwd",
        "s3://media-bucket/a/%2e%2e/%2E%2E/etc/passwd",
    ] {
        assert_eq!(
            address(traversal).map(|address| address.key),
            Some("etc/passwd".to_owned()),
            "{traversal}"
        );
    }
    assert!(address("s3://media-bucket/a//b").is_none());
    assert!(address("s3://media-bucket/a.bin?x-id=GetObject").is_none());
    assert!(address("https://media-bucket/a.bin").is_none());
}

#[test]
fn bucket_names_follow_the_service_rules() {
    assert!(is_valid_bucket_name("my.bucket-01"));
    assert!(!is_valid_bucket_name("ab"));
    assert!(!is_valid_bucket_name("Upper"));
    assert!(!is_valid_bucket_name("-leading"));
    assert!(!is_valid_bucket_name("a..b"));
    assert!(!is_valid_bucket_name("10.0.0.1"));
    assert!(!is_valid_bucket_name(&"a".repeat(64)));
    assert!(is_valid_container_name("media-2026"));
    assert!(!is_valid_container_name("media-"));
    assert!(!is_valid_container_name("Media"));
    assert!(is_valid_gcs_bucket_name("media_bucket"));
    assert!(is_valid_gcs_bucket_name(&format!(
        "{}.example",
        "a".repeat(63)
    )));
    assert!(!is_valid_gcs_bucket_name(&"a".repeat(64)));
    assert!(!is_valid_gcs_bucket_name("_media"));
    assert!(!is_valid_gcs_bucket_name("a..b"));
    assert!(!is_valid_gcs_bucket_name("goog-media"));
}

#[test]
fn azure_and_google_links_follow_their_own_naming_rules() {
    let azure = address("az://archive@media-2026/shows/e01.mkv").expect("azure link");
    assert_eq!(azure.provider, ObjectStorageProvider::Azure);
    assert_eq!(azure.bucket, "media-2026");
    assert_eq!(azure.profile.as_deref(), Some("archive"));
    // Containers take no dots and no doubled hyphens.
    assert!(address("az://my.container/a").is_none());
    assert!(address("az://a--b/a").is_none());
    let google = address("gs://media_bucket.example.com/a%20b.bin").expect("gcs link");
    assert_eq!(google.provider, ObjectStorageProvider::Gcs);
    assert_eq!(google.key, "a b.bin");
    assert_eq!(
        google.url().expect("url").as_str(),
        "gs://media_bucket.example.com/a%20b.bin"
    );
    assert!(address("gs://169.254.169.254/computeMetadata/v1/").is_none());
    assert!(address("gs://google-things/a").is_none());
    assert!(address("gs://media:443/a").is_none());
    // Dot segments resolve inside the bucket, as for S3 above.
    assert_eq!(
        address("gs://media/a/../b").map(|address| address.key),
        Some("b".to_owned())
    );
}

#[test]
fn shared_access_signatures_are_an_azure_thing() {
    use ObjectCredentialSource::{Ambient, SharedAccessSignature, Static};
    assert!(ObjectStorageProvider::Azure.supports(SharedAccessSignature));
    assert!(!ObjectStorageProvider::S3.supports(SharedAccessSignature));
    assert!(!ObjectStorageProvider::Gcs.supports(SharedAccessSignature));
    assert!(ObjectStorageProvider::Gcs.supports(Static));
    assert!(ObjectStorageProvider::Gcs.supports(Ambient));
    assert!(SharedAccessSignature.stores_secret());
    assert!(!Ambient.stores_secret());
}

#[test]
fn a_profile_serves_only_links_of_its_own_provider() {
    let mut azure = profile("blob", None);
    azure.provider = ObjectStorageProvider::Azure;
    let profiles = [azure];
    assert!(select_profile(&profiles, &address("az://media/a").expect("link")).is_ok());
    assert_eq!(
        select_profile(&profiles, &address("s3://media/a").expect("link")).err(),
        Some(ProfileChoiceError::None)
    );
}

#[test]
fn a_child_is_appended_below_the_prefix() {
    let base = address("s3://media-bucket/shows/").expect("prefix");
    assert_eq!(base.child("season 1/e01.mkv").key, "shows/season 1/e01.mkv");
    let root = address("s3://media-bucket").expect("bucket");
    assert_eq!(root.child("a.bin").key, "a.bin");
    assert_eq!(
        base.child("season 1/e01.mkv").url().expect("url").as_str(),
        "s3://media-bucket/shows/season%201/e01.mkv"
    );
}

#[test]
fn a_bound_profile_beats_the_general_one() {
    let profiles = [
        profile("general", None),
        profile("media", Some("media-bucket")),
    ];
    let chosen =
        select_profile(&profiles, &address("s3://media-bucket/a").expect("link")).expect("profile");
    assert_eq!(chosen.name, "media");
    let chosen =
        select_profile(&profiles, &address("s3://other-bucket/a").expect("link")).expect("profile");
    assert_eq!(chosen.name, "general");
}

#[test]
fn two_general_profiles_are_refused_rather_than_guessed() {
    let profiles = [profile("aws", None), profile("minio", None)];
    assert_eq!(
        select_profile(&profiles, &address("s3://some-bucket/a").expect("link")).err(),
        Some(ProfileChoiceError::Ambiguous)
    );
    // Naming one settles it.
    let chosen = select_profile(
        &profiles,
        &address("s3://minio@some-bucket/a").expect("link"),
    )
    .expect("profile");
    assert_eq!(chosen.name, "minio");
    let by_id = format!("s3://{}@some-bucket/a", profiles[0].id);
    let chosen = select_profile(&profiles, &address(&by_id).expect("link")).expect("profile");
    assert_eq!(chosen.name, "aws");
}

/// RD-1190-20: naming a bound profile in the link does not widen its binding — before, the
/// hint picked the profile whatever bucket the link named.
#[test]
fn a_named_profile_serves_only_its_bound_bucket() {
    let profiles = [profile("media", Some("media-bucket"))];
    let chosen = select_profile(
        &profiles,
        &address("s3://media@media-bucket/a").expect("link"),
    )
    .expect("profile");
    assert_eq!(chosen.name, "media");
    assert_eq!(
        select_profile(
            &profiles,
            &address("s3://media@other-bucket/a").expect("link")
        )
        .err(),
        Some(ProfileChoiceError::None)
    );
    let by_id = format!("s3://{}@other-bucket/a", profiles[0].id);
    assert_eq!(
        select_profile(&profiles, &address(&by_id).expect("link")).err(),
        Some(ProfileChoiceError::None)
    );
    assert!(profile("general", None).serves_bucket("any-bucket"));
}

/// RD-1190-20: the machine's own credentials reach a custom endpoint only on an explicit yes.
#[test]
fn ambient_credentials_go_to_a_custom_endpoint_only_when_confirmed() {
    let mut ambient = profile("machine", None);
    ambient.credential_source = ObjectCredentialSource::Ambient;
    assert!(
        !ambient.ambient_endpoint_unconfirmed(),
        "the provider's own service"
    );
    ambient.endpoint = Some("https://minio.example:9000".to_owned());
    assert!(ambient.ambient_endpoint_unconfirmed());
    ambient.ambient_custom_endpoint = true;
    assert!(!ambient.ambient_endpoint_unconfirmed());
    let mut keyed = profile("keyed", None);
    keyed.endpoint = Some("https://minio.example:9000".to_owned());
    keyed.credential_source = ObjectCredentialSource::Static;
    assert!(
        !keyed.ambient_endpoint_unconfirmed(),
        "a stored key is the person's own"
    );
}

#[test]
fn a_disabled_or_unknown_profile_is_not_used() {
    let mut off = profile("off", None);
    off.enabled = false;
    let profiles = [off];
    assert_eq!(
        select_profile(&profiles, &address("s3://off@some-bucket/a").expect("link")).err(),
        Some(ProfileChoiceError::Disabled)
    );
    assert_eq!(
        select_profile(&profiles, &address("s3://some-bucket/a").expect("link")).err(),
        Some(ProfileChoiceError::None)
    );
    assert_eq!(
        select_profile(
            &profiles,
            &address("s3://missing@some-bucket/a").expect("link")
        )
        .err(),
        Some(ProfileChoiceError::None)
    );
}

#[test]
fn stored_references_are_never_serialized() {
    let value = serde_json::to_string(&profile("aws", None)).expect("json");
    assert!(!value.contains("vault://"));
    assert!(value.contains("\"has_secret\":true"));
}
