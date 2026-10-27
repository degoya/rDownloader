//! The one-shot subcommands: they run once, do their one thing and end the process. Nothing
//! here belongs to the long-lived agent.

use anyhow::{Context, Result};

use crate::{
    agent_settings,
    cli::{
        ConfigureArgs, ConnectionArgs, HandleArgs, IntegrationArgs, IntegrationCommand, LinkArgs,
        OpenArgs,
    },
    client::{CaptureClient, ForeignListener, ServiceRefusal},
    clipboard::{self, HandOver},
    config, desktop_shortcut,
    hand_over::{self, FileKind},
    link_phase, os_integration, scheme,
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

/// `link` (RD-1390-02): opens the running agent's link page, or prints its address.
pub(crate) fn link(args: LinkArgs) -> Result<()> {
    let directory = config::config_directory()?;
    let page = running_page(&directory)?;
    // Through `link.html`: the address carries the secret, and a browser's command line is
    // readable by every local account.
    if !args.print {
        match link_phase::open_redirect(&directory) {
            Ok(()) => return Ok(()),
            Err(error) => {
                tracing::warn!(%error, "could not open the browser; open the address below yourself");
            }
        }
    }
    println!("{page}");
    Ok(())
}

/// The page of the agent that runs, or what to do when none does.
pub(crate) fn running_page(directory: &std::path::Path) -> Result<url::Url> {
    link_phase::page_of_running_agent(directory)
        .context("The agent is not running; start it first (rdownloader-capture run)")
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

/// Hands files to the service: straight to it, with the pairing token, once the service address
/// has answered as rDownloader (RD-1200-03). It used to go to the Click'n'Load port without any
/// credential, to whoever listened there.
///
/// An NZB, a link container or a torrent, each to the route that reads it (RD-1260-09). Several
/// at once -- a drop on the desktop shortcut, Send to -- end in one notification with the count;
/// a file of another type is refused by name with `capture.file_type_unsupported`, and the others
/// still go.
pub(crate) async fn open(args: OpenArgs) -> Result<()> {
    let client = connected(args.connection)?;
    let mut outcome = hand_over::Outcome::default();
    for path in &args.paths {
        let name = hand_over::display_name(path);
        match hand_over_file(&client, path, &name).await {
            Ok(()) => outcome.handed.push(name),
            // Nothing goes to whoever listens there; one notification says so, for all of them.
            Err(error) if error.is::<ForeignListener>() => return told(Err(error)).await,
            Err(error) => outcome.refused.push((name, refusal_reason(&error))),
        }
    }
    let summary = outcome.summary();
    println!("{summary}");
    if outcome.tells() {
        crate::notify::toast(summary.clone()).await;
    }
    if outcome.refused.is_empty() {
        Ok(())
    } else {
        anyhow::bail!("{summary}")
    }
}

/// One file to the route that reads its kind.
async fn hand_over_file(client: &CaptureClient, path: &std::path::Path, name: &str) -> Result<()> {
    let kind = hand_over::kind_of(path)?;
    let content = read_at_most(path, rd_collector::MAX_NZB_BYTES).await?;
    let name = name.to_owned();
    match kind {
        FileKind::Nzb => client.upload_nzb_bytes(name, content).await,
        FileKind::Container | FileKind::Torrent => client.upload_file(name, content).await,
    }
}

/// What a refused file is reported with: the service's stable code where it sent one, the
/// agent's own for a type it does not take, otherwise the cause.
fn refusal_reason(error: &anyhow::Error) -> String {
    if let Some(code) = error
        .downcast_ref::<ServiceRefusal>()
        .and_then(ServiceRefusal::code)
    {
        return code.to_owned();
    }
    if error.is::<hand_over::Unsupported>() {
        return hand_over::UNSUPPORTED.to_owned();
    }
    error.root_cause().to_string()
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
                paths: vec![path],
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

/// `drop-shortcut` and `send-to` (RD-1260-09): the desktop's two drop targets on Windows, for the
/// agent at the path a package manager keeps across updates, as `integration` registers it.
pub(crate) fn shortcut(args: IntegrationArgs, place: desktop_shortcut::Place) -> Result<()> {
    match args.command {
        IntegrationCommand::Install => {
            let executable = rd_autostart::stable_executable_path(
                &std::env::current_exe().context("locate capture executable")?,
            );
            desktop_shortcut::install(place, &executable)?;
            println!("rDownloader's {} installed.", place.label());
        }
        IntegrationCommand::Remove => {
            desktop_shortcut::remove(place)?;
            println!("rDownloader's {} removed.", place.label());
        }
    }
    Ok(())
}

pub(crate) fn autostart(args: IntegrationArgs) -> Result<()> {
    match args.command {
        IntegrationCommand::Install => {
            // Not tied to a pairing (RD-1390-03): an unlinked agent starts and offers its link
            // page (RD-1390-02), so the MSI's AUTOSTART=1 registers it on a fresh install.
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
