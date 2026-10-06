//! `rdownloader auth reset-password`: a new administrator password without the current one
//! (RD-190-24), for the owner who forgot it.
//!
//! The trust boundary is the data directory. Whoever can read the local control file or write
//! the database beside it holds the installation already; this command turns that into a
//! checked, recorded step. With the service running it asks it over the local control token,
//! with the service stopped it writes the database — the two ways of `auth password-login on`
//! (`crate::auth_cli`). What the reset changes and keeps is `rd_api::password_reset`'s to say.
//!
//! By default the command draws a random password and prints it once; `--prompt` reads one from
//! the terminal instead, twice and without echo, and refuses when standard input is no terminal —
//! a password piped in would sit in a shell history or a script.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow, bail};
use clap::Args;
use rd_api::password_reset;

use crate::auth_cli::{Reached, ask_service, data_directory_of, open_existing};
use crate::remote::{CommandError, Failure};

/// The one account an installation has until there are several (RD-R08).
const ACCOUNT: &str = "admin";

#[derive(Args)]
pub(crate) struct ResetPasswordArgs {
    /// The account whose password is reset; an installation has one, `admin`.
    #[arg(long, default_value = ACCOUNT)]
    user: String,
    /// Type the new password (twice, not shown) instead of having a random one printed.
    #[arg(long)]
    prompt: bool,
    /// Also remove the authenticator app (TOTP) and its recovery codes, for a lost phone.
    /// Passkeys stay either way.
    #[arg(long)]
    disable_totp: bool,
    /// The SQLite database file of the service; its folder holds the local control file.
    #[arg(
        long,
        env = "RDOWNLOADER_DATABASE",
        default_value = "data/rdownloader.sqlite3"
    )]
    database: PathBuf,
}

/// What one reset did, as the command reports it.
#[derive(Debug, Eq, PartialEq)]
pub(crate) struct Outcome {
    pub reached: Reached,
    pub sessions_ended: u64,
    pub password_login_off: bool,
    /// The vault references of the authenticator apps a stopped service's reset removed, for
    /// the command to delete; the running service deletes its own.
    pub removed_material: Vec<String>,
}

pub(crate) async fn run(args: ResetPasswordArgs) -> Result<()> {
    if args.user != ACCOUNT {
        return Err(CommandError::new(
            Failure::Usage,
            format!(
                "there is no account named '{}'; this installation has one, '{ACCOUNT}'",
                args.user
            ),
        )
        .into());
    }
    let password = if args.prompt {
        chosen_password(std::io::stdin().is_terminal(), read_hidden)?
    } else {
        password_reset::one_time_password()
    };
    let data_directory = data_directory_of(&args.database);
    let outcome = reset_password(
        &args.database,
        &data_directory,
        &password,
        args.prompt,
        args.disable_totp,
    )
    .await?;
    remove_material(&data_directory, &outcome.removed_material).await;
    report(
        &outcome,
        (!args.prompt).then_some(password.as_str()),
        args.disable_totp,
    );
    Ok(())
}

/// Sets `password` for the service of `data_directory`: over the local control token while it
/// runs, in `database` while it is stopped.
pub(crate) async fn reset_password(
    database: &Path,
    data_directory: &Path,
    password: &str,
    prompted: bool,
    disable_totp: bool,
) -> Result<Outcome> {
    // Judged here as well, so a password the policy refuses never travels at all.
    password_reset::validate_password(password).map_err(refusal)?;
    let request = serde_json::json!({
        "new_password": password,
        "disable_totp": disable_totp,
        "prompted": prompted,
    });
    if let Some(answer) =
        ask_service(data_directory, "/api/v1/auth/password/reset", &request).await?
    {
        let params = &answer["params"];
        return Ok(Outcome {
            reached: Reached::Service,
            sessions_ended: params["sessions_ended"]
                .as_str()
                .and_then(|count| count.parse().ok())
                .unwrap_or(0),
            password_login_off: params["password_login"] == "off",
            removed_material: Vec::new(),
        });
    }
    let database = open_existing(database).await?;
    let written = write_database(&database, password, prompted, disable_totp).await;
    database.close().await?;
    let reset = written?;
    Ok(Outcome {
        reached: Reached::Database,
        sessions_ended: reset.sessions_ended,
        password_login_off: reset.password_login_off,
        removed_material: reset.removed_material,
    })
}

/// The reset and its audit record, straight into a stopped service's database.
async fn write_database(
    database: &rd_db::Database,
    password: &str,
    prompted: bool,
    disable_totp: bool,
) -> Result<password_reset::Reset> {
    let reset = password_reset::reset(database, password, disable_totp)
        .await
        .map_err(refusal)?;
    database
        .append_audit_record(rd_api::audit::to_record(
            password_reset::audit_event(
                &reset,
                password_reset::Path::Database,
                prompted,
                disable_totp,
            )
            .actor(rd_api::audit::Actor::cli()),
        ))
        .await?;
    Ok(reset)
}

/// A refusal of the service's rules as one line, its stable code included.
fn refusal(error: rd_api::ApiError) -> anyhow::Error {
    anyhow!("{} [{}]", error.message(), error.code())
}

/// The password the owner types: only from a terminal, twice, under the ordinary policy.
pub(crate) fn chosen_password(
    terminal: bool,
    mut read: impl FnMut(&str) -> Result<String>,
) -> Result<String> {
    if !terminal {
        return Err(CommandError::new(
            Failure::Usage,
            "--prompt reads the password from a terminal, and standard input is none; run the \
             command in a terminal, or leave --prompt out to have a random password printed",
        )
        .into());
    }
    let first = read("New administrator password: ")?;
    password_reset::validate_password(&first).map_err(refusal)?;
    let second = read("The same password again: ")?;
    if first != second {
        bail!("the two passwords differ; nothing was changed");
    }
    Ok(first)
}

/// One line from the terminal with its echo switched off, the prompt on standard error.
#[cfg(unix)]
fn read_hidden(prompt: &str) -> Result<String> {
    use std::io::{BufRead, Write};

    use rustix::termios::{LocalModes, OptionalActions, tcgetattr, tcsetattr};

    let stdin = std::io::stdin();
    let original = tcgetattr(&stdin)?;
    let mut hidden = original.clone();
    hidden.local_modes.remove(LocalModes::ECHO);
    // The Enter key still moves the cursor on, so the next prompt starts on a line of its own.
    hidden.local_modes.insert(LocalModes::ECHONL);
    eprint!("{prompt}");
    std::io::stderr().flush()?;
    tcsetattr(&stdin, OptionalActions::Now, &hidden)?;
    let mut line = String::new();
    let read = stdin.lock().read_line(&mut line);
    // The echo comes back before either result is looked at.
    let restored = tcsetattr(&stdin, OptionalActions::Now, &original);
    read?;
    restored?;
    Ok(line.trim_end_matches(['\r', '\n']).to_owned())
}

/// Windows has no safe way to switch a console's echo off without a crate of its own, and a
/// password typed in the clear is not offered: the default, a printed random password, is.
#[cfg(not(unix))]
fn read_hidden(_prompt: &str) -> Result<String> {
    Err(CommandError::new(
        Failure::Usage,
        "--prompt is not available on Windows; leave it out to have a random password printed, \
         then replace it under Settings > Security",
    )
    .into())
}

/// Deletes the vault files behind the authenticator apps a stopped service's reset removed.
/// Their rows are gone already, so a file left behind is unreachable; a failure is said, not
/// fatal.
async fn remove_material(data_directory: &Path, references: &[String]) {
    if references.is_empty() {
        return;
    }
    let vault =
        match rd_secrets::SecretStore::open_with_os_keyring(data_directory.join("secrets")).await {
            Ok(vault) => vault,
            Err(error) => {
                eprintln!("The authenticator app's stored secret could not be removed: {error}");
                return;
            }
        };
    for reference in references {
        if let Err(error) = vault.remove(reference).await {
            eprintln!("The authenticator app's stored secret could not be removed: {error}");
        }
    }
}

fn report(outcome: &Outcome, printed: Option<&str>, disable_totp: bool) {
    match outcome.reached {
        Reached::Service => {
            println!("The administrator password was reset; the running service uses it now.");
        }
        Reached::Database => println!(
            "The administrator password was reset; rDownloader is not running and uses it from \
             its next start."
        ),
    }
    if let Some(password) = printed {
        println!();
        println!("    {password}");
        println!();
        println!(
            "It is shown only this once. Sign in with it, then replace it under Settings > \
             Security."
        );
    }
    let totp = if disable_totp {
        "the authenticator app (TOTP) and its recovery codes were removed"
    } else {
        "the authenticator app (TOTP) stays enrolled"
    };
    println!(
        "{} sessions were ended. API tokens and passkeys stay valid; {totp}.",
        outcome.sessions_ended
    );
    if outcome.password_login_off {
        println!(
            "The password sign-in is switched off for the identity provider; run `rdownloader \
             auth password-login on` to sign in with this password."
        );
    }
}

#[cfg(test)]
#[path = "reset_password_cli_tests.rs"]
mod tests;
