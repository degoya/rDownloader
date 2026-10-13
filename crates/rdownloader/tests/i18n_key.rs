//! Guards `scripts/i18n-key.sh`, the one sanctioned way to add a translation key, where the
//! Bash test cannot: the order of the keys a catalogue already holds.
//!
//! Every other case — a code flat in all four languages, nesting, the refusals and that a
//! refusal writes nothing — is `scripts/tests/i18n-key.sh` since RD-1280-22. This one, two keys
//! added after an existing one and found in the order they were written rather than sorted, is
//! the one property the Bash test does not pin.
//!
//! The case runs the script against a throwaway catalogue tree through `RD_LOCALES_DIR`, which
//! exists for exactly this.
//!
//! Unix only: the script is a bash tool for the repository, and on a Windows runner `bash` is
//! WSL's launcher without a distribution behind it.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::Command;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root")
}

/// A catalogue tree in a temporary directory: `codes` flat the way `server.json` keeps it, and a
/// nested group the way every other catalogue does.
fn catalogues(name: &str) -> PathBuf {
    let directory = std::env::temp_dir().join(format!("rd-i18n-key-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    for language in ["de", "en", "es", "fr"] {
        let path = directory.join(language);
        std::fs::create_dir_all(&path).expect("locale directory");
        std::fs::write(
            path.join("server.json"),
            "{\n  \"codes\": {\n    \"collector.already_here\": \"vorhanden\"\n  }\n}\n",
        )
        .expect("server catalogue");
        std::fs::write(
            path.join("plugins.json"),
            "{\n  \"actions\": {\n    \"enable\": \"Aktivieren\"\n  }\n}\n",
        )
        .expect("plugins catalogue");
    }
    directory
}

fn run(directory: &Path, arguments: &[&str]) -> std::process::Output {
    Command::new("bash")
        .arg(workspace_root().join("scripts/i18n-key.sh"))
        .args(arguments)
        .env("RD_LOCALES_DIR", directory)
        .output()
        .expect("run i18n-key.sh")
}

fn read(directory: &Path, language: &str, catalogue: &str) -> String {
    std::fs::read_to_string(directory.join(language).join(format!("{catalogue}.json")))
        .expect("catalogue")
}

#[test]
fn the_order_of_the_existing_keys_is_kept() {
    let directory = catalogues("order");
    for key in ["codes.a\\.two", "codes.a\\.one"] {
        let output = run(&directory, &["server", key, "A", "B", "C", "D"]);
        assert!(output.status.success(), "{output:?}");
    }
    let text = read(&directory, "en", "server");
    let first = text.find("collector.already_here").expect("existing key");
    let second = text.find("a.two").expect("first addition");
    let third = text.find("a.one").expect("second addition");
    assert!(
        first < second && second < third,
        "keys have to keep the order they were written in, not be sorted: {text}"
    );
    let _ = std::fs::remove_dir_all(&directory);
}
