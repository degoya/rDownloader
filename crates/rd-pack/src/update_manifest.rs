//! `update manifest …` (`rdownloader` and `rd-pack` alike): the publishing side of the update
//! manifest (RD-180-01).
//!
//! The format and its verification are `rd_update::manifest`; this builds one release's manifest
//! from the release's `SHA256SUMS` (the hashes), the published files (their sizes), the
//! version's `RELEASE-NOTES.md` section (the notes for users) and its `CHANGELOG.md` heading (the
//! anchor of the full changes, RD-1150-02), signs it with the update key and writes it as
//! the channel's asset name. The release workflow runs `build` after the checksums and `verify`
//! over what it wrote, before both go to the release.
//!
//! Which file is which artifact is read from its name, as the release names them:
//! `rdownloader-<linux|windows|macos>-<x86_64|aarch64>.<tar.gz|zip>` is the portable archive, and
//! a `.msi`, `.deb` or `.rpm` whose name starts with `rdownloader` and carries an architecture
//! (`x86_64`/`amd64`/`x64`, `aarch64`/`arm64`) is that installer — the MSI only in English, the
//! other languages' `…-de.msi` are not updates. The capture agent's own archives,
//! `rdownloader-capture-<platform>-<arch>.<tar.gz|zip>` (`scripts/release-assets.sh agent`), are
//! the manifest's `agent_artifacts` (RD-1210-03). The extensions, the plugins and everything else
//! are not updates and are left out.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail, ensure};
use chrono::{Duration, SubsecRound, Utc};
use clap::{Args, Subcommand};
use rd_update::{
    Artifact, Channel, UpdateManifest,
    manifest::{self, DEFAULT_VALIDITY_DAYS, MAX_VALIDITY_DAYS, kind},
};

/// The environment variable the release workflow hands the update key in.
const KEY_ENV: &str = "RDOWNLOADER_UPDATE_SIGNING_KEY";

#[derive(Subcommand)]
pub enum UpdateCommand {
    /// Builds, signs and verifies the signed update manifest of a release.
    #[command(subcommand)]
    Manifest(ManifestCommand),
}

// `Build` carries every flag of the release step and `Verify` two paths; the value is parsed once
// per command-line call, so boxing it would buy nothing.
#[allow(clippy::large_enum_variant)]
#[derive(Subcommand)]
pub enum ManifestCommand {
    /// Builds and signs the manifest of one release from its SHA256SUMS and files.
    Build(BuildArgs),
    /// Verifies a signed manifest against this build's update root.
    Verify(VerifyArgs),
}

#[derive(Args)]
pub struct BuildArgs {
    /// The release version or tag, e.g. `v1.8.0` or `1.8.0-beta.1`. A pre-release goes to the
    /// beta channel, anything else to stable.
    #[arg(long)]
    version: String,
    /// The release's `SHA256SUMS` (`<sha256>  ./<file>` per line).
    #[arg(long)]
    checksums: PathBuf,
    /// Directory holding the published files, for their sizes.
    #[arg(long)]
    assets: PathBuf,
    /// The `https://` directory the files are downloaded from, ending in `/`.
    #[arg(long)]
    base_url: String,
    /// `CHANGELOG.md`; the anchor of the version's `## [X.Y.Z] - YYYY-MM-DD` heading goes into
    /// the manifest, which the interface links as the full changes at the tag.
    #[arg(long)]
    changelog: Option<PathBuf>,
    /// `RELEASE-NOTES.md`; the version's section, its points for users, becomes the notes. A
    /// section the rules refuse, or one still marked as a draft, stops the build.
    #[arg(long)]
    release_notes: Option<PathBuf>,
    /// Directory the manifest is written to, as `rdownloader-update-<channel>.json`.
    #[arg(long)]
    out: PathBuf,
    /// PKCS#8 PEM private key, as `rdownloader plugin keygen --role release` writes it;
    /// alternatively set RDOWNLOADER_UPDATE_SIGNING_KEY to the PEM text.
    #[arg(long)]
    key: Option<PathBuf>,
    /// The `key_id` the manifest names: the compiled-in update root.
    #[arg(long, default_value = rd_sign::UPDATE_KEY_ID)]
    key_id: String,
    /// The manifest's sequence. Defaults to the signing time in Unix seconds, which only grows.
    #[arg(long)]
    sequence: Option<u64>,
    /// Days until the manifest expires.
    #[arg(long, default_value_t = DEFAULT_VALIDITY_DAYS)]
    valid_days: i64,
    /// Whether the release changes the database schema: `true` or `false`, as the release
    /// workflow works it out from `crates/rd-db/migrations/` (`scripts/update-schema-change.sh`).
    /// Left out, the manifest does not say, and every installation treats it as a change.
    #[arg(long, value_name = "true|false")]
    schema_change: Option<bool>,
}

#[derive(Args)]
pub struct VerifyArgs {
    /// The signed manifest. Its channel is read from its file name unless `--channel` names it.
    file: PathBuf,
    /// `stable` or `beta`.
    #[arg(long)]
    channel: Option<String>,
    /// Directory holding the listed files; each one present is checked against its size and
    /// SHA-256.
    #[arg(long)]
    assets: Option<PathBuf>,
}

pub async fn run(command: UpdateCommand) -> Result<()> {
    match command {
        UpdateCommand::Manifest(ManifestCommand::Build(args)) => build(&args).await,
        UpdateCommand::Manifest(ManifestCommand::Verify(args)) => verify(&args).await,
    }
}

async fn build(args: &BuildArgs) -> Result<()> {
    ensure!(
        (1..=MAX_VALIDITY_DAYS).contains(&args.valid_days),
        "--valid-days must be between 1 and {MAX_VALIDITY_DAYS}"
    );
    let version = rd_update::parse_version(&args.version)
        .with_context(|| format!("{} is not a SemVer version", args.version))?;
    let channel = if version.pre.is_empty() {
        Channel::Stable
    } else {
        Channel::Beta
    };
    ensure!(
        args.base_url.starts_with("https://") && args.base_url.ends_with('/'),
        "--base-url must be an https:// directory ending in /"
    );
    let pem = match &args.key {
        Some(path) => tokio::fs::read_to_string(path)
            .await
            .with_context(|| format!("read signing key {}", path.display()))?,
        None => std::env::var(KEY_ENV).with_context(|| format!("pass --key or set {KEY_ENV}"))?,
    };
    let key = rd_plugin_host::load_signing_key_pem(&pem)?;
    check_against_root(&args.key_id, &key)?;

    let sums = tokio::fs::read_to_string(&args.checksums)
        .await
        .with_context(|| format!("read {}", args.checksums.display()))?;
    let (mut artifacts, mut agent_artifacts) = (Vec::new(), Vec::new());
    for (sha256, name) in parse_checksums(&sums) {
        let (list, (platform, arch, kind)) = match (classify(&name), classify_agent(&name)) {
            (Some(entry), _) => (&mut artifacts, entry),
            (None, Some(entry)) => (&mut agent_artifacts, entry),
            (None, None) => continue,
        };
        let path = args.assets.join(&name);
        let size = tokio::fs::metadata(&path)
            .await
            .with_context(|| {
                format!(
                    "{} is in SHA256SUMS but not in {}",
                    name,
                    args.assets.display()
                )
            })?
            .len();
        list.push(Artifact {
            platform: platform.to_owned(),
            arch: arch.to_owned(),
            kind: kind.to_owned(),
            url: format!("{}{name}", args.base_url),
            sha256,
            size,
        });
    }
    ensure!(
        !artifacts.is_empty(),
        "{} names no application archive or installer",
        args.checksums.display()
    );
    for list in [&mut artifacts, &mut agent_artifacts] {
        list.sort_by(|left, right| {
            (&left.platform, &left.arch, &left.kind).cmp(&(
                &right.platform,
                &right.arch,
                &right.kind,
            ))
        });
    }
    let notes = match &args.release_notes {
        Some(path) => notes::user_notes(&read_text(path).await?, &version)?.unwrap_or_else(|| {
            eprintln!(
                "warning: {} has no section for {version}; the manifest carries no notes",
                path.display()
            );
            String::new()
        }),
        None => String::new(),
    };
    let changelog_anchor = match &args.changelog {
        Some(path) => {
            let anchor = notes::changelog_anchor(&read_text(path).await?, &version);
            if anchor.is_none() {
                eprintln!(
                    "warning: {} has no heading for {version}; the full changes link the file",
                    path.display()
                );
            }
            anchor
        }
        None => None,
    };

    let issued_at = Utc::now().trunc_subsecs(0);
    let sequence = match args.sequence {
        Some(sequence) => sequence,
        None => u64::try_from(issued_at.timestamp()).context("the clock is before 1970")?,
    };
    let update = UpdateManifest {
        schema_version: manifest::UPDATE_MANIFEST_SCHEMA_VERSION,
        sequence,
        issued_at,
        not_after: issued_at + Duration::days(args.valid_days),
        channel,
        version: version.to_string(),
        released_at: issued_at,
        notes,
        changelog_anchor,
        artifacts,
        schema_change: args.schema_change,
        agent_artifacts,
    };
    let signed = manifest::sign(&args.key_id, &key, &update)?;
    // Round trip under the signing key itself; `verify` is the check against the root.
    let trust = rd_sign::TrustStore::new();
    trust.trust(args.key_id.clone(), key.verifying_key())?;
    manifest::verify_with(&signed, &trust, channel, None, issued_at).map_err(|error| {
        anyhow!(
            "the signed manifest does not verify ({}): {error}",
            error.code()
        )
    })?;
    tokio::fs::create_dir_all(&args.out).await?;
    let out = args.out.join(channel.file_name());
    tokio::fs::write(&out, &signed)
        .await
        .with_context(|| format!("write {}", out.display()))?;
    println!(
        "signed {} {} with {} artifacts and {} agent archives as sequence {} into {} (valid until {}, schema change: {})",
        channel.as_str(),
        update.version,
        update.artifacts.len(),
        update.agent_artifacts.len(),
        update.sequence,
        out.display(),
        update.not_after.to_rfc3339(),
        update
            .schema_change
            .map_or("not said", |change| if change { "yes" } else { "no" })
    );
    Ok(())
}

/// Refuses a key that is not the compiled-in update root, so a plugin or repository key given
/// by mistake is caught here and not by every installation's check.
fn check_against_root(key_id: &str, key: &rd_sign::SigningKey) -> Result<()> {
    let roots = rd_sign::keys_for_now(rd_sign::Role::Release);
    match roots.iter().find(|root| root.key_id == key_id) {
        Some(root) => {
            let embedded = rd_sign::decode_public_key(root.public_key)?;
            ensure!(
                embedded == key.verifying_key(),
                "the key given is not the update root {key_id} this build embeds \
                 (a plugin or repository key by mistake?)"
            );
        }
        None => eprintln!(
            "warning: this build embeds no update root named {key_id}; the manifest will not \
             verify against it (paste the public key into crates/rd-sign/src/roots.rs)"
        ),
    }
    Ok(())
}

/// `(sha256, file name)` per line of a `SHA256SUMS`, in either `sha256sum` spelling.
fn parse_checksums(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let (hash, name) = line.trim().split_once(char::is_whitespace)?;
            let name = name.trim().trim_start_matches('*').trim_start_matches("./");
            let hash = hash.to_ascii_lowercase();
            (hash.len() == 64
                && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
                && !name.is_empty())
            .then(|| (hash, name.to_owned()))
        })
        .collect()
}

/// `(platform, arch, kind)` of a release file that is an application update, `None` otherwise.
fn classify(name: &str) -> Option<(&'static str, &'static str, &'static str)> {
    let lower = name.to_ascii_lowercase();
    if !lower.starts_with("rdownloader") || lower.contains("capture") {
        return None;
    }
    let arch = || {
        let tokens: Vec<&str> = lower
            .split(|character: char| !character.is_ascii_alphanumeric())
            .collect();
        if lower.contains("x86_64") || tokens.iter().any(|token| matches!(*token, "amd64" | "x64"))
        {
            Some("x86_64")
        } else if tokens
            .iter()
            .any(|token| matches!(*token, "aarch64" | "arm64"))
        {
            Some("aarch64")
        } else {
            None
        }
    };
    if let Some(archive) = portable_archive(&lower) {
        return Some(archive);
    }
    let (platform, artifact_kind) = if lower.ends_with(".msi") {
        // The installers of the other languages, `…-de.msi` (RD-1120-20): an update installs
        // the English one, whose dialogs it never shows (`msiexec /qn`).
        if is_language_variant(&lower) {
            return None;
        }
        ("windows", kind::MSI)
    } else if lower.ends_with(".deb") {
        ("linux", kind::DEB)
    } else if lower.ends_with(".rpm") {
        ("linux", kind::RPM)
    } else {
        return None;
    };
    Some((platform, arch()?, artifact_kind))
}

/// `…-<language>.msi`: a two-letter language after the last hyphen.
fn is_language_variant(lower: &str) -> bool {
    lower
        .strip_suffix(".msi")
        .and_then(|stem| stem.rsplit_once('-'))
        .is_some_and(|(_, language)| {
            language.len() == 2 && language.bytes().all(|byte| byte.is_ascii_lowercase())
        })
}

/// `(platform, arch, kind)` of the capture agent's own archive (RD-1210-03),
/// `rdownloader-capture-<platform>-<arch>.<tar.gz|zip>`; `None` for every other file.
fn classify_agent(name: &str) -> Option<(&'static str, &'static str, &'static str)> {
    let lower = name.to_ascii_lowercase();
    archive_of(lower.strip_prefix("rdownloader-capture-")?)
}

/// `rdownloader-<platform>-<arch>.<tar.gz|zip>`, the portable archive.
fn portable_archive(lower: &str) -> Option<(&'static str, &'static str, &'static str)> {
    archive_of(lower.strip_prefix("rdownloader-")?)
}

/// `<platform>-<arch>.<tar.gz|zip>`, the rest of an archive's name.
fn archive_of(stem: &str) -> Option<(&'static str, &'static str, &'static str)> {
    let stem = stem
        .strip_suffix(".tar.gz")
        .or_else(|| stem.strip_suffix(".zip"))?;
    let (platform, arch) = stem.split_once('-')?;
    let platform = match platform {
        "linux" => "linux",
        "windows" => "windows",
        "macos" => "macos",
        _ => return None,
    };
    let arch = match arch {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        _ => return None,
    };
    Some((platform, arch, kind::ARCHIVE))
}

async fn read_text(path: &Path) -> Result<String> {
    tokio::fs::read_to_string(path)
        .await
        .with_context(|| format!("read {}", path.display()))
}

async fn verify(args: &VerifyArgs) -> Result<()> {
    let channel = match &args.channel {
        Some(name) => Channel::parse(name).with_context(|| format!("unknown channel {name}"))?,
        None => {
            let name = args
                .file
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            [Channel::Stable, Channel::Beta]
                .into_iter()
                .find(|channel| channel.file_name() == name)
                .with_context(|| format!("{name} names no channel; pass --channel"))?
        }
    };
    let bytes = tokio::fs::read(&args.file)
        .await
        .with_context(|| format!("read manifest {}", args.file.display()))?;
    let verified = manifest::verify(&bytes, channel, None, Utc::now()).map_err(|error| {
        anyhow!(
            "{} does not verify ({}): {error}",
            args.file.display(),
            error.code()
        )
    })?;
    println!(
        "{} verifies: {} {} with {} artifacts and {} agent archives, sequence {}, valid until {}",
        args.file.display(),
        channel.as_str(),
        verified.version,
        verified.artifacts.len(),
        verified.agent_artifacts.len(),
        verified.sequence,
        verified.not_after.to_rfc3339()
    );
    if let Some(directory) = &args.assets {
        check_assets(&verified, directory)?;
    }
    Ok(())
}

/// Every listed artifact present in `directory` has its size and SHA-256.
fn check_assets(verified: &UpdateManifest, directory: &Path) -> Result<()> {
    use sha2::{Digest, Sha256};
    for artifact in verified.artifacts.iter().chain(&verified.agent_artifacts) {
        let name = artifact.url.rsplit('/').next().unwrap_or(&artifact.url);
        let path = directory.join(name);
        let bytes = std::fs::read(&path).with_context(|| format!("read {}", path.display()))?;
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) != artifact.size
            || hex::encode(Sha256::digest(&bytes)) != artifact.sha256
        {
            bail!("{} does not match its manifest entry", path.display());
        }
    }
    println!(
        "  every listed artifact in {} matches its size and SHA-256",
        directory.display()
    );
    Ok(())
}

#[path = "update_manifest_notes.rs"]
mod notes;

#[cfg(test)]
#[path = "update_manifest_tests.rs"]
mod tests;
