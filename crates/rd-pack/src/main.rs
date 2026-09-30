//! `rd-pack`: packages, verifies and indexes plugins and verifies the site-rule file, without
//! building the service (RD-150-20).
//!
//! The same commands as `rdownloader plugin …` and `rdownloader site-rules …`, word for word, so
//! a script swaps the binary and keeps its arguments.

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(
    name = "rd-pack",
    version,
    about = "Packages, signs and verifies rDownloader plugins and release files"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Packages, verifies, checks and indexes plugin packages; scaffolds a new plugin.
    #[command(subcommand)]
    Plugin(rd_pack::plugin::PluginCommand),
    /// Signs and verifies the rule file that recognises release pages.
    #[command(subcommand)]
    SiteRules(rd_pack::site_rules::SiteRulesCommand),
    /// Builds and verifies the signed application update manifest.
    #[command(subcommand)]
    Update(rd_pack::update_manifest::UpdateCommand),
}

#[tokio::main]
async fn main() -> Result<()> {
    // Diagnostics to stderr, for the reason `rdownloader` gives: `plugin conformance --json`
    // promises that stdout carries the document and nothing else.
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("rd_=info")),
        )
        .with_writer(std::io::stderr)
        .init();
    match Cli::parse().command {
        Command::Plugin(command) => rd_pack::plugin::run(command).await,
        Command::SiteRules(command) => rd_pack::site_rules::run(command).await,
        Command::Update(command) => rd_pack::update_manifest::run(command).await,
    }
}
