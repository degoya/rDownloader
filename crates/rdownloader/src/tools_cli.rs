//! `rdownloader tools …`: publishing side of the managed external tools (RD-102-02).
//!
//! Verification is in `rd-tools`; this is the other half, and it lives here rather than in a
//! build script so both halves share one definition of the domain string and the payload
//! shape. It is what produces `crates/rd-tools/resources/tools-manifest.json` and any
//! manifest served from a URL.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};

#[derive(Args)]
pub struct ToolsArgs {
    #[command(subcommand)]
    command: ToolsCommand,
}

#[derive(Subcommand)]
enum ToolsCommand {
    /// Signs a tool-manifest payload with the tool-manifest key.
    SignManifest(SignManifestArgs),
}

#[derive(Args)]
struct SignManifestArgs {
    /// The unsigned payload: `schema_version`, `sequence`, `issued_at`, `tools`.
    #[arg(long)]
    input: PathBuf,
    /// Where the signed document is written.
    #[arg(long)]
    output: PathBuf,
    /// PKCS#8 PEM private key, as `rdownloader plugin keygen --role tool-manifest` writes it.
    #[arg(long)]
    key: PathBuf,
    /// The `key_id` the document names. Must match an entry in `EMBEDDED_KEYS`.
    #[arg(long, default_value = "rdownloader-tools-v1")]
    key_id: String,
}

pub async fn run(args: ToolsArgs) -> Result<()> {
    match args.command {
        ToolsCommand::SignManifest(args) => sign_manifest(&args).await,
    }
}

async fn sign_manifest(args: &SignManifestArgs) -> Result<()> {
    let payload = tokio::fs::read(&args.input)
        .await
        .with_context(|| format!("read manifest payload {}", args.input.display()))?;
    let manifest: rd_tools::ToolManifest =
        serde_json::from_slice(&payload).context("parse manifest payload")?;
    if manifest.schema_version != rd_tools::TOOL_MANIFEST_SCHEMA_VERSION {
        bail!(
            "payload declares schema version {}, this build signs version {}",
            manifest.schema_version,
            rd_tools::TOOL_MANIFEST_SCHEMA_VERSION
        );
    }
    let pem = tokio::fs::read_to_string(&args.key)
        .await
        .with_context(|| format!("read signing key {}", args.key.display()))?;
    let key = rd_plugin_host::load_signing_key_pem(&pem)?;
    let signed = rd_tools::manifest::sign(&args.key_id, &key, &manifest)?;
    tokio::fs::write(&args.output, &signed)
        .await
        .with_context(|| format!("write {}", args.output.display()))?;
    println!(
        "signed {} tool entries as sequence {} into {}",
        manifest.tools.len(),
        manifest.sequence,
        args.output.display()
    );
    Ok(())
}
