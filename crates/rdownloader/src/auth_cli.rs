//! `rdownloader auth …`: the sign-in steps only the machine the service runs on may take.
//!
//! `auth password-login on` is the way back in once the password sign-in was switched off after a
//! sign-in through the identity provider (RD-190-15, ADR 0021, decision D3); `auth
//! reset-password` (RD-190-24, `reset_password_cli`) is the way back in after the password itself
//! was forgotten.
//!
//! Only this machine can do either, on purpose: a provider that is down, misconfigured or deleted
//! must not lock the owner out, and nothing that reaches the service over the network — no
//! session, no API token — may take the step instead (O-LOCK). With the service running a command
//! asks it over the local control token (`rd_api::local_control`), which opens these routes
//! besides the lifecycle ones and only from this machine; with the service stopped it writes the
//! database, where the next start reads it. Both ways leave an audit record. Switching the
//! password sign-in *off* has no command: it needs a session the provider opened, in the web
//! interface.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use clap::{Args, Subcommand, ValueEnum};

use crate::remote::{Client, CommandError, Failure};
use crate::reset_password_cli::{self, ResetPasswordArgs};

#[derive(Args)]
pub struct AuthArgs {
    #[command(subcommand)]
    command: AuthCommand,
}

#[derive(Subcommand)]
enum AuthCommand {
    /// Switches the password sign-in back on (only `on`; switching it off needs a sign-in
    /// through the identity provider, in the web interface).
    PasswordLogin(PasswordLoginArgs),
    /// Sets a new administrator password without the current one: prints a random one, or reads
    /// one you type (`--prompt`), and ends every session.
    ResetPassword(ResetPasswordArgs),
}

#[derive(Args)]
struct PasswordLoginArgs {
    /// `on`, the only direction this command offers.
    #[arg(value_enum)]
    switch: Switch,
    /// The SQLite database file of the service; its folder holds the local control file.
    #[arg(
        long,
        env = "RDOWNLOADER_DATABASE",
        default_value = "data/rdownloader.sqlite3"
    )]
    database: PathBuf,
}

#[derive(Clone, Copy, ValueEnum)]
enum Switch {
    On,
}

/// Which way the setting reached the service.
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum Reached {
    /// The running service switched it, over the local control token.
    Service,
    /// No service runs; the database holds it for the next start.
    Database,
}

pub async fn run(args: AuthArgs) -> Result<()> {
    let args = match args.command {
        AuthCommand::PasswordLogin(args) => args,
        AuthCommand::ResetPassword(args) => return reset_password_cli::run(args).await,
    };
    let Switch::On = args.switch;
    let data_directory = data_directory_of(&args.database);
    match password_login_on(&args.database, &data_directory).await? {
        Reached::Service => println!("The password sign-in is switched on again."),
        Reached::Database => println!(
            "The password sign-in is switched on again; rDownloader is not running and offers \
             it from its next start."
        ),
    }
    Ok(())
}

/// The folder of `database`, which holds the local control file.
pub(crate) fn data_directory_of(database: &Path) -> PathBuf {
    database
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf()
}

/// Switches the password sign-in on for the service of `data_directory`.
pub(crate) async fn password_login_on(database: &Path, data_directory: &Path) -> Result<Reached> {
    if ask_service(
        data_directory,
        "/api/v1/auth/password-login/on",
        &serde_json::json!({}),
    )
    .await?
    .is_some()
    {
        return Ok(Reached::Service);
    }
    write_database(database).await?;
    Ok(Reached::Database)
}

/// Posts `body` to `route` of the service running for `data_directory`, with its local control
/// token. `None` when no service runs there — no control file, or one nothing answers for.
pub(crate) async fn ask_service<B: serde::Serialize>(
    data_directory: &Path,
    route: &str,
    body: &B,
) -> Result<Option<serde_json::Value>> {
    let Some(control) = rd_api::local_control::read(data_directory)? else {
        return Ok(None);
    };
    let client = Client::local(
        &format!("http://{}", control.address),
        Some(control.token.clone()),
        30,
    )?;
    match client.post(route, body).await {
        Ok(answer) => Ok(Some(answer)),
        // A file left by a process that was ended by force: nothing answers there, so the
        // database is the place, exactly as with no file at all.
        Err(error)
            if error
                .downcast_ref::<CommandError>()
                .is_some_and(|command| command.failure == Failure::Unreachable)
                && rd_api::local_control::read(data_directory)?
                    .is_some_and(|file| file == control) =>
        {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

/// Opens the database of a stopped service, refusing a path where there is none.
pub(crate) async fn open_existing(path: &Path) -> Result<rd_db::Database> {
    // Opening a path that does not exist would create an empty installation and change nothing.
    if !path.is_file() {
        bail!(
            "no rDownloader database at {}; pass --database or set RDOWNLOADER_DATABASE",
            path.display()
        );
    }
    rd_db::Database::open(path).await
}

/// Writes the switch and its audit record straight into a stopped service's database.
async fn write_database(path: &Path) -> Result<()> {
    let database = open_existing(path).await?;
    database
        .set_setting(
            rd_api::oidc_client::PASSWORD_LOGIN_OFF_SETTING.to_owned(),
            serde_json::Value::Bool(false),
        )
        .await?;
    database
        .append_audit_record(rd_api::audit::to_record(
            rd_api::audit::AuditEvent::success(rd_core::AuditAction::PasswordLoginChanged)
                .actor(rd_api::audit::Actor::cli())
                .detail("enabled", true)
                .detail("via", "cli"),
        ))
        .await?;
    database.close().await
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;
    use std::sync::{Arc, Mutex};

    use axum::{
        Router,
        http::{HeaderMap, StatusCode},
        routing::post,
    };

    use super::*;

    const SETTING: &str = rd_api::oidc_client::PASSWORD_LOGIN_OFF_SETTING;

    /// A stand-in for the running service: answers the route with the right token only.
    async fn service(token: &'static str) -> (SocketAddr, Arc<Mutex<u32>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let address = listener.local_addr().expect("address");
        let calls = Arc::new(Mutex::new(0_u32));
        let counted = calls.clone();
        let router = Router::new().route(
            "/api/v1/auth/password-login/on",
            post(move |headers: HeaderMap| {
                let counted = counted.clone();
                async move {
                    if headers
                        .get("authorization")
                        .and_then(|value| value.to_str().ok())
                        != Some(format!("Bearer {token}").as_str())
                    {
                        return (StatusCode::UNAUTHORIZED, "{}".to_owned());
                    }
                    *counted.lock().expect("calls") += 1;
                    (
                        StatusCode::OK,
                        r#"{"code":"auth.password_login_switched_on","message":"on"}"#.to_owned(),
                    )
                }
            }),
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

    async fn switched_off_database(data: &Path) -> PathBuf {
        let path = data.join("rdownloader.sqlite3");
        let database = rd_db::Database::open(&path).await.expect("database");
        database
            .set_setting(SETTING.to_owned(), serde_json::Value::Bool(true))
            .await
            .expect("switch off");
        database.close().await.expect("close");
        path
    }

    async fn read_back(path: &Path) -> (Option<serde_json::Value>, usize) {
        let database = rd_db::Database::open(path).await.expect("database");
        let value = database.get_setting(SETTING).await.expect("setting");
        let records = database
            .query_audit_records(&rd_db::AuditQuery {
                action: Some(rd_core::AuditAction::PasswordLoginChanged),
                limit: 50,
                ..rd_db::AuditQuery::default()
            })
            .await
            .expect("audit")
            .len();
        database.close().await.expect("close");
        (value, records)
    }

    /// O-LOCK, the service running: the command reaches it over the local control token, and
    /// the route is asked exactly once.
    #[tokio::test]
    async fn with_the_service_running_the_command_asks_it_over_the_local_control_token() {
        let directory = tempfile::tempdir().expect("tempdir");
        let data = directory.path();
        let database = switched_off_database(data).await;
        let (address, calls) = service("the-token").await;
        control_file(data, address, "the-token");
        assert_eq!(
            password_login_on(&database, data).await.expect("switched"),
            Reached::Service
        );
        assert_eq!(*calls.lock().expect("calls"), 1);
        // The service wrote it, not the command.
        assert_eq!(
            read_back(&database).await.0,
            Some(serde_json::Value::Bool(true))
        );
    }

    /// O-LOCK, the service stopped: the setting goes into the database, with its record.
    #[tokio::test]
    async fn with_the_service_stopped_the_command_writes_the_database() {
        let directory = tempfile::tempdir().expect("tempdir");
        let data = directory.path();
        let database = switched_off_database(data).await;
        assert_eq!(
            password_login_on(&database, data).await.expect("switched"),
            Reached::Database
        );
        let (value, records) = read_back(&database).await;
        assert_eq!(value, Some(serde_json::Value::Bool(false)));
        assert_eq!(records, 1, "the switch is audited");
    }

    /// A control file nobody answers for is a service ended by force: the database is written.
    #[tokio::test]
    async fn a_left_over_control_file_falls_back_to_the_database() {
        let directory = tempfile::tempdir().expect("tempdir");
        let data = directory.path();
        let database = switched_off_database(data).await;
        let address = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind")
            .local_addr()
            .expect("address");
        control_file(data, address, "the-token");
        assert_eq!(
            password_login_on(&database, data).await.expect("switched"),
            Reached::Database
        );
        assert_eq!(
            read_back(&database).await.0,
            Some(serde_json::Value::Bool(false))
        );
    }

    #[tokio::test]
    async fn a_missing_database_is_not_created() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("nothing-here.sqlite3");
        assert!(password_login_on(&path, directory.path()).await.is_err());
        assert!(!path.exists());
    }
}
