//! `plugin index …` (`rdownloader` and `rd-pack` alike): the publishing side of the plugin
//! repository index (RD-140-01).
//!
//! The format and its verification are `rd_plugin_host::index`; this reads a directory of
//! signed `.rdplug` packages, describes each by its `package_digest`, and signs the result with
//! the repository key. The release workflow runs `build` over the packages it has just signed
//! and `verify` over what it wrote, before both are attached to the release.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail, ensure};
use chrono::{Duration, SubsecRound, Utc};
use clap::{Args, Subcommand};
use rd_plugin_host::index::{self, PluginIndex, Revocations};

/// The environment variable the release workflow hands the repository key in.
const KEY_ENV: &str = "RDOWNLOADER_REPOSITORY_SIGNING_KEY";

#[derive(Args)]
pub struct IndexArgs {
    #[command(subcommand)]
    command: IndexCommand,
}

#[derive(Subcommand)]
enum IndexCommand {
    /// Builds and signs the index for a directory of signed `.rdplug` packages.
    Build(BuildArgs),
    /// Verifies a signed index against this build's repository root.
    Verify(VerifyArgs),
}

#[derive(Args)]
struct BuildArgs {
    /// Directory of signed `.rdplug` packages, e.g. `dist/plugins`.
    packages: PathBuf,
    /// Where the signed index is written.
    #[arg(long)]
    out: PathBuf,
    /// PKCS#8 PEM private key, as `rdownloader plugin keygen --role repository` writes it;
    /// alternatively set RDOWNLOADER_REPOSITORY_SIGNING_KEY to the PEM text.
    #[arg(long)]
    key: Option<PathBuf>,
    /// The `key_id` the index names. The official repository's is the compiled-in root.
    #[arg(long, default_value = rd_sign::REPOSITORY_KEY_ID)]
    key_id: String,
    /// The index's sequence. Defaults to the signing time in Unix seconds, which only grows,
    /// so a later index always supersedes an earlier one without a counter to keep.
    #[arg(long)]
    sequence: Option<u64>,
    /// The `https://` directory the packages are published under, ending in `/`. Without it
    /// every entry is a file name relative to the index.
    #[arg(long)]
    base_url: Option<String>,
    /// Days until the index expires.
    #[arg(long, default_value_t = index::DEFAULT_VALIDITY_DAYS)]
    valid_days: i64,
    /// JSON file listing withdrawn `package_digests` and `keys`
    /// (`crates/rd-plugin-host/resources/plugin-index-revocations.json`).
    #[arg(long)]
    revocations: Option<PathBuf>,
    /// Directory of `<package file stem>.txt` release notes; a package without one has none.
    #[arg(long)]
    notes: Option<PathBuf>,
}

#[derive(Args)]
struct VerifyArgs {
    /// The signed index.
    file: PathBuf,
    /// Directory holding the listed packages; each is checked against its entry's digest and
    /// size.
    #[arg(long)]
    packages: Option<PathBuf>,
}

pub async fn run(args: IndexArgs) -> Result<()> {
    match args.command {
        IndexCommand::Build(args) => build(&args).await,
        IndexCommand::Verify(args) => verify(&args).await,
    }
}

async fn build(args: &BuildArgs) -> Result<()> {
    ensure!(
        (1..=index::MAX_VALIDITY_DAYS).contains(&args.valid_days),
        "--valid-days must be between 1 and {}",
        index::MAX_VALIDITY_DAYS
    );
    let pem = match &args.key {
        Some(path) => tokio::fs::read_to_string(path)
            .await
            .with_context(|| format!("read signing key {}", path.display()))?,
        None => std::env::var(KEY_ENV).with_context(|| format!("pass --key or set {KEY_ENV}"))?,
    };
    let key = rd_plugin_host::load_signing_key_pem(&pem)?;
    check_against_root(&args.key_id, &key)?;

    let files = package_files(&args.packages)?;
    if files.is_empty() {
        bail!("no .rdplug packages in {}", args.packages.display());
    }
    let mut packages = Vec::with_capacity(files.len());
    for path in &files {
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .with_context(|| format!("{} has no UTF-8 file name", path.display()))?;
        let bytes = tokio::fs::read(path)
            .await
            .with_context(|| format!("read {}", path.display()))?;
        let url = index::package_url(args.base_url.as_deref(), file_name)?;
        let notes = match &args.notes {
            Some(directory) => read_notes(directory, path).await?,
            None => None,
        };
        let entry = index::describe_package(&bytes, url, notes)
            .with_context(|| format!("describe {}", path.display()))?;
        packages.push(entry);
    }
    packages.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then_with(|| left.version.cmp(&right.version))
    });
    let revoked = match &args.revocations {
        Some(path) => {
            let bytes = tokio::fs::read(path)
                .await
                .with_context(|| format!("read {}", path.display()))?;
            serde_json::from_slice(&bytes)
                .with_context(|| format!("parse withdrawals {}", path.display()))?
        }
        None => Revocations::default(),
    };

    let issued_at = Utc::now().trunc_subsecs(0);
    let sequence = match args.sequence {
        Some(sequence) => sequence,
        None => u64::try_from(issued_at.timestamp()).context("the clock is before 1970")?,
    };
    let plugin_index = PluginIndex {
        schema_version: index::PLUGIN_INDEX_SCHEMA_VERSION,
        sequence,
        issued_at,
        not_after: issued_at + Duration::days(args.valid_days),
        packages,
        revoked,
    };
    let signed = index::sign(&args.key_id, &key, &plugin_index)?;
    // Round trip under the signing key itself, so the file written is known to be readable
    // whatever the build's embedded root says; `verify` is the check against the root.
    let trust = rd_sign::TrustStore::new();
    trust.trust(args.key_id.clone(), key.verifying_key())?;
    index::verify_with(&signed, &trust, None, issued_at).map_err(|error| {
        anyhow!(
            "the signed index does not verify ({}): {error}",
            error.code()
        )
    })?;
    if let Some(parent) = args.out.parent()
        && !parent.as_os_str().is_empty()
    {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(&args.out, &signed)
        .await
        .with_context(|| format!("write {}", args.out.display()))?;
    println!(
        "signed {} packages and {} withdrawals as sequence {} into {} (valid until {})",
        plugin_index.packages.len(),
        plugin_index.revoked.package_digests.len() + plugin_index.revoked.keys.len(),
        plugin_index.sequence,
        args.out.display(),
        plugin_index.not_after.to_rfc3339()
    );
    Ok(())
}

/// Refuses a key that is not the compiled-in root it claims to be.
///
/// Mixing the three signing keys up otherwise surfaces only when an installation refuses the
/// index; here it is caught before anything is published. A `key_id` this build has no root
/// for is a third-party repository's, and allowed with a warning.
fn check_against_root(key_id: &str, key: &rd_sign::SigningKey) -> Result<()> {
    let roots = rd_sign::keys_for_now(rd_sign::Role::Repository);
    match roots.iter().find(|root| root.key_id == key_id) {
        Some(root) => {
            let embedded = rd_sign::decode_public_key(root.public_key)?;
            ensure!(
                embedded == key.verifying_key(),
                "the key given is not the repository root {key_id} this build embeds \
                 (a plugin or site-rules key by mistake?)"
            );
        }
        None => eprintln!(
            "warning: this build embeds no repository root named {key_id}; the index will not \
             verify against it (paste the public key into crates/rd-sign/src/roots.rs)"
        ),
    }
    Ok(())
}

/// Every `.rdplug` directly inside `directory`, sorted by name.
fn package_files(directory: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(directory)
        .with_context(|| format!("read package directory {}", directory.display()))?
    {
        let path = entry?.path();
        if path.is_file()
            && path
                .extension()
                .is_some_and(|extension| extension == "rdplug")
        {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

/// `<directory>/<package file stem>.txt`, if there is one.
///
/// Keyed by the package's file name, which carries its version, so a note cannot silently
/// carry over onto the next version of the same plugin.
async fn read_notes(directory: &Path, package: &Path) -> Result<Option<String>> {
    let Some(stem) = package.file_stem().and_then(|stem| stem.to_str()) else {
        return Ok(None);
    };
    let path = directory.join(format!("{stem}.txt"));
    if !path.is_file() {
        return Ok(None);
    }
    let notes = tokio::fs::read_to_string(&path)
        .await
        .with_context(|| format!("read release notes {}", path.display()))?;
    Ok(Some(notes))
}

/// Refuses an index an installation would refuse, and optionally the packages beside it.
async fn verify(args: &VerifyArgs) -> Result<()> {
    let bytes = tokio::fs::read(&args.file)
        .await
        .with_context(|| format!("read index {}", args.file.display()))?;
    let verified = index::verify(&bytes, None, Utc::now()).map_err(|error| {
        anyhow!(
            "{} does not verify ({}): {error}",
            args.file.display(),
            error.code()
        )
    })?;
    println!(
        "{} verifies: {} packages, {} withdrawn digests, {} withdrawn keys, sequence {}, valid until {}",
        args.file.display(),
        verified.packages.len(),
        verified.revoked.package_digests.len(),
        verified.revoked.keys.len(),
        verified.sequence,
        verified.not_after.to_rfc3339()
    );
    let Some(directory) = &args.packages else {
        return Ok(());
    };
    for entry in &verified.packages {
        let file_name = entry.url.rsplit('/').next().unwrap_or(&entry.url);
        let path = directory.join(file_name);
        let package = tokio::fs::read(&path)
            .await
            .with_context(|| format!("read {}", path.display()))?;
        let described = index::describe_package(&package, entry.url.clone(), None)
            .with_context(|| format!("describe {}", path.display()))?;
        ensure!(
            described.package_digest == entry.package_digest && described.size == entry.size,
            "{} does not match its index entry",
            path.display()
        );
    }
    println!(
        "  every listed package in {} matches its digest and size",
        directory.display()
    );
    Ok(())
}
