//! `update manifest …` (`rdownloader` and `rd-pack` alike): the publishing side of the update
//! manifest (RD-180-01).
//!
//! The format and its verification are `rd_update::manifest`; this builds one release's manifest
//! from the release's `SHA256SUMS` (the hashes), the published files (their sizes) and the
//! version's `CHANGELOG.md` section (the notes), signs it with the update key and writes it as
//! the channel's asset name. The release workflow runs `build` after the checksums and `verify`
//! over what it wrote, before both go to the release.
//!
//! Which file is which artifact is read from its name, as the release names them:
//! `rdownloader-<linux|windows|macos>-<x86_64|aarch64>.<tar.gz|zip>` is the portable archive, and
//! a `.msi`, `.deb` or `.rpm` whose name starts with `rdownloader` and carries an architecture
//! (`x86_64`/`amd64`/`x64`, `aarch64`/`arm64`) is that installer. The capture agent's files,
//! the extensions, the plugins and everything else are not application updates and are left out.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail, ensure};
use chrono::{Duration, SubsecRound, Utc};
use clap::{Args, Subcommand};
use rd_update::{
    Artifact, Channel, UpdateManifest,
    manifest::{self, DEFAULT_VALIDITY_DAYS, MAX_NOTES_CHARS, MAX_VALIDITY_DAYS, kind},
};

/// The environment variable the release workflow hands the update key in.
const KEY_ENV: &str = "RDOWNLOADER_UPDATE_SIGNING_KEY";

#[derive(Subcommand)]
pub enum UpdateCommand {
    /// Builds, signs and verifies the signed update manifest of a release.
    #[command(subcommand)]
    Manifest(ManifestCommand),
}

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
    /// `CHANGELOG.md`; the version's section, shortened to its entries' headlines, becomes the
    /// notes.
    #[arg(long)]
    changelog: Option<PathBuf>,
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
    let mut artifacts = Vec::new();
    for (sha256, name) in parse_checksums(&sums) {
        let Some((platform, arch, kind)) = classify(&name) else {
            continue;
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
        artifacts.push(Artifact {
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
    artifacts.sort_by(|left, right| {
        (&left.platform, &left.arch, &left.kind).cmp(&(&right.platform, &right.arch, &right.kind))
    });
    let notes = match &args.changelog {
        Some(path) => {
            let text = tokio::fs::read_to_string(path)
                .await
                .with_context(|| format!("read {}", path.display()))?;
            let notes = release_notes(&text, &version);
            if notes.is_empty() {
                eprintln!(
                    "warning: {} has no section for {version}; the manifest carries no notes",
                    path.display()
                );
            }
            notes
        }
        None => String::new(),
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
        artifacts,
        schema_change: args.schema_change,
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
        "signed {} {} with {} artifacts as sequence {} into {} (valid until {}, schema change: {})",
        channel.as_str(),
        update.version,
        update.artifacts.len(),
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

/// `rdownloader-<platform>-<arch>.<tar.gz|zip>`, the portable archive.
fn portable_archive(lower: &str) -> Option<(&'static str, &'static str, &'static str)> {
    let stem = lower.strip_prefix("rdownloader-")?;
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

/// The version's `CHANGELOG.md` section, shortened to its headings and each entry's headline.
///
/// The section of a pre-release falls back to its release's (`1.8.0-beta.1` → `1.8.0`), which
/// is where the entries of a version in the making are written before it is out. The full text
/// stays on the release page; the manifest carries what fits a notice.
fn release_notes(changelog: &str, version: &semver::Version) -> String {
    let base = format!("{}.{}.{}", version.major, version.minor, version.patch);
    let section = section(changelog, &version.to_string()).or_else(|| section(changelog, &base));
    let Some(section) = section else {
        return String::new();
    };
    let mut lines: Vec<String> = Vec::new();
    for line in section.lines() {
        if let Some(heading) = line.strip_prefix("### ") {
            lines.push(heading.trim().to_owned());
        } else if let Some(entry) = line.strip_prefix("- ") {
            let headline = entry
                .strip_prefix("**")
                .and_then(|bold| bold.split_once("**"))
                .map_or(entry, |(headline, _)| headline);
            lines.push(format!("- {}", headline.trim()));
        }
    }
    let mut notes = lines.join("\n");
    if notes.chars().count() > MAX_NOTES_CHARS {
        notes = notes.chars().take(MAX_NOTES_CHARS - 1).collect();
        notes.push('…');
    }
    notes
}

/// The lines under `## [version]` up to the next `## `.
fn section<'a>(changelog: &'a str, version: &str) -> Option<&'a str> {
    let marker = format!("## [{version}]");
    let start = changelog
        .match_indices(&marker)
        .map(|(index, _)| index)
        .find(|index| *index == 0 || changelog[..*index].ends_with('\n'))?;
    let body = &changelog[start + marker.len()..];
    let body = body.split_once('\n').map_or("", |(_, rest)| rest);
    let end = body.find("\n## ").map_or(body.len(), |index| index + 1);
    Some(&body[..end])
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
        "{} verifies: {} {} with {} artifacts, sequence {}, valid until {}",
        args.file.display(),
        channel.as_str(),
        verified.version,
        verified.artifacts.len(),
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
    for artifact in &verified.artifacts {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(clap::Parser)]
    struct Cli {
        #[command(subcommand)]
        command: UpdateCommand,
    }

    /// `--schema-change` reaches the signed manifest; left out, the manifest does not say, which
    /// every installation reads as a change.
    #[tokio::test]
    async fn the_schema_change_flag_reaches_the_signed_manifest() {
        use clap::Parser as _;
        use sha2::Digest as _;
        let directory =
            std::env::temp_dir().join(format!("rd-pack-schema-change-{}", uuid::Uuid::now_v7()));
        let assets = directory.join("assets");
        std::fs::create_dir_all(&assets).expect("assets");
        let archive = b"the archive";
        std::fs::write(assets.join("rdownloader-linux-x86_64.tar.gz"), archive).expect("archive");
        let sums = directory.join("SHA256SUMS");
        std::fs::write(
            &sums,
            format!(
                "{}  ./rdownloader-linux-x86_64.tar.gz\n",
                hex::encode(sha2::Sha256::digest(archive))
            ),
        )
        .expect("sums");
        let key = rd_plugin_host::generate_signing_key();
        let key_file = directory.join("update.key");
        std::fs::write(&key_file, &key.private_pem).expect("key");
        let trust = rd_sign::TrustStore::new();
        trust
            .trust(
                "test-update-key".to_owned(),
                key.signing_key.verifying_key(),
            )
            .expect("trust");
        let path = |path: &Path| path.display().to_string();
        for (flag, expected) in [
            (Some("false"), Some(false)),
            (Some("true"), Some(true)),
            (None, None),
        ] {
            let out = directory.join(format!("out-{}", flag.unwrap_or("absent")));
            let mut argv = vec![
                "rd-pack".to_owned(),
                "manifest".to_owned(),
                "build".to_owned(),
                "--version".to_owned(),
                "v1.8.0".to_owned(),
                "--checksums".to_owned(),
                path(&sums),
                "--assets".to_owned(),
                path(&assets),
                "--base-url".to_owned(),
                "https://example.test/releases/v1.8.0/".to_owned(),
                "--out".to_owned(),
                path(&out),
                "--key".to_owned(),
                path(&key_file),
                "--key-id".to_owned(),
                "test-update-key".to_owned(),
            ];
            if let Some(flag) = flag {
                argv.extend(["--schema-change".to_owned(), flag.to_owned()]);
            }
            run(Cli::try_parse_from(argv).expect("arguments").command)
                .await
                .expect("build");
            let bytes = std::fs::read(out.join(Channel::Stable.file_name())).expect("manifest");
            let verified = manifest::verify_with(&bytes, &trust, Channel::Stable, None, Utc::now())
                .expect("verify");
            assert_eq!(verified.schema_change, expected, "{flag:?}");
            assert_eq!(verified.changes_schema(), expected.unwrap_or(true));
        }
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn the_release_files_are_classified_by_name() {
        assert_eq!(
            classify("rdownloader-linux-x86_64.tar.gz"),
            Some(("linux", "x86_64", "archive"))
        );
        assert_eq!(
            classify("rdownloader-macos-aarch64.tar.gz"),
            Some(("macos", "aarch64", "archive"))
        );
        assert_eq!(
            classify("rdownloader-windows-x86_64.zip"),
            Some(("windows", "x86_64", "archive"))
        );
        assert_eq!(
            classify("rdownloader-1.8.0-x86_64.msi"),
            Some(("windows", "x86_64", "msi"))
        );
        assert_eq!(
            classify("rdownloader_1.8.0_amd64.deb"),
            Some(("linux", "x86_64", "deb"))
        );
        assert_eq!(
            classify("rdownloader_1.8.0_arm64.deb"),
            Some(("linux", "aarch64", "deb"))
        );
        assert_eq!(
            classify("rdownloader-1.8.0-1.aarch64.rpm"),
            Some(("linux", "aarch64", "rpm"))
        );
        // The names the installers are published under (RD-180-05).
        for (name, expected) in [
            (
                "rdownloader-windows-x86_64.msi",
                ("windows", "x86_64", "msi"),
            ),
            ("rdownloader-linux-x86_64.deb", ("linux", "x86_64", "deb")),
            ("rdownloader-linux-aarch64.rpm", ("linux", "aarch64", "rpm")),
        ] {
            assert_eq!(classify(name), Some(expected), "{name}");
        }
        for other in [
            "rdownloader-chrome.zip",
            "rdownloader-firefox.zip",
            "rdownloader-capture-1.8.0-x86_64.msi",
            "rdownloader-plugin-index.json",
            "SHA256SUMS",
            "rdownloader.spdx.json",
            "ddownload-1.0.0.rdplug",
            "rdownloader-1.8.0.msi",
        ] {
            assert_eq!(classify(other), None, "{other}");
        }
    }

    #[test]
    fn both_checksum_spellings_are_read() {
        let hash = "a".repeat(64);
        let text = format!(
            "{hash}  ./rdownloader-linux-x86_64.tar.gz\n{hash} *rdownloader-windows-x86_64.zip\nnot a line\n"
        );
        let parsed = parse_checksums(&text);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].1, "rdownloader-linux-x86_64.tar.gz");
        assert_eq!(parsed[1].1, "rdownloader-windows-x86_64.zip");
    }

    const CHANGELOG: &str = "# Changelog\n\n## [Unreleased]\n\n### Added\n\n- **Not yet.** Text.\n\n\
## [1.8.0] - 2026-10-10\n\n### Added\n\n- **Update check (RD-180-01).** A long explanation\n  over two lines.\n\
- Plain entry\n\n### Fixed\n\n- **A crash.** Details.\n\n## [1.7.0] - 2026-09-30\n\n### Added\n\n- **Old.** Old.\n";

    #[test]
    fn the_notes_are_the_sections_headlines() {
        let version = semver::Version::parse("1.8.0").expect("version");
        assert_eq!(
            release_notes(CHANGELOG, &version),
            "Added\n- Update check (RD-180-01).\n- Plain entry\nFixed\n- A crash."
        );
    }

    #[test]
    fn a_beta_falls_back_to_its_releases_section_and_a_missing_one_to_nothing() {
        let beta = semver::Version::parse("1.8.0-beta.1").expect("version");
        assert!(release_notes(CHANGELOG, &beta).starts_with("Added\n- Update check"));
        let missing = semver::Version::parse("2.0.0").expect("version");
        assert_eq!(release_notes(CHANGELOG, &missing), "");
    }
}
