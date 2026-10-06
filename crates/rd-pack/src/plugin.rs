//! `plugin …` publishing subcommands: package, verify, conformance, new, index.
//!
//! `rdownloader plugin` flattens these into its own list, beside the commands that need an
//! installation (`keygen`, `install`, `keys`); `rd-pack plugin` offers exactly these.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use rd_plugin_host::VerifyError;

#[path = "plugin_scaffold.rs"]
mod scaffold;

use scaffold::scaffold;
#[cfg(test)]
use scaffold::{sdk_templates, write_gitignore};

#[derive(Subcommand)]
pub enum PluginCommand {
    /// Builds a signed `.rdplug` from a manifest and a compiled component.
    Package(PackageArgs),
    /// Validates archive structure, component imports and signature.
    Verify(PluginPackageArgs),
    /// Checks a package against everything this core requires of it.
    Conformance(ConformanceArgs),
    /// Scaffolds a new plugin from the SDK templates.
    New(NewPluginArgs),
    /// Builds and verifies the signed plugin repository index (RD-140-01).
    Index(crate::plugin_index::IndexArgs),
}

#[derive(Args)]
pub struct ConformanceArgs {
    #[command(flatten)]
    package: PluginPackageArgs,
    /// Machine-readable report, for a CI job to act on.
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
pub struct NewPluginArgs {
    /// What to scaffold.
    #[arg(long = "type", value_name = "TYPE")]
    plugin_type: NewPluginType,
    /// Directory to create. Must not exist.
    #[arg(long)]
    out: PathBuf,
    /// Plugin and crate name; defaults to the output directory's name.
    #[arg(long)]
    name: Option<String>,
}

/// The plugin worlds `plugin new` can scaffold.
///
/// One variant per world and nothing else: the variant's own value name is the template
/// directory *and* the manifest's `plugin_type` (`intake-mirrors` excepted, see
/// `manifest_type`), so adding a world costs a line here and no branch
/// anywhere. `scripts/check-sdk-templates.sh` holds the other end: every world in the WIT has
/// a template. It used to be this enum plus a matching `match`, which is two places to
/// change and one of them easy to forget — `oauth` shipped as a world in RD-103-00 and was
/// missing from both, so the command refused a type the core already spoke.
#[derive(Clone, Copy, clap::ValueEnum)]
enum NewPluginType {
    Resolver,
    Transfer,
    Intake,
    Auth,
    /// Runs an OAuth redirect flow and renews its token (RD-105-01).
    Oauth,
    /// Turns one address into the files behind it (RD-104-03).
    Crawler,
    Enricher,
    Notifier,
    Postprocess,
    Storage,
    /// Runs a job that lives at the provider and outlives the call (RD-107-06).
    RemoteJob,
    /// Answers with an address and how its bytes become a file (RD-110-33).
    StreamTransform,
    /// An intake parser that also states every source of a file (RD-150-03).
    IntakeMirrors,
}

impl NewPluginType {
    /// The template directory, which is also the manifest's `plugin_type` for every world
    /// but `intake-mirrors`.
    ///
    /// Read off clap's own value name, which is where the string the user typed came from —
    /// so the directory and the accepted `--type` value cannot drift apart. It allocates, so
    /// it is `name` rather than `as_str`: there is no `&'static str` to hand back once the
    /// name is clap's rather than this file's.
    fn name(self) -> String {
        clap::ValueEnum::to_possible_value(&self)
            .expect("every NewPluginType variant has a value name; none is skipped")
            .get_name()
            .to_owned()
    }

    /// The manifest's `plugin_type` the template declares.
    ///
    /// The directory name for every world but one: `intake-mirrors-plugin` is a second world
    /// of the `intake` type, recognised by what the component exports rather than by a type of
    /// its own (RD-150-03), so its template declares `intake`. Only the test below asks.
    #[cfg(test)]
    fn manifest_type(self) -> String {
        match self {
            Self::IntakeMirrors => Self::Intake.name(),
            other => other.name(),
        }
    }
}

#[derive(Args)]
pub struct PackageArgs {
    #[arg(long)]
    manifest: PathBuf,
    #[arg(long)]
    component: PathBuf,
    /// Directory of `<lang>.json` translations to ship (and sign) inside the package.
    #[arg(long)]
    locales: Option<PathBuf>,
    /// PEM private key; alternatively set RDOWNLOADER_PLUGIN_SIGNING_KEY to the PEM text.
    #[arg(long)]
    key: Option<PathBuf>,
    #[arg(long)]
    output: PathBuf,
    /// Writes an unsigned archive that only development mode accepts.
    #[arg(long)]
    development: bool,
}

#[derive(Args)]
pub struct PluginPackageArgs {
    pub package: PathBuf,
    /// Trusted key as KEY_ID=BASE64_ED25519_PUBLIC_KEY; repeatable.
    #[arg(long = "trusted-key")]
    pub trusted_keys: Vec<String>,
    #[arg(long)]
    pub development_mode: bool,
    /// Ignores the release key compiled into this binary.
    #[arg(long)]
    pub no_default_plugin_key: bool,
}

pub async fn run(command: PluginCommand) -> Result<()> {
    match command {
        PluginCommand::Package(args) => package(&args).await,
        PluginCommand::Verify(args) => {
            let verifier = build_plugin_verifier(
                args.development_mode,
                &args.trusted_keys,
                !args.no_default_plugin_key,
            )?;
            match verifier.verify_file(&args.package) {
                Ok(package) => {
                    println!(
                        "verified {} {} ({})",
                        package.manifest.name, package.manifest.version, package.manifest.id
                    );
                    println!(
                        "  by {} — {}",
                        package.manifest.metadata.author, package.manifest.metadata.description
                    );
                    println!("  slug:     {}", package.manifest.message_slug());
                    Ok(())
                }
                Err(error) => Err(describe_verify_error(error)),
            }
        }
        PluginCommand::Conformance(args) => conformance(&args).await,
        PluginCommand::New(args) => scaffold(&args),
        PluginCommand::Index(args) => crate::plugin_index::run(args).await,
    }
}

/// Turns an untrusted key into actionable advice instead of a bare failure.
pub fn describe_verify_error(error: VerifyError) -> anyhow::Error {
    match error {
        VerifyError::UntrustedKey {
            key_id,
            fingerprint,
            name,
            version,
            ..
        } => anyhow::anyhow!(
            "{name} {version} is signed by the untrusted key `{key_id}`\n  \
             fingerprint: {}\n  \
             Confirm it with `rdownloader plugin keys trust --from-package <pkg>` only if it \
             matches the fingerprint published by the plugin's author.",
            group_fingerprint(&fingerprint)
        ),
        VerifyError::Other(error) => error,
    }
}

/// Groups a hex fingerprint into 8-character blocks so it can be compared by eye.
pub fn group_fingerprint(fingerprint: &str) -> String {
    fingerprint
        .as_bytes()
        .chunks(8)
        .map(|chunk| String::from_utf8_lossy(chunk).into_owned())
        .collect::<Vec<_>>()
        .join(" ")
}

async fn package(args: &PackageArgs) -> Result<()> {
    let manifest = tokio::fs::read(&args.manifest)
        .await
        .with_context(|| format!("read manifest {}", args.manifest.display()))?;
    let component = tokio::fs::read(&args.component)
        .await
        .with_context(|| format!("read component {}", args.component.display()))?;
    let key = if args.development {
        None
    } else {
        let pem = match &args.key {
            Some(path) => tokio::fs::read_to_string(path)
                .await
                .with_context(|| format!("read signing key {}", path.display()))?,
            None => std::env::var("RDOWNLOADER_PLUGIN_SIGNING_KEY").context(
                "pass --key or set RDOWNLOADER_PLUGIN_SIGNING_KEY (or use --development)",
            )?,
        };
        Some(rd_plugin_host::load_signing_key_pem(&pem)?)
    };
    let locales = match &args.locales {
        Some(directory) => read_locale_directory(directory).await?,
        None => Vec::new(),
    };
    let archive = rd_plugin_host::package_plugin(&manifest, &component, &locales, key.as_ref())?;
    if let Some(parent) = args.output.parent()
        && !parent.as_os_str().is_empty()
    {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(&args.output, &archive).await?;
    println!(
        "packaged {} ({} bytes{})",
        args.output.display(),
        archive.len(),
        if key.is_some() {
            ", signed"
        } else {
            ", unsigned development build"
        }
    );
    Ok(())
}

/// Reads `<directory>/<lang>.json` translations to ship inside the package.
async fn read_locale_directory(directory: &PathBuf) -> Result<Vec<(String, Vec<u8>)>> {
    let mut entries = tokio::fs::read_dir(directory)
        .await
        .with_context(|| format!("read locales directory {}", directory.display()))?;
    let mut locales = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        if !entry.file_type().await?.is_file() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Some(language) = name
            .strip_suffix(".json")
            .filter(|tag| rd_plugin_host::valid_language(tag))
        else {
            bail!("{name} is not a `<lang>.json` locale file");
        };
        locales.push((language.to_owned(), tokio::fs::read(entry.path()).await?));
    }
    locales.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(locales)
}

/// Verifier with the embedded release key (when configured) plus explicit `KEY_ID=BASE64` entries.
pub fn build_plugin_verifier(
    development_mode: bool,
    trusted_keys: &[String],
    include_default_key: bool,
) -> Result<rd_plugin_host::PluginVerifier> {
    let verifier = rd_plugin_host::PluginVerifier::new(development_mode);
    if include_default_key {
        for root in rd_sign::keys_for_now(rd_sign::Role::Plugin) {
            verifier.trust_key_base64(root.key_id.to_owned(), root.public_key)?;
        }
    }
    for value in trusted_keys {
        let (key_id, encoded) = value
            .split_once('=')
            .context("trusted key must use KEY_ID=BASE64 format")?;
        verifier.trust_key_base64(key_id.to_owned(), encoded)?;
    }
    Ok(verifier)
}

/// Runs the conformance checks and reports every one of them.
///
/// Exits non-zero when anything failed, so a CI job needs no output parsing to gate on it.
async fn conformance(args: &ConformanceArgs) -> Result<()> {
    let verifier = build_plugin_verifier(
        args.package.development_mode,
        &args.package.trusted_keys,
        !args.package.no_default_plugin_key,
    )?;
    let bytes = tokio::fs::read(&args.package.package)
        .await
        .with_context(|| format!("read plugin package {}", args.package.package.display()))?;
    let report = rd_plugin_host::check_package(&bytes, &verifier).await;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        if let (Some(name), Some(version)) = (&report.name, &report.version) {
            println!(
                "{name} {version} ({})",
                report.plugin_type.as_deref().unwrap_or("unknown type")
            );
        }
        for check in &report.checks {
            let mark = if check.passed { "ok  " } else { "FAIL" };
            println!("  [{mark}] {} — {}", check.id, check.about);
            if let Some(detail) = &check.detail {
                println!("         {detail}");
            }
        }
    }
    if report.passed {
        Ok(())
    } else {
        anyhow::bail!("the package does not pass conformance")
    }
}

#[cfg(test)]
#[path = "plugin_tests.rs"]
mod tests;
