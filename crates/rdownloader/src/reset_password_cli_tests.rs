use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::{
    Json, Router,
    http::{HeaderMap, StatusCode},
    routing::post,
};

use super::*;

const OLD: &str = "the-forgotten-password";
const NEW: &str = "a-brand-new-passphrase";

/// What the stand-in service was sent, one body per call.
type Calls = Arc<Mutex<Vec<serde_json::Value>>>;

/// A stand-in for the running service: answers the reset route with the right token only.
async fn service(token: &'static str) -> (SocketAddr, Calls) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let address = listener.local_addr().expect("address");
    let calls: Calls = Arc::default();
    let seen = calls.clone();
    let router = Router::new().route(
        "/api/v1/auth/password/reset",
        post(
            move |headers: HeaderMap, Json(body): Json<serde_json::Value>| {
                let seen = seen.clone();
                async move {
                    if headers
                        .get("authorization")
                        .and_then(|value| value.to_str().ok())
                        != Some(format!("Bearer {token}").as_str())
                    {
                        return (StatusCode::UNAUTHORIZED, "{}".to_owned());
                    }
                    seen.lock().expect("calls").push(body);
                    (
                        StatusCode::OK,
                        r#"{"code":"auth.password_reset_done","message":"reset",
                            "params":{"sessions_ended":"2","password_login":"off"}}"#
                            .to_owned(),
                    )
                }
            },
        ),
    );
    tokio::spawn(async move {
        axum::serve(listener, router).await.expect("serve");
    });
    (address, calls)
}

fn control_file(data: &Path, address: SocketAddr, token: &str) {
    let file = rd_api::local_control::ControlFile {
        address: address.to_string(),
        token: token.to_owned(),
        pid: 4242,
    };
    std::fs::write(
        data.join(rd_api::local_control::FILE),
        serde_json::to_vec(&file).expect("json"),
    )
    .expect("write");
}

/// An installation whose password is [`OLD`], with two sessions, an authenticator app and a
/// passkey.
async fn installation(data: &Path) -> PathBuf {
    let path = data.join("rdownloader.sqlite3");
    let database = rd_db::Database::open(&path).await.expect("database");
    password_reset::store_admin_password(&database, OLD)
        .await
        .map_err(refusal)
        .expect("password");
    for digest in ["first", "second"] {
        database
            .create_session(
                rd_core::SessionId::new(),
                digest.to_owned(),
                None,
                Some("192.0.2.7".to_owned()),
                24,
            )
            .await
            .expect("session");
    }
    for (kind, reference) in [
        (rd_core::MfaKind::Totp, "vault://totp"),
        (rd_core::MfaKind::Webauthn, "vault://passkey"),
    ] {
        database
            .create_mfa_credential(
                rd_core::MfaCredentialId::new(),
                kind,
                "phone".to_owned(),
                reference.to_owned(),
            )
            .await
            .expect("factor");
    }
    database.close().await.expect("close");
    path
}

/// What a stopped installation holds after the command.
struct Stored {
    old_matches: bool,
    new_matches: bool,
    sessions: usize,
    factors: Vec<rd_core::MfaKind>,
    records: Vec<rd_db::AuditRecord>,
}

async fn read_back(path: &Path) -> Stored {
    let database = rd_db::Database::open(path).await.expect("database");
    let matches = |password: &'static str| {
        let database = database.clone();
        async move {
            password_reset::admin_password_matches(&database, password)
                .await
                .map_err(refusal)
                .expect("match")
        }
    };
    let stored = Stored {
        old_matches: matches(OLD).await,
        new_matches: matches(NEW).await,
        sessions: database
            .list_sessions(rd_core::SessionLimits::default())
            .await
            .expect("sessions")
            .len(),
        factors: database
            .list_mfa_credentials()
            .await
            .expect("factors")
            .into_iter()
            .map(|factor| factor.kind)
            .collect(),
        records: database
            .query_audit_records(&rd_db::AuditQuery {
                action: Some(rd_core::AuditAction::PasswordResetLocal),
                limit: 50,
                ..rd_db::AuditQuery::default()
            })
            .await
            .expect("audit"),
    };
    database.close().await.expect("close");
    stored
}

/// The service running: the command asks it over the local control token, once, with the
/// password and the switches, and leaves the database to it.
#[tokio::test]
async fn with_the_service_running_the_command_asks_it_over_the_local_control_token() {
    let directory = tempfile::tempdir().expect("tempdir");
    let data = directory.path();
    let database = installation(data).await;
    let (address, calls) = service("the-token").await;
    control_file(data, address, "the-token");

    let outcome = reset_password(&database, data, NEW, true, true)
        .await
        .expect("reset");
    assert_eq!(
        outcome,
        Outcome {
            reached: Reached::Service,
            sessions_ended: 2,
            password_login_off: true,
            removed_material: Vec::new(),
        }
    );
    let calls = calls.lock().expect("calls").clone();
    assert_eq!(
        calls,
        [serde_json::json!({
            "new_password": NEW,
            "disable_totp": true,
            "prompted": true,
        })]
    );
    // The service writes it, not the command.
    let stored = read_back(&database).await;
    assert!(stored.old_matches && !stored.new_matches);
    assert_eq!(stored.sessions, 2);
    assert!(stored.records.is_empty());
}

/// The service stopped: the password goes into the database, every session ends, the second
/// factors stay, and one record says so — without the password.
#[tokio::test]
async fn with_the_service_stopped_the_command_writes_the_database() {
    let directory = tempfile::tempdir().expect("tempdir");
    let data = directory.path();
    let database = installation(data).await;

    let outcome = reset_password(&database, data, NEW, false, false)
        .await
        .expect("reset");
    assert_eq!(
        outcome,
        Outcome {
            reached: Reached::Database,
            sessions_ended: 2,
            password_login_off: false,
            removed_material: Vec::new(),
        }
    );
    let stored = read_back(&database).await;
    assert!(!stored.old_matches, "the forgotten password still matches");
    assert!(stored.new_matches, "the new password does not match");
    assert_eq!(stored.sessions, 0, "a session survived the reset");
    assert_eq!(stored.factors.len(), 2, "{:?}", stored.factors);
    let [record] = stored.records.as_slice() else {
        panic!("one record expected: {:?}", stored.records);
    };
    assert_eq!(record.actor_label.as_deref(), Some("cli"));
    assert_eq!(record.details["path"], "database");
    assert_eq!(record.details["source"], "generated");
    assert_eq!(record.details["sessions_ended"], "2");
    assert_eq!(record.details["totp"], "kept");
    assert_eq!(record.details["api_tokens"], "kept");
    let written = serde_json::to_string(record).expect("json");
    assert!(!written.contains(NEW), "the password reached the record");
}

/// `--disable-totp` is for a lost phone: the authenticator app goes, the passkey stays, and the
/// vault reference comes back for the command to delete.
#[tokio::test]
async fn disabling_totp_removes_the_authenticator_app_and_keeps_the_passkey() {
    let directory = tempfile::tempdir().expect("tempdir");
    let data = directory.path();
    let database = installation(data).await;

    let outcome = reset_password(&database, data, NEW, false, true)
        .await
        .expect("reset");
    assert_eq!(outcome.removed_material, ["vault://totp"]);
    let stored = read_back(&database).await;
    assert_eq!(stored.factors, [rd_core::MfaKind::Webauthn]);
    assert_eq!(stored.records[0].details["totp"], "removed");
}

/// A control file nobody answers for is a service ended by force: the database is written.
#[tokio::test]
async fn a_left_over_control_file_falls_back_to_the_database() {
    let directory = tempfile::tempdir().expect("tempdir");
    let data = directory.path();
    let database = installation(data).await;
    let address = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind")
        .local_addr()
        .expect("address");
    control_file(data, address, "the-token");

    let outcome = reset_password(&database, data, NEW, false, false)
        .await
        .expect("reset");
    assert_eq!(outcome.reached, Reached::Database);
    assert!(read_back(&database).await.new_matches);
}

/// A password the policy refuses never travels, and changes nothing.
#[tokio::test]
async fn a_password_the_policy_refuses_reaches_neither_way() {
    let directory = tempfile::tempdir().expect("tempdir");
    let data = directory.path();
    let database = installation(data).await;
    let (address, calls) = service("the-token").await;
    control_file(data, address, "the-token");

    let error = reset_password(&database, data, "short", true, false)
        .await
        .expect_err("refused");
    assert!(
        error.to_string().contains("auth.password_too_short"),
        "{error}"
    );
    assert!(calls.lock().expect("calls").is_empty());
    std::fs::remove_file(data.join(rd_api::local_control::FILE)).expect("remove");
    assert!(
        reset_password(&database, data, "short", true, false)
            .await
            .is_err()
    );
    assert!(read_back(&database).await.old_matches);
}

/// The first password belongs to the setup in the web interface, not to this command.
#[tokio::test]
async fn an_installation_without_a_password_is_refused() {
    let directory = tempfile::tempdir().expect("tempdir");
    let data = directory.path();
    let path = data.join("rdownloader.sqlite3");
    rd_db::Database::open(&path)
        .await
        .expect("database")
        .close()
        .await
        .expect("close");

    let error = reset_password(&path, data, NEW, false, false)
        .await
        .expect_err("refused");
    assert!(
        error
            .to_string()
            .contains("auth.password_reset_setup_pending"),
        "{error}"
    );
    let stored = read_back(&path).await;
    assert!(!stored.new_matches);
    assert!(stored.records.is_empty());
}

#[tokio::test]
async fn a_missing_database_is_not_created() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("nothing-here.sqlite3");
    assert!(
        reset_password(&path, directory.path(), NEW, false, false)
            .await
            .is_err()
    );
    assert!(!path.exists());
}

/// `--prompt` without a terminal is refused before anything is read: a password piped in would
/// sit in a shell history or a script.
#[test]
fn prompting_needs_a_terminal() {
    let error = chosen_password(false, |_| panic!("nothing may be read"))
        .expect_err("refused without a terminal");
    assert_eq!(
        error
            .downcast_ref::<CommandError>()
            .map(|command| command.failure),
        Some(Failure::Usage)
    );
}

#[test]
fn a_prompted_password_is_typed_twice_and_judged_by_the_policy() {
    let typed = |answers: [&'static str; 2]| {
        let mut answers = answers.into_iter();
        move |_: &str| {
            Ok::<_, anyhow::Error>(answers.next().expect("asked once too often").to_owned())
        }
    };
    assert_eq!(
        chosen_password(true, typed([NEW, NEW])).expect("chosen"),
        NEW
    );
    assert!(chosen_password(true, typed([NEW, OLD])).is_err());
    let error = chosen_password(true, typed(["short", "short"])).expect_err("policy");
    assert!(
        error.to_string().contains("auth.password_too_short"),
        "{error}"
    );
}
