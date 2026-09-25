//! `rdownloader site-rules ...`: the publishing side of the rule pack (RD-110-04).
//!
//! Verification is in `rd-siterules`; this is the other half, kept next to `tools_cli` so
//! both signed documents are produced the same way. `sign` is what writes
//! `crates/rd-siterules/resources/site-rules.json`, and since RD-130-07 that file is no longer
//! compiled in but carried by every release as `rdownloader-site-rules.json`; `verify` is the
//! check the release build runs over it before it is published, the same one the import runs.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Args, Subcommand};

#[derive(Args)]
pub struct SiteRulesArgs {
    #[command(subcommand)]
    command: SiteRulesCommand,
}

#[derive(Subcommand)]
enum SiteRulesCommand {
    /// Signs a rule-pack payload with the site-rules key.
    Sign(SignArgs),
    /// Verifies a signed rule file against this build's site-rules root, as the import does.
    Verify(VerifyArgs),
}

#[derive(Args)]
struct VerifyArgs {
    /// The signed rule file.
    file: PathBuf,
}

#[derive(Args)]
struct SignArgs {
    /// The unsigned payload: `format_version`, `sequence`, `issued_at`, `rules`.
    #[arg(long)]
    input: PathBuf,
    /// Where the signed document is written.
    #[arg(long)]
    output: PathBuf,
    /// PKCS#8 PEM private key, as `rdownloader plugin keygen --role site-rules` writes it.
    #[arg(long)]
    key: PathBuf,
    /// The `key_id` the document names. Must match an entry in `EMBEDDED_KEYS`.
    #[arg(long, default_value = rd_sign::SITE_RULES_KEY_ID)]
    key_id: String,
}

pub async fn run(args: SiteRulesArgs) -> Result<()> {
    match args.command {
        SiteRulesCommand::Sign(args) => sign(&args).await,
        SiteRulesCommand::Verify(args) => verify(&args).await,
    }
}

/// Every rule this installation consults, for the crawler selection (RD-110-06) and for the
/// self-test (RD-110-09).
///
/// The assembly itself -- the stored rules and the switches RD-110-08 records about them --
/// lives in `rd_api::site_rules_service`, because the settings
/// page has to produce the same answer after every write and two copies of that rule would
/// drift. This stays as the name `serve` and `doctor` already call.
pub async fn load_catalogue(database: &rd_db::Database) -> rd_siterules::Catalogue {
    rd_api::site_rule_catalogue(database).await
}

async fn sign(args: &SignArgs) -> Result<()> {
    let payload = tokio::fs::read(&args.input)
        .await
        .with_context(|| format!("read rule pack payload {}", args.input.display()))?;
    let pack: rd_siterules::RulePack =
        serde_json::from_slice(&payload).context("parse rule pack payload")?;
    let pem = tokio::fs::read_to_string(&args.key)
        .await
        .with_context(|| format!("read signing key {}", args.key.display()))?;
    let key = rd_plugin_host::load_signing_key_pem(&pem)?;
    let signed = rd_siterules::sign(&args.key_id, &key, &pack)?;
    tokio::fs::write(&args.output, &signed)
        .await
        .with_context(|| format!("write {}", args.output.display()))?;
    println!(
        "signed {} rules as sequence {} into {}",
        pack.rules.len(),
        pack.sequence,
        args.output.display()
    );
    Ok(())
}

/// Refuses a file that would not import, and names what one that would carries.
///
/// A release that publishes the rule file has to know the file verifies under the root this
/// build compiles in; a signature by the wrong key, or a payload edited after signing, is
/// otherwise found by the first person who imports it.
async fn verify(args: &VerifyArgs) -> Result<()> {
    let bytes = tokio::fs::read(&args.file)
        .await
        .with_context(|| format!("read rule file {}", args.file.display()))?;
    let pack = rd_siterules::verify(&bytes, None, chrono::Utc::now()).map_err(|error| {
        anyhow::anyhow!(
            "{} does not verify ({}): {error}",
            args.file.display(),
            error.code()
        )
    })?;
    let mut groups: Vec<&str> = pack.rules.iter().map(|rule| rule.group.as_str()).collect();
    groups.sort_unstable();
    groups.dedup();
    println!(
        "{} verifies: {} rules as sequence {}, groups {}",
        args.file.display(),
        pack.rules.len(),
        pack.sequence,
        groups.join(", ")
    );
    Ok(())
}
