/// This installation's own OAuth client goes into the address the person is sent to, and a
/// missing one is a refusal rather than an address with an empty `client_id=` in it
/// (RD-106-04).
///
/// The refusal is the point. Sending somebody to Google with no client produces an error
/// page about a client that does not exist, which names nothing anybody can act on; refusing
/// here produces `oauth.client_not_configured`, whose translated text carries the steps.
#[test]
fn the_installations_own_client_goes_into_the_address_or_the_sign_in_refuses() {
    let url = "https://accounts.google.com/o/oauth2/v2/auth?client_id={{client_id}}&scope=x";
    assert_eq!(
        super::substitute_client_id(url, Some("42-abc.apps.googleusercontent.com"))
            .expect("substituted"),
        "https://accounts.google.com/o/oauth2/v2/auth\
         ?client_id=42-abc.apps.googleusercontent.com&scope=x"
    );
    // Whitespace somebody pasted along with it is not part of the client id.
    assert_eq!(
        super::substitute_client_id(url, Some("  42-abc  ")).expect("substituted"),
        "https://accounts.google.com/o/oauth2/v2/auth?client_id=42-abc&scope=x"
    );
    // A value that needs encoding gets it: this lands in a query string.
    assert_eq!(
        super::substitute_client_id("https://x/?c={{client_id}}", Some("a b&c"))
            .expect("substituted"),
        "https://x/?c=a+b%26c"
    );
    for missing in [None, Some(""), Some("   ")] {
        assert_eq!(
            super::substitute_client_id(url, missing),
            Err(super::ClientNotConfigured),
            "{missing:?}"
        );
    }
    // A plugin that names no client — one whose provider registers none — is untouched.
    let plain = "https://accounts.example.invalid/authorize?client_id=built-in";
    assert_eq!(
        super::substitute_client_id(plain, None).expect("untouched"),
        plain
    );
}
use rd_plugin_api::{ClientIdentity, HostHttpRequest, HostHttpResponse, ResolverHost};

use super::{
    AUTH_PLUGIN_MISSING, AUTH_PROVIDER_UNREACHABLE, AuthFlowService, AuthFlowState, AuthProgress,
    FailureKind, ProviderError, REFRESH_BACKOFF, RENEWAL_PLUGIN_MISSING, RenewalAction,
    SIGN_IN_BACKOFF, SIGN_IN_GRACE, TokenOutcome, renewal_action, sign_in_progress,
};

/// A host that can do nothing. No plugin is ever instantiated in these tests -- the point
/// of them is the case where none is installed -- so nothing here is ever called.
struct NoHost;

#[async_trait::async_trait]
impl ResolverHost for NoHost {
    async fn http_request(
        &self,
        _client: &ClientIdentity,
        _request: HostHttpRequest,
    ) -> Result<HostHttpResponse, rd_core::Failure> {
        Err(rd_core::Failure::new(
            FailureKind::Unsupported,
            "no host in this test".to_owned(),
        ))
    }

    async fn secret_available(&self, _account_id: rd_core::AccountId, _reference: &str) -> bool {
        false
    }
}

fn flow(started_at: chrono::DateTime<chrono::Utc>) -> rd_core::AuthFlow {
    rd_core::AuthFlow {
        account_id: rd_core::AccountId::new(),
        plugin_id: "019d0000-0000-7000-8000-0000000001ff".to_owned(),
        state: AuthFlowState::Polling,
        verification_url: Some("https://api.example.com/device".to_owned()),
        user_code: Some("ABCD-EFGH".to_owned()),
        expires_at: None,
        next_poll_at: Some(started_at),
        message: None,
        started_at,
        token_expires_at: None,
        refresh_ref: None,
        access_ref: None,
        key_ref: None,
        callback_state: None,
        flow_state: None,
    }
}

/// The renewal half of RD-106-02: the two errors are opposite outcomes, not one.
#[test]
fn a_renewal_without_a_plugin_fails_while_an_unreachable_provider_is_deferred() {
    assert_eq!(
        renewal_action(Err(ProviderError::NoPlugin {
            provider_slug: "example".to_owned(),
        })),
        RenewalAction::Fail(RENEWAL_PLUGIN_MISSING.to_owned()),
    );
    assert_eq!(
        renewal_action(Err(ProviderError::Failed(anyhow::anyhow!(
            "connection reset"
        )))),
        RenewalAction::Defer(REFRESH_BACKOFF),
    );
}

/// A provider's own answer keeps deciding, with the category it always sent and the host
/// used to drop: busy is not the same as refused.
#[test]
fn a_provider_that_refused_ends_the_renewal_and_one_that_was_busy_does_not() {
    assert_eq!(
        renewal_action(Ok(TokenOutcome::Authorized)),
        RenewalAction::Settle
    );
    assert_eq!(
        renewal_action(Ok(TokenOutcome::Pending {
            retry_after_seconds: 90,
        })),
        RenewalAction::Defer(90),
    );
    assert_eq!(
        renewal_action(Ok(TokenOutcome::Failed {
            category: FailureKind::AccountInvalid,
            message: "invalid_grant".to_owned(),
        })),
        RenewalAction::Fail("invalid_grant".to_owned()),
    );
    assert_eq!(
        renewal_action(Ok(TokenOutcome::Failed {
            category: FailureKind::RateLimited {
                retry_after_seconds: Some(120),
            },
            message: "slow down".to_owned(),
        })),
        RenewalAction::Defer(120),
    );
    assert_eq!(
        renewal_action(Ok(TokenOutcome::Failed {
            category: FailureKind::Offline,
            message: "no route to host".to_owned(),
        })),
        RenewalAction::Defer(REFRESH_BACKOFF),
    );
}

/// The sign-in half, which had the same mistake mirrored: everything ended the flow.
#[test]
fn a_sign_in_without_a_plugin_fails_while_an_unreachable_provider_is_polled_again() {
    let now = chrono::Utc::now();
    let fresh = flow(now);
    assert_eq!(
        sign_in_progress(
            Err(ProviderError::NoPlugin {
                provider_slug: "example".to_owned(),
            }),
            &fresh,
            now,
        ),
        AuthProgress::Failed {
            message: AUTH_PLUGIN_MISSING.to_owned(),
        },
    );
    assert_eq!(
        sign_in_progress(
            Err(ProviderError::Failed(anyhow::anyhow!("connection reset"))),
            &fresh,
            now,
        ),
        AuthProgress::Pending {
            retry_after_seconds: SIGN_IN_BACKOFF,
        },
    );
}

/// Deferred, but not forever: a flow the provider gave no window for is given up on.
#[test]
fn a_sign_in_nobody_named_a_window_for_is_given_up_on_after_the_grace() {
    let now = chrono::Utc::now();
    let old = flow(now - chrono::Duration::seconds(SIGN_IN_GRACE + 1));
    assert_eq!(
        sign_in_progress(
            Err(ProviderError::Failed(anyhow::anyhow!("connection reset"))),
            &old,
            now,
        ),
        AuthProgress::Failed {
            message: AUTH_PROVIDER_UNREACHABLE.to_owned(),
        },
    );
    // One that carries its own expiry keeps being polled until that expiry runs out,
    // which the sweep checks before it ever asks.
    let mut bounded = old.clone();
    bounded.expires_at = Some(now + chrono::Duration::seconds(60));
    assert_eq!(
        sign_in_progress(
            Err(ProviderError::Failed(anyhow::anyhow!("connection reset"))),
            &bounded,
            now,
        ),
        AuthProgress::Pending {
            retry_after_seconds: SIGN_IN_BACKOFF,
        },
    );
}

/// The reported defect, end to end: a renewal row whose provider no installed plugin
/// claims is attempted once, recorded as failed, and never queued again.
#[tokio::test]
async fn a_renewal_without_an_oauth_plugin_is_tried_once_and_then_fails() {
    let temporary = tempfile::tempdir().expect("tempdir");
    let database = rd_db::Database::open(temporary.path().join("auth-flows.sqlite3"))
        .await
        .expect("database");
    let account = database
        .create_account(rd_db::NewAccount {
            provider: "example".to_owned(),
            label: "Example".to_owned(),
            username: None,
            credential_mode: None,
            secret_ref: None,
            cookie_ref: None,
            proxy_profile_id: None,
            enabled: true,
        })
        .await
        .expect("account");
    let now = chrono::Utc::now();
    database
        .upsert_auth_flow(rd_db::UpsertAuthFlow {
            account_id: account.id,
            plugin_id: "019d0000-0000-7000-8000-0000000001ff".to_owned(),
            state: AuthFlowState::Authorized,
            verification_url: None,
            user_code: None,
            expires_at: None,
            next_poll_at: None,
            message: None,
            token_expires_at: Some(now + chrono::Duration::seconds(10)),
            refresh_ref: Some("account/example/refresh".to_owned()),
            access_ref: None,
            key_ref: None,
            callback_state: None,
            flow_state: None,
        })
        .await
        .expect("flow");

    let service = AuthFlowService::detached(
        database.clone(),
        temporary.path().join("plugins"),
        std::sync::Arc::new(NoHost),
    );
    service.sweep_renewals(now).await.expect("sweep");

    let stored = database
        .auth_flow(account.id)
        .await
        .expect("read")
        .expect("flow");
    assert_eq!(stored.state, AuthFlowState::Failed);
    assert_eq!(stored.message.as_deref(), Some(RENEWAL_PLUGIN_MISSING));

    // And there is no second attempt: the row left the renewal queue instead of coming
    // back in five minutes for an attempt that could not have gone any differently.
    let later = now + chrono::Duration::seconds(REFRESH_BACKOFF * 2);
    assert!(
        database
            .due_refresh_auth_flows(later, later + chrono::Duration::seconds(60))
            .await
            .expect("due")
            .is_empty()
    );
}

/// RA-HOST-02: every duration here is a plugin's, and `now + u64::MAX` seconds panicked in
/// `chrono`. Held to a day, a renewal's wait included.
#[test]
fn a_plugins_overlong_wait_is_held_to_a_day() {
    let now = chrono::Utc::now();
    let day = rd_core::MAX_RETRY_AFTER_SECONDS;
    let day_seconds = i64::try_from(day).expect("a day fits");
    assert_eq!(
        super::seconds_after(now, u64::MAX),
        now + chrono::Duration::seconds(day_seconds)
    );
    assert_eq!(
        super::seconds_after(now, 30),
        now + chrono::Duration::seconds(30)
    );
    assert_eq!(
        renewal_action(Ok(TokenOutcome::Pending {
            retry_after_seconds: u64::MAX,
        })),
        RenewalAction::Defer(day_seconds),
    );
    assert_eq!(
        renewal_action(Ok(TokenOutcome::Failed {
            category: FailureKind::RateLimited {
                retry_after_seconds: Some(u64::MAX),
            },
            message: "slow down".to_owned(),
        })),
        RenewalAction::Defer(day_seconds),
    );
}

/// RA-DB-05: a flow row that cannot be read is an error, not "nothing to carry over".
/// Read as `None`, the upsert after it wrote the renewal and access references as empty,
/// and the tokens behind them were lost while the account still had a working sign-in.
#[tokio::test]
async fn a_flow_that_cannot_be_read_is_never_overwritten_without_its_tokens() {
    let temporary = tempfile::tempdir().expect("tempdir");
    let path = temporary.path().join("auth-flows.sqlite3");
    let database = rd_db::Database::open(&path).await.expect("database");
    let account = database
        .create_account(rd_db::NewAccount {
            provider: "example".to_owned(),
            label: "Example".to_owned(),
            username: None,
            credential_mode: None,
            secret_ref: None,
            cookie_ref: None,
            proxy_profile_id: None,
            enabled: true,
        })
        .await
        .expect("account");
    let plugin_id = "019d0000-0000-7000-8000-0000000001ff";
    database
        .upsert_auth_flow(rd_db::UpsertAuthFlow {
            account_id: account.id,
            plugin_id: plugin_id.to_owned(),
            state: AuthFlowState::Polling,
            verification_url: None,
            user_code: None,
            expires_at: None,
            next_poll_at: None,
            message: None,
            token_expires_at: None,
            refresh_ref: Some("account/example/refresh".to_owned()),
            access_ref: Some("account/example/access".to_owned()),
            key_ref: None,
            callback_state: None,
            flow_state: None,
        })
        .await
        .expect("flow");
    // A state no build knows: the row is there, and reading it fails.
    let raw =
        sqlx::SqlitePool::connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(&path))
            .await
            .expect("raw connection");
    sqlx::query("UPDATE auth_flows SET state = 'from_a_later_build'")
        .execute(&raw)
        .await
        .expect("corrupt the state");
    let service = AuthFlowService::detached(
        database.clone(),
        temporary.path().join("plugins"),
        std::sync::Arc::new(NoHost),
    );

    for progress in [
        AuthProgress::Authorized,
        AuthProgress::Pending {
            retry_after_seconds: 5,
        },
    ] {
        assert!(
            service
                .store(account.id, plugin_id, progress)
                .await
                .is_err(),
            "a failed read stops the write"
        );
    }
    let (refresh, access): (Option<String>, Option<String>) =
        sqlx::query_as("SELECT refresh_ref, access_ref FROM auth_flows")
            .fetch_one(&raw)
            .await
            .expect("row");
    assert_eq!(refresh.as_deref(), Some("account/example/refresh"));
    assert_eq!(access.as_deref(), Some("account/example/access"));
}
