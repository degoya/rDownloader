//! `rdownloader plugin …` subcommands: keygen, package, verify, install, keys.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use rd_plugin_host::VerifyError;

use crate::trusted_keys::RELEASE_KEY_ID;

#[derive(Args)]
pub struct PluginArgs {
    #[command(subcommand)]
    command: PluginCommand,
}

#[derive(Subcommand)]
enum PluginCommand {
    /// Generates an Ed25519 signing key pair for plugin releases.
    Keygen(KeygenArgs),
    /// Builds a signed `.rdplug` from a manifest and a compiled component.
    Package(PackageArgs),
    /// Validates archive structure, component imports and signature.
    Verify(PluginPackageArgs),
    /// Installs a validated, version-pinned package atomically.
    Install(PluginInstallArgs),
    /// Inspects and edits the trust-on-first-use signing keys.
    Keys(KeysArgs),
    /// Checks a package against everything this core requires of it.
    Conformance(ConformanceArgs),
    /// Scaffolds a new plugin from the SDK templates.
    New(NewPluginArgs),
}

#[derive(Args)]
struct ConformanceArgs {
    #[command(flatten)]
    package: PluginPackageArgs,
    /// Machine-readable report, for a CI job to act on.
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
struct NewPluginArgs {
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
/// directory *and* the manifest's `plugin_type`, so adding a world costs a line here and no
/// branch anywhere. It used to be this enum plus a matching `match`, which is two places to
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
}

impl NewPluginType {
    /// The template directory, which is also the manifest's `plugin_type`.
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
}

#[derive(Args)]
pub struct KeysArgs {
    #[command(subcommand)]
    command: KeysCommand,
    /// Database holding the confirmed keys.
    #[arg(long, default_value = "data/rdownloader.sqlite3", global = true)]
    database: PathBuf,
}

#[derive(Subcommand)]
enum KeysCommand {
    /// Lists the signing keys confirmed on this installation.
    List,
    /// Confirms the key a package is signed with, after showing its fingerprint.
    Trust(TrustKeyArgs),
    /// Revokes a key; plugins signed with it are skipped from the next start.
    Revoke(RevokeKeyArgs),
}

#[derive(Args)]
struct TrustKeyArgs {
    /// Package whose signing key should be trusted.
    #[arg(long = "from-package")]
    from_package: PathBuf,
    /// Confirms without an interactive prompt; required in scripts.
    #[arg(long)]
    yes: bool,
}

#[derive(Args)]
struct RevokeKeyArgs {
    key_id: String,
}

#[derive(Args)]
struct KeygenArgs {
    /// Directory receiving `rdownloader-<role>.key` (PEM, keep secret) and `.pub`.
    #[arg(long, default_value = ".")]
    output: PathBuf,
    /// Which trust root the pair is for. Each role is separate on purpose: one compromise
    /// must not be able to vouch for everything (`crates/rd-sign/src/roots.rs`).
    #[arg(long, value_enum, default_value_t = KeyRole::Plugin)]
    role: KeyRole,
}

/// The roles `crates/rd-sign/src/roots.rs` knows, as a CLI value.
#[derive(Clone, Copy, clap::ValueEnum)]
enum KeyRole {
    /// Plugin packages and bundled manifests.
    Plugin,
    /// Application update manifests.
    Release,
    /// The managed external-tool manifest (RD-102-02).
    ToolManifest,
    /// Plugin repository indexes.
    Repository,
    /// The rule pack that recognises release pages (RD-110-04).
    SiteRules,
}

impl KeyRole {
    /// The `key_id` documents signed with this pair carry, matching `EMBEDDED_KEYS`.
    fn key_id(self) -> &'static str {
        match self {
            Self::Plugin => RELEASE_KEY_ID,
            Self::Release => "rdownloader-update-v1",
            Self::ToolManifest => "rdownloader-tools-v1",
            Self::Repository => "rdownloader-repository-v1",
            Self::SiteRules => rd_sign::SITE_RULES_KEY_ID,
        }
    }

    /// File-name stem, so two roles never overwrite each other's key files.
    fn file_stem(self) -> &'static str {
        match self {
            Self::Plugin => "rdownloader-plugin",
            Self::Release => "rdownloader-update",
            Self::ToolManifest => "rdownloader-tools",
            Self::Repository => "rdownloader-repository",
            Self::SiteRules => "rdownloader-siterules",
        }
    }
}

#[derive(Args)]
struct PackageArgs {
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
    package: PathBuf,
    /// Trusted key as KEY_ID=BASE64_ED25519_PUBLIC_KEY; repeatable.
    #[arg(long = "trusted-key")]
    trusted_keys: Vec<String>,
    #[arg(long)]
    development_mode: bool,
    /// Ignores the release key compiled into this binary.
    #[arg(long)]
    no_default_plugin_key: bool,
}

#[derive(Args)]
struct PluginInstallArgs {
    #[command(flatten)]
    package: PluginPackageArgs,
    #[arg(long, default_value = "data/plugins")]
    root: PathBuf,
}

pub async fn run(args: PluginArgs) -> Result<()> {
    match args.command {
        PluginCommand::Keygen(args) => keygen(&args).await,
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
        PluginCommand::Install(args) => {
            let verifier = build_plugin_verifier(
                args.package.development_mode,
                &args.package.trusted_keys,
                !args.package.no_default_plugin_key,
            )?;
            let installer = rd_plugin_host::PluginInstaller::new(args.root, verifier);
            match installer.install(args.package.package).await {
                Ok(installed) => {
                    println!("installed {}", installed.path.display());
                    Ok(())
                }
                Err(error) => Err(describe_verify_error(error)),
            }
        }
        PluginCommand::Keys(args) => keys(args).await,
        PluginCommand::Conformance(args) => conformance(&args).await,
        PluginCommand::New(args) => scaffold(&args),
    }
}

/// Turns an untrusted key into actionable advice instead of a bare failure.
fn describe_verify_error(error: VerifyError) -> anyhow::Error {
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
fn group_fingerprint(fingerprint: &str) -> String {
    fingerprint
        .as_bytes()
        .chunks(8)
        .map(|chunk| String::from_utf8_lossy(chunk).into_owned())
        .collect::<Vec<_>>()
        .join(" ")
}

async fn keys(args: KeysArgs) -> Result<()> {
    let database = rd_db::Database::open(&args.database).await?;
    match args.command {
        KeysCommand::List => {
            let keys = database.list_plugin_trusted_keys().await?;
            if keys.is_empty() {
                println!("no plugin signing keys confirmed on this installation");
            }
            for key in keys {
                println!("{}", key.key_id);
                println!("  fingerprint: {}", group_fingerprint(&key.fingerprint));
                if let Some(plugin) = &key.plugin_name {
                    println!("  first seen:  {plugin}");
                }
                println!("  confirmed:   {}", key.confirmed_at);
            }
            Ok(())
        }
        KeysCommand::Trust(trust) => {
            // Verify with an empty trust store so the package's own key is reported back.
            let verifier = rd_plugin_host::PluginVerifier::new(false);
            let Err(VerifyError::UntrustedKey {
                key_id,
                public_key,
                fingerprint,
                name,
                version,
            }) = verifier.verify_file(&trust.from_package)
            else {
                bail!(
                    "{} is not signed by a confirmable key (already trusted, unsigned or invalid)",
                    trust.from_package.display()
                );
            };
            println!("{name} {version}");
            println!("  key id:      {key_id}");
            println!("  fingerprint: {}", group_fingerprint(&fingerprint));
            if !trust.yes {
                bail!("re-run with --yes to confirm this key");
            }
            database
                .trust_plugin_key(rd_db::NewPluginTrustedKey {
                    key_id: key_id.clone(),
                    public_key,
                    fingerprint,
                    plugin_name: Some(name),
                })
                .await?;
            println!("trusted {key_id}");
            Ok(())
        }
        KeysCommand::Revoke(revoke) => {
            if database.revoke_plugin_key(revoke.key_id.clone()).await? {
                println!("revoked {}", revoke.key_id);
                println!("plugins signed with it are skipped from the next start");
            } else {
                println!("{} was not trusted", revoke.key_id);
            }
            Ok(())
        }
    }
}

async fn keygen(args: &KeygenArgs) -> Result<()> {
    tokio::fs::create_dir_all(&args.output).await?;
    let stem = args.role.file_stem();
    let private_path = args.output.join(format!("{stem}.key"));
    let public_path = args.output.join(format!("{stem}.pub"));
    if private_path.exists() || public_path.exists() {
        bail!("key files already exist in {}", args.output.display());
    }
    let generated = rd_plugin_host::generate_signing_key();
    tokio::fs::write(&private_path, &generated.private_pem).await?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(&private_path, std::fs::Permissions::from_mode(0o600)).await?;
    }
    tokio::fs::write(&public_path, format!("{}\n", generated.public_base64)).await?;
    println!("private key: {}", private_path.display());
    println!("public key:  {}", public_path.display());
    println!("key id:      {}", args.role.key_id());
    if matches!(args.role, KeyRole::Plugin) {
        println!(
            "trust flag:  --trusted-plugin-key {RELEASE_KEY_ID}={}",
            generated.public_base64
        );
    }
    println!(
        "paste the public key into EMBEDDED_KEYS in crates/rd-sign/src/roots.rs to make it the default"
    );
    Ok(())
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
    let bytes = std::fs::read(&args.package.package)
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

/// Writes a buildable plugin from the SDK templates.
///
/// The scaffold is a real, compiling plugin rather than a sketch with holes in it: an author
/// should be able to build and package it before changing a line, so that a later failure is
/// unambiguously about their code.
fn scaffold(args: &NewPluginArgs) -> Result<()> {
    let templates = sdk_templates()?;
    let source = templates.join(args.plugin_type.name());
    anyhow::ensure!(
        source.is_dir(),
        "SDK template {} is missing",
        source.display()
    );
    anyhow::ensure!(
        !args.out.exists(),
        "{} already exists; choose a directory that does not",
        args.out.display()
    );
    let name = match &args.name {
        Some(name) => name.clone(),
        None => args
            .out
            .file_name()
            .and_then(|value| value.to_str())
            .context("--out needs a directory name")?
            .to_owned(),
    };
    let slug: String = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    anyhow::ensure!(
        slug.len() >= 2 && slug.starts_with(|c: char| c.is_ascii_lowercase()),
        "`{name}` does not make a usable slug; pass --name"
    );
    // Every scaffold gets its own identity: two plugins sharing a UUID would fight over the
    // same installation directory.
    let id = uuid::Uuid::now_v7();
    // And its own signing key, so the scaffold can be built, packaged and verified before a
    // line of it is changed. An author who has to stop and generate a key first finds out
    // about the packaging step at the worst possible moment: when something else is broken.
    let key = rd_plugin_host::generate_signing_key();
    copy_template(
        &source,
        &args.out,
        &Substitutions {
            name: &name,
            slug: &slug,
            id: &id.to_string(),
            public_key: &key.public_base64,
        },
    )?;
    let key_path = args.out.join("plugin-signing.key");
    std::fs::write(&key_path, &key.private_pem)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600))?;
    }
    println!("scaffolded {} in {}", name, args.out.display());
    println!("  id:      {id}");
    println!("  slug:    {slug}");
    println!(
        "  key:     {} (keep this out of version control)",
        key_path.display()
    );
    println!("next:");
    println!("  cargo component build --release --target wasm32-unknown-unknown");
    println!(
        "  rdownloader plugin package --manifest manifest.toml \\\n    --component target/wasm32-unknown-unknown/release/{slug}.wasm \\\n    --locales locales --key plugin-signing.key --output {slug}.rdplug"
    );
    Ok(())
}

/// Locates `sdk/templates`, whether running from the repository or from an installation.
fn sdk_templates() -> Result<PathBuf> {
    let candidates = [
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../sdk/templates"),
        std::env::current_dir()?.join("sdk/templates"),
    ];
    candidates
        .into_iter()
        .find(|path| path.is_dir())
        .context("could not find the SDK templates; run from a checkout of the repository")
}

/// What the template placeholders stand for.
struct Substitutions<'a> {
    name: &'a str,
    slug: &'a str,
    id: &'a str,
    public_key: &'a str,
}

fn copy_template(
    source: &std::path::Path,
    target: &std::path::Path,
    values: &Substitutions<'_>,
) -> Result<()> {
    std::fs::create_dir_all(target)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let destination = target.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_template(&entry.path(), &destination, values)?;
            continue;
        }
        let text = std::fs::read_to_string(entry.path())
            .with_context(|| format!("read template {}", entry.path().display()))?;
        let filled = text
            .replace("{{PLUGIN_NAME}}", values.name)
            .replace("{{PLUGIN_SLUG}}", values.slug)
            .replace("{{PLUGIN_ID}}", values.id)
            .replace(
                "REPLACE_WITH_YOUR_BASE64_ED25519_PUBLIC_KEY",
                values.public_key,
            );
        std::fs::write(&destination, filled)
            .with_context(|| format!("write {}", destination.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use clap::ValueEnum;

    use super::NewPluginType;

    /// Every `--type` value has a template, and the template declares that very type.
    ///
    /// The guard that makes the enum cheap to extend: adding a world is one variant, and if
    /// its template is missing or names another `plugin_type`, this fails here instead of the
    /// first time somebody runs `plugin new` — which is what happened to `oauth`, offered by
    /// the core since RD-103-00 and scaffoldable by nobody.
    #[test]
    fn every_scaffoldable_type_has_a_template_that_declares_it() {
        let templates = super::sdk_templates().expect("sdk templates");
        for variant in NewPluginType::value_variants() {
            let name = variant.name();
            let directory = templates.join(&name);
            assert!(
                directory.is_dir(),
                "`plugin new --type {name}` has no template at {}",
                directory.display()
            );
            let manifest = std::fs::read_to_string(directory.join("manifest.toml"))
                .expect("template manifest");
            assert!(
                manifest.contains(&format!("plugin_type = \"{name}\"")),
                "{name} template declares another plugin_type"
            );
            assert!(
                directory.join("wit/rdownloader.wit").is_file(),
                "{name} template ships no copy of the contract"
            );
        }
    }

    /// The contract copy every template carries is the one this core speaks.
    ///
    /// CI checks the same thing with `diff -u`; having it here too means a template that
    /// drifted is caught by a plain `cargo nextest run -p rdownloader`.
    #[test]
    fn every_template_carries_the_current_contract() {
        let contract = std::fs::read_to_string(
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../rd-plugin-api/wit/rdownloader.wit"),
        )
        .expect("the contract");
        let templates = super::sdk_templates().expect("sdk templates");
        for entry in std::fs::read_dir(&templates).expect("read templates") {
            let path = entry.expect("entry").path().join("wit/rdownloader.wit");
            if !path.is_file() {
                continue;
            }
            let copy = std::fs::read_to_string(&path).expect("template contract");
            assert_eq!(copy, contract, "{} is out of date", path.display());
        }
    }
}
