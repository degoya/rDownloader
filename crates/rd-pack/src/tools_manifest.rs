//! `tools …` (`rdownloader` and `rd-pack` alike): the publishing side of the managed external
//! tools (RD-102-02).
//!
//! Verification is in `rd-tools`; this is the other half, and it calls the same `rd_tools`
//! functions so both halves share one definition of the domain string and the payload shape. It
//! is what produces `crates/rd-tools/resources/tools-manifest.json` and any manifest served from
//! a URL. It lives here rather than in the service since RD-1110-14, so
//! `scripts/sign-tools-manifest.sh` signs without building the service.

use std::path::PathBuf;

use anyhow::{Context, Result, anyhow, bail, ensure};
use chrono::Utc;
use clap::{Args, Subcommand};

#[derive(Subcommand)]
pub enum ToolsCommand {
    /// Signs a tool-manifest payload with the tool-manifest key.
    SignManifest(SignManifestArgs),
}

#[derive(Args)]
pub struct SignManifestArgs {
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
    #[arg(long, default_value = rd_sign::TOOL_MANIFEST_KEY_ID)]
    key_id: String,
}

pub async fn run(command: ToolsCommand) -> Result<()> {
    match command {
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
    check_against_root(&args.key_id, &key)?;
    let signed = rd_tools::manifest::sign(&args.key_id, &key, &manifest)?;
    // Round trip under the signing key itself, which runs every check an installation runs on
    // the entries (managed name, https, SHA-256, size) before anything is written.
    let trust = rd_sign::TrustStore::new();
    trust.trust(args.key_id.clone(), key.verifying_key())?;
    rd_tools::manifest::verify_with(&signed, &trust, None, Utc::now())
        .map_err(|error| anyhow!("the signed manifest does not verify: {error}"))?;
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

/// Refuses a key that is not the compiled-in tool-manifest root, so a plugin or update key given
/// by mistake is caught here and not by every installation's check.
fn check_against_root(key_id: &str, key: &rd_sign::SigningKey) -> Result<()> {
    let roots = rd_sign::keys_for_now(rd_sign::Role::ToolManifest);
    match roots.iter().find(|root| root.key_id == key_id) {
        Some(root) => {
            let embedded = rd_sign::decode_public_key(root.public_key)?;
            ensure!(
                embedded == key.verifying_key(),
                "the key given is not the tool-manifest root {key_id} this build embeds \
                 (a plugin or update key by mistake?)"
            );
        }
        None => eprintln!(
            "warning: this build embeds no tool-manifest root named {key_id}; the manifest will \
             not verify against it"
        ),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(clap::Parser)]
    struct Cli {
        #[command(subcommand)]
        command: ToolsCommand,
    }

    fn payload(url: &str) -> String {
        serde_json::json!({
            "schema_version": rd_tools::TOOL_MANIFEST_SCHEMA_VERSION,
            "sequence": 7,
            "issued_at": "2026-10-05T00:00:00Z",
            "tools": [{
                "name": "yt-dlp",
                "version": "2026.08.19",
                "platform": "x86_64-unknown-linux-gnu",
                "url": url,
                "sha256": "a".repeat(64),
                "size": 1024,
            }],
        })
        .to_string()
    }

    /// A payload signed under a throwaway key verifies with that key; an entry an installation
    /// would refuse is refused before anything is written, and so is the wrong key under the
    /// embedded root's id.
    #[tokio::test]
    async fn the_signed_manifest_verifies_and_a_bad_one_is_never_written() {
        use clap::Parser as _;
        let directory =
            std::env::temp_dir().join(format!("rd-pack-tools-manifest-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&directory).expect("directory");
        let key = rd_plugin_host::generate_signing_key();
        let key_file = directory.join("tools.key");
        std::fs::write(&key_file, &key.private_pem).expect("key");
        let path = |path: &std::path::Path| path.display().to_string();
        let sign = |input: &std::path::Path, output: &std::path::Path, key_id: &str| {
            Cli::try_parse_from([
                "rd-pack".to_owned(),
                "sign-manifest".to_owned(),
                "--input".to_owned(),
                path(input),
                "--output".to_owned(),
                path(output),
                "--key".to_owned(),
                path(&key_file),
                "--key-id".to_owned(),
                key_id.to_owned(),
            ])
            .expect("arguments")
            .command
        };

        let good = directory.join("good.json");
        std::fs::write(&good, payload("https://example.test/yt-dlp")).expect("payload");
        let signed = directory.join("signed.json");
        run(sign(&good, &signed, "test-tools-key"))
            .await
            .expect("sign");
        let trust = rd_sign::TrustStore::new();
        trust
            .trust("test-tools-key".to_owned(), key.signing_key.verifying_key())
            .expect("trust");
        let bytes = std::fs::read(&signed).expect("signed");
        let verified =
            rd_tools::manifest::verify_with(&bytes, &trust, None, Utc::now()).expect("verify");
        assert_eq!(verified.sequence, 7);

        let plain = directory.join("plain.json");
        std::fs::write(&plain, payload("http://example.test/yt-dlp")).expect("payload");
        let refused = directory.join("refused.json");
        assert!(run(sign(&plain, &refused, "test-tools-key")).await.is_err());
        assert!(!refused.exists(), "a refused manifest is never written");

        let wrong_root = directory.join("wrong-root.json");
        assert!(
            run(sign(&good, &wrong_root, rd_sign::TOOL_MANIFEST_KEY_ID))
                .await
                .is_err()
        );
        assert!(!wrong_root.exists());
        let _ = std::fs::remove_dir_all(&directory);
    }
}
