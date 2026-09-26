//! Guards `scripts/i18n-key.sh`, the one sanctioned way to add a translation key.
//!
//! The script is the rule: `AGENTS.md` says new keys are added with it and nothing else, because
//! it writes all four languages in one go. So the group where most new keys appear -- the error
//! codes in `server.json` -- has to be reachable by it. Until RD-120-24 it was not: `codes` holds
//! literal keys with dots in them, and a dotted argument built a nested group instead, which
//! resolved nowhere. All four catalogues agreed about that, so the language comparison could not
//! see it either (`web/src/i18n/sourceKeys.test.ts` says which six codes got through that way).
//!
//! These cases run the script against a throwaway catalogue tree through `RD_LOCALES_DIR`, which
//! exists for exactly this: the script is a repository tool, and there is no other way to check
//! what it does to a file without letting it write to the real ones.
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
fn an_error_code_lands_flat_inside_codes_in_all_four_languages() {
    let directory = catalogues("flat");
    let output = run(
        &directory,
        &[
            "server",
            r"codes.collector\.check_no_resolver",
            "Kein Resolver",
            "No resolver",
            "Sin resolver",
            "Aucun resolveur",
        ],
    );
    assert!(output.status.success(), "{output:?}");

    for (language, expected) in [
        ("de", "Kein Resolver"),
        ("en", "No resolver"),
        ("es", "Sin resolver"),
        ("fr", "Aucun resolveur"),
    ] {
        let text = read(&directory, language, "server");
        let parsed: serde_json::Value = serde_json::from_str(&text).expect("valid json");
        assert_eq!(
            parsed["codes"]["collector.check_no_resolver"], expected,
            "{language} did not get the code as one literal key: {text}"
        );
    }
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn a_nested_key_still_nests() {
    let directory = catalogues("nested");
    let output = run(
        &directory,
        &[
            "plugins",
            "actions.disable",
            "Deaktivieren",
            "Disable",
            "Desactivar",
            "Desactiver",
        ],
    );
    assert!(output.status.success(), "{output:?}");

    let parsed: serde_json::Value =
        serde_json::from_str(&read(&directory, "en", "plugins")).expect("valid json");
    assert_eq!(parsed["actions"]["disable"], "Disable");
    assert_eq!(parsed["actions"]["enable"], "Aktivieren");
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn a_dotted_key_that_would_open_a_group_inside_a_flat_one_is_refused() {
    let directory = catalogues("guard");
    let output = run(
        &directory,
        &["server", "codes.collector.other", "A", "B", "C", "D"],
    );
    assert!(!output.status.success(), "the script should have refused");

    let message = String::from_utf8_lossy(&output.stderr);
    assert!(
        message.contains(r"'codes.collector\.other'"),
        "the refusal has to name the escaped form to use: {message}"
    );
    // Nothing was written: a refusal that has already edited two of four catalogues is worse
    // than the mistake it refuses.
    assert_eq!(
        read(&directory, "de", "server"),
        "{\n  \"codes\": {\n    \"collector.already_here\": \"vorhanden\"\n  }\n}\n"
    );
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn an_existing_code_is_refused() {
    let directory = catalogues("exists");
    let output = run(
        &directory,
        &[
            "server",
            r"codes.collector\.already_here",
            "A",
            "B",
            "C",
            "D",
        ],
    );
    assert!(
        !output.status.success(),
        "an existing leaf has to be refused"
    );
    let _ = std::fs::remove_dir_all(&directory);
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

#[test]
fn a_new_subgroup_under_an_ordinary_group_is_still_allowed() {
    // `actions` holds only strings, like 407 groups in the real catalogues. The guard is about
    // groups whose keys carry dots; a first subgroup here is ordinary nesting and must work.
    let directory = catalogues("subgroup");
    let output = run(
        &directory,
        &[
            "plugins",
            "actions.bulk.enable",
            "Alle",
            "All",
            "Todos",
            "Tous",
        ],
    );
    assert!(output.status.success(), "{output:?}");
    let parsed: serde_json::Value =
        serde_json::from_str(&read(&directory, "en", "plugins")).expect("valid json");
    assert_eq!(parsed["actions"]["bulk"]["enable"], "All");
    let _ = std::fs::remove_dir_all(&directory);
}
