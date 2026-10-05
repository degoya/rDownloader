//! Capture and API tokens, and authentication profiles.

use chrono::{Duration, Utc};
use rd_core::{AuthMethod, AuthOrigin, AuthProfileSelection, AuthScope, DownloadId, PackageId};

use super::probe_url;
use crate::{Database, NewAuthProfile, NewDownload, NewPackage, UpdateAuthProfile};

fn scope(input: &str, subdomains: bool) -> AuthScope {
    AuthScope::parse(input, subdomains).expect("scope")
}

fn new_profile(name: &str, host: &str, subdomains: bool) -> NewAuthProfile {
    NewAuthProfile {
        name: name.to_owned(),
        scope: scope(host, subdomains),
        method: AuthMethod::Bearer,
        origin: AuthOrigin::Manual,
        enabled: true,
        expires_at: None,
        username: None,
        secret_ref: Some(format!("vault://{name}")),
        certificate_ref: None,
    }
}

#[tokio::test]
async fn token_scopes_separate_capture_from_api_access() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("tokens.sqlite"))
        .await
        .expect("database");
    let capture_sha = "aa".repeat(32);
    let api_sha = "bb".repeat(32);
    database
        .create_capture_token(
            rd_core::CaptureTokenId::new(),
            "Browser".to_owned(),
            capture_sha.clone(),
            vec![rd_core::CAPTURE_SCOPE.to_owned()],
        )
        .await
        .expect("capture token");
    let api_token = database
        .create_capture_token(
            rd_core::CaptureTokenId::new(),
            "Assistant".to_owned(),
            api_sha.clone(),
            vec![rd_core::API_SCOPE.to_owned()],
        )
        .await
        .expect("api token");

    let valid = |sha: String, scope: &'static str| {
        let database = database.clone();
        async move {
            database
                .capture_token_valid(&sha, scope)
                .await
                .expect("check")
        }
    };
    assert!(valid(capture_sha.clone(), rd_core::CAPTURE_SCOPE).await);
    assert!(!valid(capture_sha.clone(), rd_core::API_SCOPE).await);
    assert!(valid(api_sha.clone(), rd_core::API_SCOPE).await);
    assert!(!valid(api_sha.clone(), rd_core::CAPTURE_SCOPE).await);
    // Full API access covers the read-only surface, so one token is enough for a client
    // that both acts and reads; the reverse never holds.
    assert!(valid(api_sha.clone(), rd_core::API_READ_SCOPE).await);
    assert!(!valid(capture_sha.clone(), rd_core::API_READ_SCOPE).await);

    let capture_list = database
        .list_capture_tokens(&[rd_core::CAPTURE_SCOPE])
        .await
        .expect("capture list");
    let api_list = database
        .list_capture_tokens(&[rd_core::API_SCOPE])
        .await
        .expect("api list");
    assert_eq!(capture_list.len(), 1);
    assert_eq!(capture_list[0].label, "Browser");
    assert_eq!(api_list.len(), 1);
    assert_eq!(api_list[0].label, "Assistant");

    database
        .revoke_capture_token(api_token.id)
        .await
        .expect("revoke");
    assert!(!valid(api_sha, rd_core::API_SCOPE).await);
}

#[tokio::test]
async fn auth_profiles_survive_a_restart_and_keep_matching() {
    // The acceptance criterion is that import and use work unchanged after a server
    // restart, so the database is closed and reopened from the same directory.
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("auth.sqlite");
    let created = {
        let database = Database::open(path.clone()).await.expect("database");
        database
            .create_auth_profile(new_profile("intranet", "https://files.example.com/", false))
            .await
            .expect("profile")
    };

    let database = Database::open(path).await.expect("reopen");
    let matched = database
        .match_auth_profile(&"https://files.example.com/report.pdf".parse().expect("url"))
        .await
        .expect("match")
        .expect("profile still matches after restart");
    assert_eq!(matched.id, created.id);
    assert_eq!(matched.secret_ref.as_deref(), Some("vault://intranet"));
}

#[tokio::test]
async fn auto_match_prefers_the_most_specific_scope() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("auth.sqlite"))
        .await
        .expect("database");
    database
        .create_auth_profile(new_profile("wide", "example.com", true))
        .await
        .expect("wide");
    let narrow = database
        .create_auth_profile(new_profile("narrow", "https://cdn.example.com/", false))
        .await
        .expect("narrow");

    let matched = database
        .match_auth_profile(&"https://cdn.example.com/f.bin".parse().expect("url"))
        .await
        .expect("match")
        .expect("profile");
    assert_eq!(matched.id, narrow.id, "more host labels must win");
}

#[tokio::test]
async fn auto_match_skips_disabled_and_expired_profiles() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("auth.sqlite"))
        .await
        .expect("database");
    let mut expiring = new_profile("expired", "expired.example", false);
    expiring.expires_at = Some(Utc::now() - Duration::hours(1));
    database
        .create_auth_profile(expiring)
        .await
        .expect("create");
    let disabled = database
        .create_auth_profile(new_profile("disabled", "disabled.example", false))
        .await
        .expect("create");
    database
        .set_auth_profile_enabled(disabled.id, false)
        .await
        .expect("disable");

    for host in ["https://expired.example/f", "https://disabled.example/f"] {
        assert!(
            database
                .match_auth_profile(&host.parse().expect("url"))
                .await
                .expect("match")
                .is_none(),
            "{host}"
        );
    }
}

#[tokio::test]
async fn a_pinned_profile_fails_the_job_instead_of_downloading_unauthenticated() {
    // Silently continuing without the credential would write a login page over the file.
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("auth.sqlite"))
        .await
        .expect("database");
    let profile = database
        .create_auth_profile(new_profile("pinned", "example.com", false))
        .await
        .expect("create");
    let url = probe_url();

    let resolved = database
        .network_client_config(
            None,
            None,
            None,
            AuthProfileSelection::Pinned(profile.id),
            &url,
        )
        .await
        .expect("pinned config");
    assert_eq!(resolved.auth.expect("profile").id, profile.id);

    database
        .set_auth_profile_enabled(profile.id, false)
        .await
        .expect("disable");
    assert!(
        database
            .network_client_config(
                None,
                None,
                None,
                AuthProfileSelection::Pinned(profile.id),
                &url,
            )
            .await
            .is_err(),
        "a disabled pinned profile must fail the job"
    );

    // Explicitly asking for no profile stays silent even where one would match.
    let none = database
        .network_client_config(None, None, None, AuthProfileSelection::None, &url)
        .await
        .expect("none config");
    assert!(none.auth.is_none());
}

#[tokio::test]
async fn a_pinned_profile_outside_its_scope_is_refused() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("auth.sqlite"))
        .await
        .expect("database");
    let profile = database
        .create_auth_profile(new_profile("scoped", "example.com", false))
        .await
        .expect("create");
    assert!(
        database
            .network_client_config(
                None,
                None,
                None,
                AuthProfileSelection::Pinned(profile.id),
                &"https://other.tld/f".parse().expect("url"),
            )
            .await
            .is_err()
    );
}

#[tokio::test]
async fn captured_profiles_are_never_created_enabled() {
    // A capture token lives in a browser; it must not be able to mint a usable credential.
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("auth.sqlite"))
        .await
        .expect("database");
    let mut input = new_profile("captured", "example.com", false);
    input.origin = AuthOrigin::BrowserCapture;
    input.enabled = true;
    let profile = database.create_auth_profile(input).await.expect("create");
    assert!(!profile.enabled, "capture intake must land disabled");
    assert!(
        database
            .match_auth_profile(&probe_url())
            .await
            .expect("match")
            .is_none()
    );

    let approved = database
        .set_auth_profile_enabled(profile.id, true)
        .await
        .expect("approve");
    assert!(approved.enabled);
}

#[tokio::test]
async fn updating_and_deleting_a_profile_reports_orphaned_secret_references() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("auth.sqlite"))
        .await
        .expect("database");
    let mut input = new_profile("rotate", "example.com", false);
    input.certificate_ref = Some("vault://cert".to_owned());
    let profile = database.create_auth_profile(input).await.expect("create");

    let (updated, orphaned) = database
        .update_auth_profile(
            profile.id,
            UpdateAuthProfile {
                name: "rotate".to_owned(),
                scope: scope("example.com", false),
                method: AuthMethod::Bearer,
                enabled: true,
                expires_at: None,
                username: None,
                secret_ref: Some("vault://rotated".to_owned()),
                certificate_ref: Some("vault://cert".to_owned()),
            },
        )
        .await
        .expect("update");
    // Only the replaced reference is orphaned; the untouched certificate stays in use.
    assert_eq!(orphaned, vec!["vault://rotate".to_owned()]);
    assert!(updated.has_secret && updated.has_client_certificate);

    let remaining = database
        .delete_auth_profile(profile.id)
        .await
        .expect("delete");
    assert_eq!(
        remaining,
        vec!["vault://rotated".to_owned(), "vault://cert".to_owned()]
    );
    assert!(
        database
            .list_auth_profiles()
            .await
            .expect("list")
            .is_empty()
    );
}

#[tokio::test]
async fn deleting_a_profile_releases_the_jobs_that_pinned_it() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("auth.sqlite"))
        .await
        .expect("database");
    let profile = database
        .create_auth_profile(new_profile("pinned", "example.com", false))
        .await
        .expect("create");
    let package = database
        .create_package(NewPackage {
            id: PackageId::new(),
            name: "package".to_owned(),
            destination: directory.path().display().to_string(),
            category_id: None,
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");
    let download = database
        .create_download(NewDownload {
            id: DownloadId::new(),
            package_id: package.id,
            source: probe_url(),
            file_name: "file.bin".to_owned(),
            total_bytes: None,
            expected_checksum: None,
            account_id: None,
            proxy_profile_id: None,
            auth_profile: AuthProfileSelection::Pinned(profile.id),
            initial_state: rd_core::DownloadState::Paused,
            kind: rd_core::DownloadKind::Http,
            media: None,
            remote_credential_id: None,
            replay: None,
            mirror_group: None,
            enrichment: Vec::new(),
            secret_fragment: None,
        })
        .await
        .expect("download");
    assert_eq!(
        download.auth_profile,
        AuthProfileSelection::Pinned(profile.id)
    );

    database
        .delete_auth_profile(profile.id)
        .await
        .expect("delete");
    let reloaded = database
        .get_download(download.id)
        .await
        .expect("load")
        .expect("download");
    // Falling back to auto-matching beats leaving a dangling pin that fails every retry.
    assert_eq!(reloaded.auth_profile, AuthProfileSelection::Auto);
}

/// Re-scoping a live token, and the trail it has to leave.
///
/// The trail is the point. Fixed scopes were their own audit story — a token could only ever
/// do what it was born to do — and changeable scopes replace that story with a written one.
/// So this asserts both halves: the new areas take effect for the digest that was already
/// there, and the `events` table can afterwards say what the token was issued with and what
/// it was changed to.
#[tokio::test]
async fn token_scopes_can_be_rewritten_and_both_steps_are_recorded() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("rescope.sqlite"))
        .await
        .expect("database");
    let digest = "cc".repeat(32);
    let token = database
        .create_capture_token(
            rd_core::CaptureTokenId::new(),
            "Assistant".to_owned(),
            digest.clone(),
            vec![rd_core::API_READ_SCOPE.to_owned()],
        )
        .await
        .expect("api token");
    assert!(
        !database
            .capture_token_valid(&digest, rd_core::API_CONFIG_SCOPE)
            .await
            .expect("check")
    );

    let widened = database
        .update_capture_token_scopes(
            token.id,
            vec![
                rd_core::API_READ_SCOPE.to_owned(),
                rd_core::API_CONFIG_SCOPE.to_owned(),
            ],
        )
        .await
        .expect("widen");
    assert_eq!(widened.id, token.id);
    assert_eq!(widened.label, "Assistant");
    assert_eq!(widened.created_at, token.created_at);
    // The digest is untouched, which is what "the bearer keeps working" means at this layer.
    assert!(
        database
            .capture_token_valid(&digest, rd_core::API_CONFIG_SCOPE)
            .await
            .expect("check")
    );

    let narrowed = database
        .update_capture_token_scopes(token.id, vec![rd_core::API_READ_SCOPE.to_owned()])
        .await
        .expect("narrow");
    assert_eq!(narrowed.scopes, vec![rd_core::API_READ_SCOPE.to_owned()]);
    assert!(
        !database
            .capture_token_valid(&digest, rd_core::API_CONFIG_SCOPE)
            .await
            .expect("check")
    );

    let payloads = sqlx::query_scalar::<_, String>(
        "SELECT payload_json FROM events WHERE kind = 'capture_changed' ORDER BY occurred_at, id",
    )
    .fetch_all(&database.readers)
    .await
    .expect("events");
    let payloads: Vec<serde_json::Value> = payloads
        .iter()
        .map(|payload| serde_json::from_str(payload).expect("payload"))
        .filter(|payload: &serde_json::Value| {
            payload["capture_token_id"] == serde_json::json!(token.id)
        })
        .collect();
    assert_eq!(payloads.len(), 3, "{payloads:?}");
    assert_eq!(payloads[0]["issued"], serde_json::json!(true));
    assert_eq!(
        payloads[0]["scopes"],
        serde_json::json!([rd_core::API_READ_SCOPE])
    );
    assert_eq!(payloads[1]["scopes_changed"], serde_json::json!(true));
    assert_eq!(
        payloads[1]["previous_scopes"],
        serde_json::json!([rd_core::API_READ_SCOPE])
    );
    assert_eq!(
        payloads[1]["scopes"],
        serde_json::json!([rd_core::API_READ_SCOPE, rd_core::API_CONFIG_SCOPE])
    );
    assert_eq!(
        payloads[2]["previous_scopes"],
        serde_json::json!([rd_core::API_READ_SCOPE, rd_core::API_CONFIG_SCOPE])
    );
}

/// A revoked token is gone, not merely inert: re-scoping must not resurrect one.
#[tokio::test]
async fn a_revoked_or_unknown_token_cannot_be_rescoped() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("rescope-revoked.sqlite"))
        .await
        .expect("database");
    let token = database
        .create_capture_token(
            rd_core::CaptureTokenId::new(),
            "Assistant".to_owned(),
            "dd".repeat(32),
            vec![rd_core::API_READ_SCOPE.to_owned()],
        )
        .await
        .expect("api token");
    database
        .revoke_capture_token(token.id)
        .await
        .expect("revoke");

    for id in [token.id, rd_core::CaptureTokenId::new()] {
        let error = database
            .update_capture_token_scopes(id, vec![rd_core::API_SCOPE.to_owned()])
            .await
            .expect_err("no live token");
        assert!(error.to_string().contains("not found"), "{error}");
    }
}
