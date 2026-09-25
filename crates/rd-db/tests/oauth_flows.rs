//! What an OAuth flow needs the database to remember (RD-103-00).
//!
//! The point of every one of these is restart survival. A renewal that lived in memory would
//! be lost the moment the service stopped, and the person would be asked to sign in again for
//! no reason other than a restart -- so the questions "is this due" and "whose callback is
//! this" are answered by a query, and these tests are those queries.

use chrono::{Duration, Utc};
use rd_core::{AccountId, AuthFlowState};
use rd_db::{Database, NewAccount, UpsertAuthFlow};
use tempfile::TempDir;

async fn database(directory: &TempDir) -> Database {
    Database::open(directory.path().join("oauth.sqlite"))
        .await
        .expect("database")
}

async fn account(database: &Database) -> AccountId {
    database
        .create_account(NewAccount {
            provider: "demo".to_owned(),
            label: "Demo".to_owned(),
            username: None,
            credential_mode: None,
            secret_ref: None,
            cookie_ref: None,
            proxy_profile_id: None,
            enabled: true,
        })
        .await
        .expect("account")
        .id
}

/// A flow with every field a redirect sign-in fills in.
fn flow(account_id: AccountId) -> UpsertAuthFlow {
    UpsertAuthFlow {
        account_id,
        plugin_id: "demo-plugin".to_owned(),
        state: AuthFlowState::WaitingForUser,
        verification_url: Some("https://accounts.example.com/authorize".to_owned()),
        user_code: None,
        expires_at: None,
        next_poll_at: None,
        message: None,
        token_expires_at: None,
        refresh_ref: None,
        access_ref: None,
        key_ref: None,
        callback_state: Some("the-echoed-value".to_owned()),
        flow_state: Some("the-plugins-bookkeeping".to_owned()),
    }
}

/// An authorised flow whose token dies at `expires_in` from now.
fn renewable(account_id: AccountId, expires_in: Duration) -> UpsertAuthFlow {
    UpsertAuthFlow {
        state: AuthFlowState::Authorized,
        callback_state: None,
        token_expires_at: Some(Utc::now() + expires_in),
        refresh_ref: Some("vault://11111111-1111-1111-1111-111111111111".to_owned()),
        access_ref: None,
        key_ref: None,
        ..flow(account_id)
    }
}

#[tokio::test]
async fn a_flow_waiting_for_a_redirect_is_never_polled() {
    // The trap this closes: a redirect flow sits in `waiting_for_user` with no next poll,
    // which is exactly what the sign-in sweep picks up. It would drive the flow through the
    // device-code plugins, which do not claim its provider, and fail it on the first tick.
    let directory = TempDir::new().expect("directory");
    let database = database(&directory).await;
    let account_id = account(&database).await;
    database
        .upsert_auth_flow(flow(account_id))
        .await
        .expect("upsert");

    let due = database.due_auth_flows(Utc::now()).await.expect("due");
    assert!(
        due.is_empty(),
        "a redirect flow must not be offered for polling"
    );
}

#[tokio::test]
async fn a_device_flow_alongside_it_is_still_polled() {
    // The exclusion must be about the callback state, not about the state name it shares.
    let directory = TempDir::new().expect("directory");
    let database = database(&directory).await;
    let account_id = account(&database).await;
    database
        .upsert_auth_flow(UpsertAuthFlow {
            callback_state: None,
            ..flow(account_id)
        })
        .await
        .expect("upsert");

    let due = database.due_auth_flows(Utc::now()).await.expect("due");
    assert_eq!(due.len(), 1, "a device flow is still due");
}

#[tokio::test]
async fn a_callback_finds_its_flow_by_the_value_the_provider_echoed() {
    let directory = TempDir::new().expect("directory");
    let database = database(&directory).await;
    let account_id = account(&database).await;
    database
        .upsert_auth_flow(flow(account_id))
        .await
        .expect("upsert");

    let found = database
        .auth_flow_by_callback_state("the-echoed-value")
        .await
        .expect("lookup");
    assert_eq!(found.map(|flow| flow.account_id), Some(account_id));

    // The lookup is the check: a callback quoting anything else belongs to nobody.
    let stranger = database
        .auth_flow_by_callback_state("not-a-state-we-issued")
        .await
        .expect("lookup");
    assert!(stranger.is_none(), "an unknown callback matches no flow");
}

#[tokio::test]
async fn renewal_becomes_due_only_once_the_expiry_is_within_reach() {
    let directory = TempDir::new().expect("directory");
    let database = database(&directory).await;
    let account_id = account(&database).await;
    database
        .upsert_auth_flow(renewable(account_id, Duration::minutes(10)))
        .await
        .expect("upsert");

    let now = Utc::now();
    let early = database
        .due_refresh_auth_flows(now, now + Duration::minutes(1))
        .await
        .expect("due");
    assert!(early.is_empty(), "a token with ten minutes left is not due");

    let late = database
        .due_refresh_auth_flows(now, now + Duration::minutes(11))
        .await
        .expect("due");
    assert_eq!(late.len(), 1, "a token inside the lead is due");
    assert_eq!(
        late[0].refresh_ref.as_deref(),
        Some("vault://11111111-1111-1111-1111-111111111111"),
        "the reference has to come back, or there is nothing to renew with"
    );
}

#[tokio::test]
async fn a_flow_with_no_refresh_material_is_never_due_for_renewal() {
    // Without material the only way back is the person signing in again, and a sweep that
    // claimed otherwise would ask a provider the same impossible question forever.
    let directory = TempDir::new().expect("directory");
    let database = database(&directory).await;
    let account_id = account(&database).await;
    database
        .upsert_auth_flow(UpsertAuthFlow {
            refresh_ref: None,
            access_ref: None,
            key_ref: None,
            ..renewable(account_id, Duration::minutes(-5))
        })
        .await
        .expect("upsert");

    let now = Utc::now();
    let due = database
        .due_refresh_auth_flows(now, now + Duration::minutes(1))
        .await
        .expect("due");
    assert!(due.is_empty());
}

#[tokio::test]
async fn a_held_back_renewal_waits_its_turn() {
    // The rate-limit guard: a provider that answers "not yet" must not be asked again on the
    // next three-second tick.
    let directory = TempDir::new().expect("directory");
    let database = database(&directory).await;
    let account_id = account(&database).await;
    database
        .upsert_auth_flow(renewable(account_id, Duration::minutes(-5)))
        .await
        .expect("upsert");

    let now = Utc::now();
    assert_eq!(
        database
            .due_refresh_auth_flows(now, now + Duration::minutes(1))
            .await
            .expect("due")
            .len(),
        1,
        "due before it is held back"
    );

    database
        .defer_auth_flow_renewal(account_id, now + Duration::minutes(5))
        .await
        .expect("defer");
    assert!(
        database
            .due_refresh_auth_flows(now, now + Duration::minutes(1))
            .await
            .expect("due")
            .is_empty(),
        "held back until its time comes"
    );
    assert_eq!(
        database
            .due_refresh_auth_flows(now + Duration::minutes(6), now + Duration::minutes(7))
            .await
            .expect("due")
            .len(),
        1,
        "and due again afterwards"
    );
}

#[tokio::test]
async fn what_a_renewal_needs_survives_a_restart() {
    // The whole reason any of this is in the database rather than in memory: a service that
    // stops mid-life must come back knowing when the token dies and what renews it.
    let directory = TempDir::new().expect("directory");
    let account_id = {
        let database = database(&directory).await;
        let account_id = account(&database).await;
        database
            .upsert_auth_flow(renewable(account_id, Duration::minutes(-1)))
            .await
            .expect("upsert");
        account_id
    };

    let reopened = database(&directory).await;
    let now = Utc::now();
    let due = reopened
        .due_refresh_auth_flows(now, now + Duration::minutes(1))
        .await
        .expect("due");
    assert_eq!(due.len(), 1, "the renewal is picked up after a restart");
    assert_eq!(due[0].account_id, account_id);
    assert!(due[0].token_expires_at.is_some(), "the expiry came back");
    assert!(due[0].refresh_ref.is_some(), "the reference came back");
}

/// A callback that has been answered cannot be answered a second time (RD-105-01).
///
/// The idempotency rule the redirect flow rests on. An authorization code is single-use, and
/// the address carrying it lands in a browser history, a proxy log and a bookmark bar — so
/// reloading that page, or replaying it, must find nothing rather than start the exchange
/// again. What enforces it is that completing a flow clears `callback_state`, which is the
/// only way in: the lookup then matches no row.
#[tokio::test]
async fn an_answered_callback_state_matches_nothing_the_second_time() {
    let directory = TempDir::new().expect("directory");
    let database = database(&directory).await;
    let account_id = account(&database).await;
    database
        .upsert_auth_flow(flow(account_id))
        .await
        .expect("upsert");
    assert!(
        database
            .auth_flow_by_callback_state("the-echoed-value")
            .await
            .expect("lookup")
            .is_some()
    );

    // What `complete_oauth` writes once the exchange has been made: the flow stays, its
    // callback state does not.
    database
        .upsert_auth_flow(UpsertAuthFlow {
            state: AuthFlowState::Authorized,
            callback_state: None,
            flow_state: None,
            ..flow(account_id)
        })
        .await
        .expect("upsert");

    let replayed = database
        .auth_flow_by_callback_state("the-echoed-value")
        .await
        .expect("lookup");
    assert!(
        replayed.is_none(),
        "a code presented twice must find no flow the second time"
    );
    // And the flow itself is still there, authorised — clearing the state is not deleting it.
    let kept = database.auth_flow(account_id).await.expect("flow");
    assert_eq!(kept.map(|flow| flow.state), Some(AuthFlowState::Authorized));
}
