//! RD-120-30 against the real host: a real database, a real vault, MEGA's real provider row.
//!
//! Every test here answers one of the job's questions. Can a sign-in keep its session without
//! destroying the password? Does a chain over a typed credential still have to begin one-way?
//! Is a chain over sign-in key material admitted? And -- the one the relaxation stands or falls
//! by -- **can anything make a person's credential be treated as sign-in key material?**

use std::sync::Arc;

use aes::{
    Aes128,
    cipher::{BlockEncrypt, KeyInit},
};
use rd_core::AccountId;
use rd_db::NewAccount;
use rd_http::{ClientPool, NetworkDefaults};
use rd_plugin_api::{
    ClientIdentity, DerivationStep, HostHttpRequest, HostRequestValue, ResolverHost as _,
};
use tokio::sync::RwLock;

use super::NativeHost;

const PASSWORD: &str = "the-password-a-person-typed";
const SESSION_ID: &str = "an-invented-session-identifier";
/// Sixteen invented bytes, base64url: what `plugins/mega-auth` stores as the master key.
const MASTER_KEY: [u8; 16] = *b"invented-master!";
const MASTER_KEY_TEXT: &str = "aW52ZW50ZWQtbWFzdGVyIQ";

async fn test_host(dir: &std::path::Path) -> NativeHost {
    crate::native::register_bundled_providers_for_tests();
    let database = rd_db::Database::open(dir.join("db.sqlite"))
        .await
        .expect("database");
    let secrets = rd_secrets::SecretStore::open(dir.join("secrets"))
        .await
        .expect("secret store");
    NativeHost::new(
        database,
        ClientPool::default(),
        secrets,
        Arc::new(RwLock::new(NetworkDefaults::default())),
        None,
    )
}

/// A MEGA account as the accounts form creates one: an address and a typed password.
async fn mega_account(host: &NativeHost) -> AccountId {
    let secret_ref = host
        .secrets
        .put_string(PASSWORD.to_owned())
        .await
        .expect("put the password");
    host.database
        .create_account(NewAccount {
            provider: "mega".to_owned(),
            label: "Test".to_owned(),
            username: Some("person@example.test".to_owned()),
            credential_mode: None,
            secret_ref: Some(secret_ref),
            cookie_ref: None,
            proxy_profile_id: None,
            enabled: true,
        })
        .await
        .expect("create account")
        .id
}

fn envelope() -> String {
    format!(r#"{{"token":"{SESSION_ID}","key":"{MASTER_KEY_TEXT}"}}"#)
}

fn identity(account_id: AccountId) -> ClientIdentity {
    ClientIdentity {
        account_id: Some(account_id),
        proxy_profile_id: None,
        tls_revision: 0,
    }
}

fn wrap(key: &[u8; 16], plain: &[u8]) -> Vec<u8> {
    let cipher = Aes128::new(key.into());
    let mut out = plain.to_vec();
    for block in out.as_chunks_mut::<16>().0 {
        cipher.encrypt_block(block.into());
    }
    out
}

/// What `{{secret:<reference>}}` resolves to for a request to MEGA's command endpoint.
async fn sent(host: &NativeHost, account_id: AccountId, reference: &str) -> Option<String> {
    let request = HostHttpRequest {
        method: "POST".to_owned(),
        url: "https://g.api.mega.co.nz/cs".parse().expect("url"),
        query: vec![HostRequestValue {
            name: "sid".to_owned(),
            value_template: format!("{{{{secret:{reference}}}}}"),
        }],
        headers: Vec::new(),
        body: Vec::new(),
        granted_secret: None,
        authority: rd_plugin_api::RequestAuthority::Provider,
        write_methods: false,
    };
    host.request_secrets(&identity(account_id), &request)
        .await
        .expect("resolved")
        .named(reference)
        .map(str::to_owned)
}

fn code(result: Result<Vec<u8>, rd_core::Failure>) -> Option<String> {
    result.err().and_then(|failure| failure.code)
}

#[tokio::test]
async fn a_sign_in_keeps_the_password_and_stores_its_token_and_key_apart() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let host = test_host(directory.path()).await;
    let account_id = mega_account(&host).await;

    host.store_token(account_id, &envelope())
        .await
        .expect("store the session");

    // The password survived. Before RD-120-30 this was the session, and the next sign-in
    // computed PBKDF2 over a session identifier.
    assert_eq!(
        sent(&host, account_id, "mega_password").await.as_deref(),
        Some(PASSWORD)
    );
    // The marker sends the token and only the token -- never the object, never the key.
    assert_eq!(
        sent(&host, account_id, "mega_session").await.as_deref(),
        Some(SESSION_ID)
    );
    assert!(host.secret_available(account_id, "mega_session").await);
}

#[tokio::test]
async fn a_chain_over_the_sign_in_key_is_admitted_and_unwraps_under_it() {
    // Acceptance criterion 2 of RD-120-30, through the vault.
    let directory = tempfile::tempdir().expect("temporary directory");
    let host = test_host(directory.path()).await;
    let account_id = mega_account(&host).await;
    host.store_token(account_id, &envelope())
        .await
        .expect("store the session");

    let node_key: Vec<u8> = (0_u8..32).collect();
    let answer = host
        .derive_from_secret(
            &identity(account_id),
            "mega_session",
            &[DerivationStep::Aes128EcbDecrypt(wrap(
                &MASTER_KEY,
                &node_key,
            ))],
        )
        .await
        .expect("the unwrap");
    assert_eq!(answer, node_key);
}

#[tokio::test]
async fn a_symmetric_chain_over_the_typed_password_is_still_refused() {
    // Acceptance criterion 1, through the vault: the same chain, aimed at the other slot.
    let directory = tempfile::tempdir().expect("temporary directory");
    let host = test_host(directory.path()).await;
    let account_id = mega_account(&host).await;
    host.store_token(account_id, &envelope())
        .await
        .expect("store the session");

    let refused = host
        .derive_from_secret(
            &identity(account_id),
            "mega_password",
            &[DerivationStep::Aes128EcbDecrypt(vec![0; 16])],
        )
        .await;
    assert_eq!(
        code(refused).as_deref(),
        Some("plugin.key_derivation_needs_one_way")
    );
}

#[tokio::test]
async fn a_person_s_credential_is_never_read_as_sign_in_key_material() {
    // Acceptance criterion 3: the origin cannot be forged. A plugin names a slot and nothing
    // else, and the only thing naming the flow slot changes is *where the host reads* -- the
    // flow row, which the accounts form never writes. With a typed password and no sign-in,
    // a chain over the flow slot finds nothing to compute over, rather than the password.
    let directory = tempfile::tempdir().expect("temporary directory");
    let host = test_host(directory.path()).await;
    let account_id = mega_account(&host).await;

    let chain = [DerivationStep::Aes128EcbDecrypt(vec![0; 16])];
    let refused = host
        .derive_from_secret(&identity(account_id), "mega_session", &chain)
        .await;
    assert_eq!(
        code(refused).as_deref(),
        Some("plugin.provider_secret_missing")
    );

    // A flow that stores a plain token has stored no key either: a bearer credential is not
    // key material, and the token half is never what a chain runs over.
    host.store_token(account_id, "just-a-token")
        .await
        .expect("store a plain token");
    let refused = host
        .derive_from_secret(&identity(account_id), "mega_session", &chain)
        .await;
    assert_eq!(
        code(refused).as_deref(),
        Some("plugin.provider_secret_missing")
    );
}

#[tokio::test]
async fn the_origin_follows_the_slot_and_no_chain_shape_changes_it() {
    // The other half of criterion 3. Whatever a guest sends, the host decides the origin from
    // the reference's slot in the provider table. A window first -- the step that would read a
    // stored key byte by byte -- is refused over both slots, and PBKDF2 over the sign-in key is
    // refused rather than quietly computed over the password instead.
    let directory = tempfile::tempdir().expect("temporary directory");
    let host = test_host(directory.path()).await;
    let account_id = mega_account(&host).await;
    host.store_token(account_id, &envelope())
        .await
        .expect("store the session");

    for reference in ["mega_password", "mega_session"] {
        let window = host
            .derive_from_secret(
                &identity(account_id),
                reference,
                &[
                    DerivationStep::Take {
                        offset: 0,
                        length: 16,
                    },
                    DerivationStep::Aes128EcbDecrypt(vec![0; 16]),
                ],
            )
            .await;
        assert!(window.is_err(), "{reference} admitted a window first");
    }
    let one_way = host
        .derive_from_secret(
            &identity(account_id),
            "mega_session",
            &[DerivationStep::Pbkdf2HmacSha512 {
                salt: b"salt".to_vec(),
                rounds: crate::keyderive::MIN_PBKDF2_ROUNDS,
                length: 32,
            }],
        )
        .await;
    assert_eq!(
        code(one_way).as_deref(),
        Some("plugin.key_derivation_needs_key_step")
    );
}

#[tokio::test]
async fn signing_in_again_replaces_both_halves_and_drops_the_old_ones() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let host = test_host(directory.path()).await;
    let account_id = mega_account(&host).await;
    host.store_token(account_id, &envelope())
        .await
        .expect("first sign-in");
    let first = host
        .database
        .auth_flow(account_id)
        .await
        .expect("read")
        .expect("a row");

    let second_key = [7_u8; 16];
    let second = format!(
        r#"{{"token":"second-session","key":"{}"}}"#,
        base64::Engine::encode(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD,
            second_key
        )
    );
    host.store_token(account_id, &second)
        .await
        .expect("second sign-in");

    assert_eq!(
        sent(&host, account_id, "mega_session").await.as_deref(),
        Some("second-session")
    );
    let node_key = [9_u8; 16];
    let answer = host
        .derive_from_secret(
            &identity(account_id),
            "mega_session",
            &[DerivationStep::Aes128EcbDecrypt(wrap(
                &second_key,
                &node_key,
            ))],
        )
        .await
        .expect("the unwrap");
    assert_eq!(
        answer, node_key,
        "the new token was paired with the old key"
    );
    for old in [first.access_ref, first.key_ref].into_iter().flatten() {
        assert!(
            host.secrets.get(&old).await.is_err(),
            "the previous session's {old} is still in the vault"
        );
    }
}

#[tokio::test]
async fn a_malformed_session_is_refused_and_leaves_the_password_alone() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let host = test_host(directory.path()).await;
    let account_id = mega_account(&host).await;
    let failure = host
        .store_token(
            account_id,
            &format!(r#"{{"sid":"{SESSION_ID}","mk":"{MASTER_KEY_TEXT}"}}"#),
        )
        .await
        .expect_err("not the shape a keyed session has");
    assert_eq!(
        failure.code.as_deref(),
        Some("plugin.store_token_session_invalid")
    );
    assert_eq!(
        sent(&host, account_id, "mega_password").await.as_deref(),
        Some(PASSWORD)
    );
    assert!(!host.secret_available(account_id, "mega_session").await);
}
