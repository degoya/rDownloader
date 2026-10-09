//! The one-shot subcommands: they run once, do their one thing and end the process. Nothing
//! here belongs to the long-lived agent.

use anyhow::{Context, Result};

use crate::{
    agent_settings,
    cli::{
        ConfigureArgs, ConnectionArgs, HandleArgs, IntegrationArgs, IntegrationCommand, OpenArgs,
    },
    client::{CaptureClient, ForeignListener},
    clipboard::{self, HandOver},
    config, os_integration, scheme,
};

pub(crate) fn configure(args: ConfigureArgs) -> Result<()> {
    let token = match args.token {
        Some(token) => token,
        None => read_token_from_stdin()?,
    };
    config::save(&args.service, &token, args.allow_insecure_service)?;
    println!("Capture connection stored securely: {}", args.service);
    Ok(())
}

/// Takes the capture token off standard input.
///
/// Reads to the end rather than one line, so `configure --token-stdin < token.txt` and a pipe
/// from a password manager both work; the value is trimmed, so a trailing newline is not part
/// of the token.
fn read_token_from_stdin() -> Result<String> {
    use std::io::Read;

    let mut token = String::new();
    std::io::stdin()
        .read_to_string(&mut token)
        .context("read the capture token from standard input")?;
    let token = token.trim().to_owned();
    if token.is_empty() {
        anyhow::bail!("no capture token arrived on standard input");
    }
    Ok(token)
}

/// The client of a one-shot command, connected as the running agent is.
fn connected(args: ConnectionArgs) -> Result<CaptureClient> {
    let connection = config::load(args.service, args.token)?;
    CaptureClient::new(connection.service, connection.token)
}

/// `pause` and `resume` (RD-1180-01): switch clipboard watching at the service. A running agent
/// follows within seconds, and every later start keeps it.
pub(crate) async fn clipboard_watch(args: ConnectionArgs, paused: bool) -> Result<()> {
    let client = connected(args)?;
    let settings = client
        .set_clipboard_paused(paused)
        .await
        .context("the service did not take the switch; nothing was changed")?;
    agent_settings::store_cached(config::config_directory().ok().as_deref(), &settings);
    println!("{}", clipboard_line(&settings));
    Ok(())
}

/// `status`: the clipboard pause and the shortcuts, from the service or, while it does not
/// answer, as this machine last knew them.
pub(crate) async fn status(args: ConnectionArgs) -> Result<()> {
    let client = connected(args)?;
    let settings = match client.agent_settings().await {
        Ok(settings) => settings,
        Err(error) => {
            eprintln!("The service did not answer ({error}); as last known here:");
            agent_settings::load_cached(config::config_directory().ok().as_deref())
        }
    };
    println!("{}", clipboard_line(&settings));
    println!("Shortcuts (CmdOrCtrl is Ctrl, Cmd on a Mac):");
    for command in rd_core::CaptureCommand::ALL {
        println!(
            "  {:<16} {}",
            command.as_str(),
            settings.shortcuts.get(command).unwrap_or("-")
        );
    }
    Ok(())
}

/// `send-clipboard` (RD-1180-03): the tray's "Hand over clipboard now".
pub(crate) async fn send_clipboard(args: ConnectionArgs) -> Result<()> {
    let client = connected(args)?;
    let outcome = clipboard::hand_over_once(&client).await;
    match outcome {
        HandOver::Delivered(_) | HandOver::Listed(_) | HandOver::NoLinks | HandOver::Concealed => {
            println!("{}", outcome.message());
            Ok(())
        }
        _ => anyhow::bail!("{}", outcome.message()),
    }
}

fn clipboard_line(settings: &rd_core::CaptureAgentSettings) -> &'static str {
    if settings.clipboard_paused {
        "Clipboard watching: paused"
    } else {
        "Clipboard watching: on"
    }
}

/// Reads a file, refusing it the moment it turns out to be bigger than `limit`.
///
/// The limit used to be measured with `metadata()` and the file read afterwards with no limit at
/// all, which measured one file and read another: between the two calls a file can grow and a
/// symlink can be pointed somewhere else, and the 64 MiB cap was then simply not there. Reading
/// one byte past the limit and refusing on that byte closes the window -- nothing beyond it is
/// ever pulled into memory. `open` is reachable both through the `.nzb` file association and
/// through `rdownloader://`, so its input is not the user's to vouch for (RD-109-03).
async fn read_at_most(path: &std::path::Path, limit: usize) -> Result<Vec<u8>> {
    use tokio::io::AsyncReadExt;

    let file = tokio::fs::File::open(path)
        .await
        .with_context(|| format!("read {}", path.display()))?;
    let mut content = Vec::new();
    let read = u64::try_from(limit).unwrap_or(u64::MAX).saturating_add(1);
    file.take(read)
        .read_to_end(&mut content)
        .await
        .with_context(|| format!("read {}", path.display()))?;
    if content.len() > limit {
        anyhow::bail!("{} exceeds {limit} bytes", path.display());
    }
    Ok(content)
}

/// Imports one `.nzb` file: straight to the service, with the pairing token, once the service
/// address has answered as rDownloader (RD-1200-03). It used to go to the Click'n'Load port
/// without any credential, to whoever listened there.
pub(crate) async fn open(args: OpenArgs) -> Result<()> {
    let content = read_at_most(&args.path, rd_collector::MAX_NZB_BYTES).await?;
    let name = args
        .path
        .file_name()
        .and_then(|value| value.to_str())
        .context("NZB filename is not Unicode")?
        .to_owned();
    let client = connected(args.connection)?;
    told(client.upload_nzb_bytes(name, content).await).await?;
    println!("NZB handed to the LinkGrabber: {}", args.path.display());
    Ok(())
}

/// Acts on one `rdownloader://` address.
///
/// Everything the address is allowed to mean is decided in `scheme::parse`; this only hands the
/// result to the service, through the same capture routes the running agent uses. No new way
/// into the service is opened here.
pub(crate) async fn handle(args: HandleArgs) -> Result<()> {
    match scheme::parse(&args.url)? {
        scheme::Action::Links(links) => {
            let urls = rd_collector::extract_urls(&links.join("\n"));
            if urls.is_empty() {
                anyhow::bail!("the address names no link the collector accepts");
            }
            let count = urls.len();
            let client = connected(args.connection)?;
            told(
                client
                    .submit_links(urls, "click_and_load", None, None)
                    .await
                    .map(|_| ()),
            )
            .await?;
            println!("{count} link(s) handed to the LinkGrabber");
            Ok(())
        }
        scheme::Action::OpenFile(path) => {
            open(OpenArgs {
                connection: args.connection,
                path,
            })
            .await
        }
    }
}

/// Passes `outcome` on, and a foreign listener at the service address as a notification too:
/// `open` and `handle` are started by the desktop, where nobody reads standard error.
async fn told(outcome: Result<()>) -> Result<()> {
    if let Err(error) = &outcome
        && error.is::<ForeignListener>()
    {
        crate::notify::toast(
            "Not handed over: the program at the service address is not rDownloader".to_owned(),
        )
        .await;
    }
    outcome
}

pub(crate) fn integration(args: IntegrationArgs, kind: os_integration::Kind) -> Result<()> {
    // The alias a package manager keeps across updates, not this version's folder: the handler
    // is started long after `scoop update` or `brew upgrade` may have replaced it.
    let executable = rd_autostart::stable_executable_path(
        &std::env::current_exe().context("locate capture executable")?,
    );
    match args.command {
        IntegrationCommand::Install => os_integration::install(kind, &executable),
        IntegrationCommand::Remove => os_integration::remove(kind),
    }
}

pub(crate) fn autostart(args: IntegrationArgs) -> Result<()> {
    match args.command {
        IntegrationCommand::Install => {
            // `is_paired`, not `load`: this only has to know whether there is anything to
            // connect with, and `load` reads the keyring for it -- which is a second macOS
            // Keychain prompt on top of the agent's own, for an answer it does not need
            // (RD-109-04).
            if !config::is_paired(None) {
                anyhow::bail!(
                    "capture agent is not configured; run \
                     `rdownloader-capture configure --token-stdin` first"
                );
            }
            let executable = std::env::current_exe().context("locate capture executable")?;
            rd_autostart::install(rd_autostart::Target::Capture, &executable)?;
            println!("rDownloader capture autostart installed for the next login.");
        }
        IntegrationCommand::Remove => {
            rd_autostart::remove(rd_autostart::Target::Capture)?;
            println!("rDownloader capture autostart removed.");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::read_at_most;

    /// The size cap has to hold against the file that is actually read, not against the one
    /// `metadata()` happened to see a moment earlier (RD-109-03).
    #[tokio::test]
    async fn a_file_over_the_limit_is_refused_while_it_is_read() {
        let path = std::env::temp_dir().join(format!(
            "rd-capture-read-at-most-{}.bin",
            std::process::id()
        ));
        std::fs::write(&path, vec![b'x'; 100]).expect("write the sample");

        let refused = read_at_most(&path, 50)
            .await
            .expect_err("a file over the limit is not read");
        assert!(
            refused.to_string().contains("exceeds 50 bytes"),
            "{refused}"
        );

        // Exactly at the limit still reads, so the boundary is inclusive rather than off by one.
        assert_eq!(
            read_at_most(&path, 100).await.expect("at the limit").len(),
            100
        );
        assert_eq!(
            read_at_most(&path, 4096)
                .await
                .expect("under the limit")
                .len(),
            100
        );

        std::fs::remove_file(&path).expect("clean up the sample");
    }
}
