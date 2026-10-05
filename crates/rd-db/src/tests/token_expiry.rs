//! Token expiry (RD-1110-07): past its `expires_at` a token matches no live token, as after a
//! revocation, yet stays listed until somebody revokes it.

use chrono::{Duration, Utc};

use crate::Database;

#[tokio::test]
async fn an_expired_token_is_no_live_token_but_stays_listed() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("expiry.sqlite"))
        .await
        .expect("database");
    let expired_sha = "cc".repeat(32);
    let current_sha = "dd".repeat(32);
    let never_sha = "ee".repeat(32);
    for (sha, expires_at) in [
        (&expired_sha, Some(Utc::now() - Duration::minutes(1))),
        (&current_sha, Some(Utc::now() + Duration::days(1))),
        (&never_sha, None),
    ] {
        database
            .create_expiring_capture_token(
                rd_core::CaptureTokenId::new(),
                format!("token {}", &sha[..2]),
                sha.clone(),
                vec![rd_core::API_READ_SCOPE.to_owned()],
                expires_at,
            )
            .await
            .expect("token");
    }

    for (sha, live) in [
        (&expired_sha, false),
        (&current_sha, true),
        (&never_sha, true),
    ] {
        assert_eq!(
            database
                .capture_token_valid(sha, rd_core::API_READ_SCOPE)
                .await
                .expect("check"),
            live,
            "{sha}"
        );
        assert_eq!(
            database
                .capture_token_scopes(sha)
                .await
                .expect("scopes")
                .is_some(),
            live,
            "{sha}"
        );
        assert_eq!(
            database
                .capture_token_identity(sha)
                .await
                .expect("identity")
                .is_some(),
            live,
            "{sha}"
        );
    }

    let listed = database
        .list_capture_tokens(&[rd_core::API_READ_SCOPE])
        .await
        .expect("list");
    assert_eq!(listed.len(), 3, "an expired token is still listed");
    assert_eq!(
        listed
            .iter()
            .filter(|token| token.expires_at.is_none())
            .count(),
        1
    );
}
