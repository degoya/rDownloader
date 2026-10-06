//! `plugin new`: a buildable plugin from the SDK templates, with its own identity and key.

use std::path::PathBuf;

use anyhow::{Context, Result};

use super::NewPluginArgs;

/// Writes a buildable plugin from the SDK templates.
///
/// The scaffold is a real, compiling plugin rather than a sketch with holes in it: an author
/// should be able to build and package it before changing a line, so that a later failure is
/// unambiguously about their code.
pub(super) fn scaffold(args: &NewPluginArgs) -> Result<()> {
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
    // The ignore rules go in before the key does: a printed warning was the only guard, and
    // `git add .` in a fresh plugin repository committed the private key (audit K1).
    write_gitignore(&args.out)?;
    let key_path = args.out.join(SIGNING_KEY_FILE);
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
        "  key:     {} (listed in .gitignore; keep it out of version control)",
        key_path.display()
    );
    println!("next:");
    println!("  cargo build --release --target wasm32-unknown-unknown");
    println!(
        "  wasm-tools component new target/wasm32-unknown-unknown/release/{slug}.wasm -o target/{slug}.wasm"
    );
    println!(
        "  rdownloader plugin package --manifest manifest.toml \\\n    --component target/{slug}.wasm \\\n    --locales locales --key plugin-signing.key --output {slug}.rdplug"
    );
    Ok(())
}

/// Locates `sdk/templates`, whether running from the repository or from an installation.
///
/// The crate's own directory is read at run time, where cargo and the test runners set it. As
/// `env!` it was compiled into the release binary as the build machine's checkout path, which
/// `--remap-path-prefix` does not reach, so two checkouts built two different executables
/// (RD-180-12, docs/reproducible-builds.md).
pub(super) fn sdk_templates() -> Result<PathBuf> {
    let candidates = [
        std::env::var_os("CARGO_MANIFEST_DIR")
            .map(|dir| PathBuf::from(dir).join("../../sdk/templates")),
        Some(std::env::current_dir()?.join("sdk/templates")),
    ];
    candidates
        .into_iter()
        .flatten()
        .find(|path| path.is_dir())
        .context("could not find the SDK templates; run from a checkout of the repository")
}

/// The scaffold's private signing key, next to its manifest.
const SIGNING_KEY_FILE: &str = "plugin-signing.key";

/// What a scaffold keeps out of version control: the private key, the build output and the
/// packages built from it.
const IGNORED: [&str; 3] = [SIGNING_KEY_FILE, "target/", "*.rdplug"];

/// Adds [`IGNORED`] to the scaffold's `.gitignore`, keeping whatever the template already put
/// there.
pub(super) fn write_gitignore(directory: &std::path::Path) -> Result<()> {
    let path = directory.join(".gitignore");
    let mut text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => {
            return Err(error).with_context(|| format!("read {}", path.display()));
        }
    };
    for entry in IGNORED {
        if text.lines().any(|line| line.trim() == entry) {
            continue;
        }
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(entry);
        text.push('\n');
    }
    std::fs::write(&path, text).with_context(|| format!("write {}", path.display()))
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
