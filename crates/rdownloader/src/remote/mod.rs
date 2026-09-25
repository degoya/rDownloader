//! Queue and LinkGrabber commands against a local or remote server (RD-090-07).

mod client;
mod links_cmd;
mod output;
mod queue_cmd;

use anyhow::Result;
use clap::{Args, Subcommand};

pub(crate) use client::{Client, CommandError, DEFAULT_SERVER, Failure};
pub(crate) use output::Format;

/// Connection options shared by every remote command.
#[derive(Clone, Args)]
pub struct ConnectionArgs {
    /// Server to talk to. Defaults to the local service.
    #[arg(long, env = "RDOWNLOADER_SERVER", default_value = DEFAULT_SERVER, global = true)]
    server: String,
    /// API token. Reading commands work with an `api:read` token.
    #[arg(long, env = "RDOWNLOADER_TOKEN", global = true)]
    token: Option<String>,
    /// Print the server's JSON instead of a table.
    #[arg(long, global = true)]
    json: bool,
    /// Request timeout in seconds.
    #[arg(long, default_value_t = 30, global = true)]
    timeout: u64,
}

impl ConnectionArgs {
    fn connect(&self) -> Result<(Client, Format)> {
        Ok((
            Client::new(&self.server, self.token.clone(), self.timeout)?,
            Format::from_flag(self.json),
        ))
    }
}

#[derive(Args)]
pub struct QueueArgs {
    #[command(subcommand)]
    command: QueueCommand,
    #[command(flatten)]
    connection: ConnectionArgs,
}

#[derive(Subcommand)]
enum QueueCommand {
    /// Lists the queue.
    List,
    /// Shows the queue totals.
    Summary,
    /// Adds one or more URLs directly to the queue.
    Add {
        /// HTTP(S) or magnet URLs.
        #[arg(required = true)]
        urls: Vec<String>,
    },
    /// Pauses downloads by id.
    Pause {
        #[arg(required = true)]
        ids: Vec<String>,
    },
    /// Resumes downloads by id.
    Resume {
        #[arg(required = true)]
        ids: Vec<String>,
    },
    /// Removes downloads by id, deleting their part files.
    Remove {
        #[arg(required = true)]
        ids: Vec<String>,
        /// Confirm the removal. Required, because this deletes partial data.
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Args)]
pub struct LinksArgs {
    #[command(subcommand)]
    command: LinksCommand,
    #[command(flatten)]
    connection: ConnectionArgs,
}

#[derive(Subcommand)]
enum LinksCommand {
    /// Lists the LinkGrabber packages awaiting review.
    List,
    /// Hands links to the LinkGrabber, or with `--enqueue` straight to the queue.
    Add {
        /// Links, or `-` to read them from standard input.
        #[arg(required = true)]
        links: Vec<String>,
        /// Category, by name or id. A name is looked up, which needs the `api:config` scope;
        /// an id does not.
        #[arg(long)]
        category: Option<String>,
        /// Keep every link in one package of this name.
        #[arg(long)]
        package: Option<String>,
        /// Add the links to the download queue directly instead of the LinkGrabber
        /// (`api:intake`, the same as `queue add`).
        #[arg(long)]
        enqueue: bool,
    },
    /// Moves a reviewed package into the queue.
    Enqueue {
        #[arg(required = true)]
        ids: Vec<String>,
    },
    /// Discards a package from the LinkGrabber without queueing it.
    Remove {
        #[arg(required = true)]
        ids: Vec<String>,
        /// Confirm the removal.
        #[arg(long)]
        yes: bool,
    },
}

/// Refuses a destructive command that was not confirmed.
///
/// A flag rather than a prompt: these commands are written into scripts and cron jobs, where
/// a prompt either hangs forever or is answered by accident. Requiring `--yes` makes the
/// intent part of the command line, which is also what a shell history shows later.
fn confirm(yes: bool, count: usize, noun: &str) -> Result<()> {
    if yes {
        return Ok(());
    }
    Err(CommandError::new(
        Failure::Usage,
        format!("refusing to remove {count} {noun}(s) without --yes"),
    )
    .into())
}

/// Runs a `queue` subcommand.
pub async fn queue(args: QueueArgs) -> Result<()> {
    let (client, format) = args.connection.connect()?;
    match args.command {
        QueueCommand::List => queue_cmd::list(&client, format).await,
        QueueCommand::Summary => queue_cmd::summary(&client, format).await,
        QueueCommand::Add { urls } => queue_cmd::add(&client, format, &urls).await,
        QueueCommand::Pause { ids } => queue_cmd::act(&client, format, "pause", &ids).await,
        QueueCommand::Resume { ids } => queue_cmd::act(&client, format, "resume", &ids).await,
        QueueCommand::Remove { ids, yes } => {
            confirm(yes, ids.len(), "download")?;
            queue_cmd::act(&client, format, "remove", &ids).await
        }
    }
}

/// Runs a `links` subcommand.
pub async fn links(args: LinksArgs) -> Result<()> {
    let (client, format) = args.connection.connect()?;
    match args.command {
        LinksCommand::List => links_cmd::list(&client, format).await,
        LinksCommand::Add {
            links,
            category,
            package,
            enqueue,
        } => {
            let target = links_cmd::Target {
                category: category.as_deref(),
                package: package.as_deref(),
                enqueue,
            };
            links_cmd::add(&client, format, &links, target).await
        }
        LinksCommand::Enqueue { ids } => links_cmd::enqueue(&client, format, &ids).await,
        LinksCommand::Remove { ids, yes } => {
            confirm(yes, ids.len(), "package")?;
            links_cmd::remove(&client, format, &ids).await
        }
    }
}

/// Ends the process with the exit code that matches why a remote command failed.
///
/// Printing and exiting here rather than returning the error to `main`: anyhow's default
/// exit code is 1 for everything, and the whole point of [`Failure`] is that a caller can
/// tell an unreachable host from a rejected token without parsing the message.
pub fn finish(result: Result<()>) -> Result<()> {
    let Err(error) = result else { return Ok(()) };
    let failure = error
        .downcast_ref::<CommandError>()
        .map_or(Failure::Other, |command| command.failure);
    eprintln!("Error: {error}");
    std::process::exit(failure.code());
}
