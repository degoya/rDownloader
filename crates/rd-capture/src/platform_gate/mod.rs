//! What a Linux build of the capture agent is allowed to link, held as a fact about the
//! manifest rather than as a fact about how somebody happened to spell a line.
//!
//! The Linux agent is headless: it links no window toolkit, no GTK and no WebKitGTK. Nothing
//! fails when that decision is broken — the build on this machine goes green, and the cost
//! arrives later, on a server that has no display libraries and now needs them. So it is
//! checked here, and checked in the direction that stays true as the crate grows: every
//! dependency an ordinary Linux build reads has to be named below with the reason it carries
//! no window stack behind it. A crate added tomorrow is refused until somebody writes that
//! reason, instead of being waved through until somebody remembers to extend a list of
//! forbidden names.
//!
//! Where the line actually runs: `arboard` is ungated and pulls `x11rb` and `wl-clipboard-rs`
//! behind it, so the headless agent does link X11 and Wayland *client* libraries. A clipboard
//! client is not a window toolkit — it needs no GTK, and with no display server reachable it
//! returns an error instead of taking the process down. That is the line, and it is the right
//! one; `crates/rd-capture/Cargo.toml` says the same next to the gate.
//!
//! What this does not catch: a window stack pulled in *transitively*, by a dependency that
//! looks innocent — a new dependency of `rd-core`, a feature flipped on somewhere in the
//! workspace, a `[patch]` that redirects a harmless name onto a fork with GTK underneath. No
//! parse of this manifest can see any of it. That is the second half of the guard and it
//! lives outside this file: `scripts/check-capture-linux-tree.sh` holds the *resolved* Linux
//! tree against a named list of window, widget and rendering stacks, and CI runs it on every
//! push (RD-109-37). The two halves answer different questions — this one asks what was
//! declared and why, that one asks what the resolver actually produced — and the case below
//! keeps the crates named here from dropping out of the list over there.
//!
//! The source side has a backstop of its own that neither manifest nor tree provides: window
//! code written straight into an ungated file does not link a GUI stack quietly, it fails to
//! compile on Linux, and CI compiles this crate on Linux.

use std::{fs, path::Path};

/// Whether a dependency is linked into the agent or only into the machinery that builds it.
///
/// Both matter. A build- or dev-dependency on a window toolkit does not ship, but it still
/// makes the Linux build demand display libraries that are not there.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Reach {
    Linked,
    Tooling,
}

/// Every dependency an ordinary Linux build links, with the reason it drags no window stack.
///
/// This is the whole rule, in the one direction that does not rot: what is not here does not
/// reach the Linux agent. Adding a crate means adding its reason.
const LINUX_LINKED: &[(&str, &str)] = &[
    ("aes", "pure-Rust block cipher for the Click'n'Load payload"),
    ("anyhow", "error type"),
    (
        "arboard",
        "clipboard client; the one ungated crate that touches the display server. It pulls \
         x11rb and wl-clipboard-rs, which are protocol clients, not a toolkit: no GTK, and a \
         missing display server is an error rather than a crash. Clipboard watching is wanted \
         on Linux, so this is where the line runs",
    ),
    ("axum", "HTTP server behind Click'n'Load"),
    ("base64", "encoding"),
    (
        "boa_engine",
        "sandboxed JavaScript for container decryption",
    ),
    ("cbc", "block cipher mode"),
    ("chrono", "time"),
    ("clap", "command line"),
    ("directories", "per-user paths"),
    ("hex", "encoding"),
    (
        "keyring",
        "credential storage; the Linux backend talks to the Secret Service over zbus, which is \
         a pure-Rust D-Bus client",
    ),
    (
        "notify-rust",
        "desktop notifications; built with the z feature, so Linux uses zbus and no GTK",
    ),
    ("rd-collector", "workspace crate, headless"),
    ("rd-autostart", "workspace crate, headless"),
    ("rd-core", "workspace crate, headless"),
    ("regex", "text"),
    ("reqwest", "HTTP client"),
    ("serde", "serialization"),
    ("serde_json", "serialization"),
    ("sha2", "hashing"),
    ("tokio", "runtime"),
    ("tokio-util", "runtime helpers"),
    ("tower-http", "HTTP middleware"),
    ("tracing", "logging"),
    ("tracing-subscriber", "logging"),
    ("url", "parsing"),
];

/// The same, for what only builds or tests the agent on Linux.
const LINUX_TOOLING: &[(&str, &str)] = &[
    (
        "embed-resource",
        "compiles the Windows resource file; on Linux it finds no toolchain and does nothing",
    ),
    ("toml", "reads this crate's manifest for the checks below"),
];

/// Crates that carry a window or rendering stack and must stay out of reach of a Linux build.
///
/// The reason list above already refuses them — they are not on it. This names them anyway,
/// so the failure says *why* rather than only that something is unaccounted for, and so that
/// deleting the target table cannot pass unnoticed.
const WINDOW_STACK: &[&str] = &["image", "open", "tao", "tray-icon"];

/// Modules whose code needs those crates, and which therefore may never be compiled on Linux.
const GATED_MODULES: &[&str] = &["tray"];

/// Modules that were removed on purpose and must not quietly return.
///
/// The captcha window went with RD-109-11: a widget captcha is answered in the person's own
/// browser through the extension. Bringing back an embedded WebView is a decision, not an edit.
const RETIRED_MODULES: &[&str] = &["captcha", "captcha_window"];

fn manifest_text() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    fs::read_to_string(&path).expect("the crate manifest is readable")
}

/// The key/value pairs that hold for an ordinary Linux build.
///
/// An unknown key — `feature`, anything else — yields `None`, which the callers read as
/// "cannot tell" and treat as reachable.
fn holds_on_linux(key: &str, value: &str) -> Option<bool> {
    const LINUX: &[(&str, &str)] = &[
        ("target_os", "linux"),
        ("target_family", "unix"),
        ("target_env", "gnu"),
        ("target_arch", "x86_64"),
        ("target_vendor", "unknown"),
        ("target_pointer_width", "64"),
        ("target_endian", "little"),
    ];
    LINUX
        .iter()
        .find(|(name, _)| *name == key)
        .map(|(_, expected)| *expected == value)
}

/// A cursor over a `cfg(...)` expression.
struct Predicate<'a> {
    rest: &'a str,
}

impl<'a> Predicate<'a> {
    fn skip_space(&mut self) {
        self.rest = self.rest.trim_start();
    }

    fn take(&mut self, wanted: char) -> bool {
        self.skip_space();
        match self.rest.strip_prefix(wanted) {
            Some(rest) => {
                self.rest = rest;
                true
            }
            None => false,
        }
    }

    fn identifier(&mut self) -> &'a str {
        self.skip_space();
        let end = self
            .rest
            .find(|character: char| !character.is_ascii_alphanumeric() && character != '_')
            .unwrap_or(self.rest.len());
        let (name, rest) = self.rest.split_at(end);
        self.rest = rest;
        name
    }

    fn string(&mut self) -> Option<&'a str> {
        self.skip_space();
        let rest = self.rest.strip_prefix('"')?;
        let end = rest.find('"')?;
        let (value, tail) = rest.split_at(end);
        self.rest = &tail[1..];
        Some(value)
    }

    /// `Some(true)`/`Some(false)` where the expression can be decided, `None` where it cannot.
    fn evaluate(&mut self) -> Option<bool> {
        let name = self.identifier();
        if name.is_empty() {
            return None;
        }
        if self.take('(') {
            let mut parts: Vec<Option<bool>> = Vec::new();
            loop {
                if self.take(')') {
                    break;
                }
                parts.push(self.evaluate());
                if self.take(',') {
                    continue;
                }
                if self.take(')') {
                    break;
                }
                return None;
            }
            return match name {
                "any" => combine(&parts, true),
                "all" => combine(&parts, false),
                "not" => match parts.as_slice() {
                    [single] => single.map(|held| !held),
                    _ => None,
                },
                _ => None,
            };
        }
        if self.take('=') {
            let value = self.string()?;
            return holds_on_linux(name, value);
        }
        match name {
            "unix" => Some(true),
            "windows" => Some(false),
            _ => None,
        }
    }
}

/// `any`/`all` over parts that may be undecidable: one decisive part wins, otherwise a single
/// undecidable part makes the whole thing undecidable.
fn combine(parts: &[Option<bool>], decisive: bool) -> Option<bool> {
    if parts.contains(&Some(decisive)) {
        return Some(decisive);
    }
    if parts.iter().any(Option::is_none) {
        return None;
    }
    Some(!decisive)
}

/// Whether the dependencies under a `[target.<key>]` table reach an ordinary Linux build.
///
/// Fails closed on purpose. An expression this cannot decide — a feature predicate, an unknown
/// key, a malformed cfg — counts as reachable, so its dependencies need a written reason. The
/// cost of being wrong that way is a sentence; the cost of the other way is a server that will
/// not build.
fn reaches_linux(key: &str) -> bool {
    let Some(rest) = key.strip_prefix("cfg(") else {
        // Not a cfg: a literal target triple such as `x86_64-pc-windows-msvc`.
        return key.contains("linux");
    };
    let Some(expression) = rest.strip_suffix(')') else {
        // A cfg that does not close is a cfg this cannot read.
        return true;
    };
    let mut predicate = Predicate { rest: expression };
    let verdict = predicate.evaluate();
    if !predicate.rest.trim().is_empty() {
        return true;
    }
    verdict.unwrap_or(true)
}

fn dependency_names(table: Option<&toml::Value>) -> Vec<String> {
    table
        .and_then(toml::Value::as_table)
        .map(|entries| entries.keys().cloned().collect())
        .unwrap_or_default()
}

const KINDS: &[(&str, Reach)] = &[
    ("dependencies", Reach::Linked),
    ("build-dependencies", Reach::Tooling),
    ("dev-dependencies", Reach::Tooling),
];

/// Every dependency table a Linux build reads: where it stands, how far it reaches, what it
/// declares. Both the shared tables and every `[target.…]` table whose cfg can hold on Linux.
fn linux_tables(manifest: &toml::Value) -> Vec<(String, Reach, Vec<String>)> {
    let mut tables = Vec::new();
    for (kind, reach) in KINDS {
        let names = dependency_names(manifest.get(kind));
        if !names.is_empty() {
            tables.push((format!("[{kind}]"), *reach, names));
        }
    }
    let targets = manifest.get("target").and_then(toml::Value::as_table);
    for (key, entry) in targets.into_iter().flatten() {
        if !reaches_linux(key) {
            continue;
        }
        for (kind, reach) in KINDS {
            let names = dependency_names(entry.get(kind));
            if !names.is_empty() {
                tables.push((format!("[target.'{key}'.{kind}]"), *reach, names));
            }
        }
    }
    tables
}

/// Everything wrong with a manifest, in the words the failing test prints.
///
/// Takes the text rather than reading the file so the cases below can hand it a manifest that
/// does what nobody has done yet.
fn unaccounted_dependencies(manifest: &str) -> Vec<String> {
    let parsed: toml::Value = toml::from_str(manifest).expect("the manifest is TOML");
    let mut complaints = Vec::new();
    for (table, reach, names) in linux_tables(&parsed) {
        for name in names {
            let reasons = match reach {
                Reach::Linked => LINUX_LINKED,
                Reach::Tooling => LINUX_TOOLING,
            };
            if WINDOW_STACK.contains(&name.as_str()) {
                complaints.push(format!(
                    "{name} in {table} reaches a Linux build; it pulls a window stack behind it \
                     and belongs behind the Windows/macOS gate"
                ));
            } else if !reasons.iter().any(|(known, _)| *known == name) {
                complaints.push(format!(
                    "{name} in {table} reaches a Linux build with no reason written beside it in \
                     platform_gate/mod.rs; write down why it links no window stack"
                ));
            }
        }
    }
    complaints
}

/// Every declared module under `src/`, as (file, module name, whatever governs it).
fn declared_modules() -> Vec<(String, String, String)> {
    let mut sources = Vec::new();
    collect_sources(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut sources,
    );
    let mut found = Vec::new();
    for (path, text) in sources {
        let lines: Vec<&str> = text.lines().collect();
        for (index, line) in lines.iter().enumerate() {
            let Some((name, prefix)) = declared_module(line) else {
                continue;
            };
            let governing = if prefix.contains("#[cfg(") {
                prefix.to_string()
            } else {
                preceding_attribute(&lines[..index])
            };
            found.push((path.clone(), name.to_string(), governing));
        }
    }
    found
}

fn collect_sources(directory: &Path, into: &mut Vec<(String, String)>) {
    let entries = fs::read_dir(directory).expect("the source directory is readable");
    for entry in entries {
        let path = entry.expect("the directory entry is readable").path();
        if path.is_dir() {
            collect_sources(&path, into);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            let text = fs::read_to_string(&path).expect("the source file is readable");
            into.push((path.display().to_string(), text));
        }
    }
}

/// The module a line declares, and whatever stands before it on that same line.
///
/// Indentation is deliberately ignored: a declaration is a declaration wherever in the file it
/// stands, and this check is not satisfied by looking only at the head of `main.rs`.
fn declared_module(line: &str) -> Option<(&str, &str)> {
    let trimmed = line.trim();
    let (prefix, declaration) = match trimmed.rfind("] ") {
        Some(end) if trimmed.starts_with("#[") => trimmed.split_at(end + 2),
        _ => ("", trimmed),
    };
    let declaration = declaration.trim_start();
    let body = declaration
        .strip_prefix("pub(crate) ")
        .or_else(|| declaration.strip_prefix("pub "))
        .unwrap_or(declaration);
    let name = body.strip_prefix("mod ")?.strip_suffix(';')?.trim();
    if name.is_empty()
        || !name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
    {
        return None;
    }
    Some((name, prefix))
}

/// The attribute line in front of a declaration, skipping blank lines and comments.
fn preceding_attribute(before: &[&str]) -> String {
    for line in before.iter().rev() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("//") {
            continue;
        }
        return trimmed.to_string();
    }
    String::new()
}

/// The `cfg(...)` inside an attribute, if it carries one.
fn cfg_of(attribute: &str) -> Option<&str> {
    let start = attribute.find("#[cfg(")?;
    let rest = &attribute[start + 2..];
    let end = rest.rfind(')')?;
    Some(&rest[..=end])
}

/// The cases, including the ones that hand this a manifest nobody has written yet.
#[cfg(test)]
mod tests;
