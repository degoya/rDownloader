//! `rdownloader plugin …` subcommands: keygen, package, verify, install, keys, index.
//!
//! The publishing commands — package, verify, conformance, new, index — are `rd_pack::plugin`,
//! flattened in here, and the same ones the `rd-pack` binary offers (RD-150-20). What stays here
//! needs an installation or is run once by hand: keygen, install and the trusted keys.

use std::path::PathBuf;

use anyhow::{Result, bail};
use clap::{Args, Subcommand};
use rd_pack::plugin::{
    PluginPackageArgs, build_plugin_verifier, describe_verify_error, group_fingerprint,
};
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
    #[command(flatten)]
    Publish(rd_pack::plugin::PluginCommand),
    /// Installs a validated, version-pinned package atomically.
    Install(PluginInstallArgs),
    /// Inspects and edits the trust-on-first-use signing keys.
    Keys(KeysArgs),
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
            Self::Release => rd_sign::UPDATE_KEY_ID,
            Self::ToolManifest => "rdownloader-tools-v1",
            Self::Repository => rd_sign::REPOSITORY_KEY_ID,
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
struct PluginInstallArgs {
    #[command(flatten)]
    package: PluginPackageArgs,
    #[arg(long, default_value = "data/plugins")]
    root: PathBuf,
}

pub async fn run(args: PluginArgs) -> Result<()> {
    match args.command {
        PluginCommand::Keygen(args) => keygen(&args).await,
        PluginCommand::Publish(command) => rd_pack::plugin::run(command).await,
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
    }
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
    if matches!(args.role, KeyRole::Release) {
        println!(
            "CI secret:   RDOWNLOADER_UPDATE_SIGNING_KEY = the contents of {}",
            private_path.display()
        );
    }
    if matches!(args.role, KeyRole::Repository) {
        println!(
            "CI secret:   RDOWNLOADER_REPOSITORY_SIGNING_KEY = the contents of {}",
            private_path.display()
        );
    }
    println!(
        "paste the public key into EMBEDDED_KEYS in crates/rd-sign/src/roots.rs to make it the default"
    );
    Ok(())
}
