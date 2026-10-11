//! Guards the rule that no program the service starts opens a console window on Windows.
//!
//! 1.8.0 live finding: after a self-update the updater had started the service without a
//! console, and every spawn that did not ask for none opened a visible window of its own - the
//! archive tools first, one window per extraction. Each spawn now asks through
//! `rd_files::NoConsoleWindow`, and this test scans `crates/*/src` for `Command::new(`: the
//! function around it must use one of [`ACCEPTED`], or the call must sit under an attribute
//! that keeps it off Windows ([`OFF_WINDOWS`]). Test code is not checked (`#[cfg(test)]`,
//! `*_tests.rs`, `tests.rs`). The scan reads the source as rustfmt writes it: an item ends at the
//! closing brace at its own indentation.
//!
//! A second rule: only [`CREATION_FLAGS_FILES`] call `creation_flags`, because the call replaces
//! the flags set before - a later one anywhere else would undo the helper unseen.

use std::path::{Path, PathBuf};

/// What a function that starts a program uses so the program opens no window: the helper, the
/// two `rd_tools::process` wrappers that apply it, or the flag itself where the updater combines
/// it with others.
const ACCEPTED: &[&str] = &[
    "no_console_window(",
    "run_to_output(",
    "ToolProcess::spawn(",
    "CREATE_NO_WINDOW",
];

/// Attributes under which an item is never compiled for Windows, or only for tests.
const OFF_WINDOWS: &[&str] = &[
    "#[cfg(unix)]",
    "#[cfg(not(windows))]",
    "#[cfg(target_os = \"linux\")]",
    "#[cfg(not(any(windows, target_os = \"macos\")))]",
    "#[cfg(target_os = \"macos\")]",
    "#[cfg(test)]",
];

/// Whole files compiled only off Windows, gated where their module is declared.
const OFF_WINDOWS_FILES: &[&str] = &[
    "crates/rd-autostart/src/platform_linux.rs",
    "crates/rd-autostart/src/platform_macos.rs",
    "crates/rd-power/src/linux.rs",
    "crates/rd-power/src/macos.rs",
];

/// The helper, and the updater, which starts the service with the flag plus a process group and
/// a job breakaway of its own.
const CREATION_FLAGS_FILES: &[&str] = &[
    "crates/rd-files/src/child_process.rs",
    "crates/rd-update/src/install/process.rs",
];

/// Words that may stand before `fn` in a function's first line.
const QUALIFIERS: &[&str] = &[
    "pub",
    "pub(crate)",
    "pub(super)",
    "async",
    "const",
    "unsafe",
];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root")
}

fn rust_sources(directory: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(directory).expect("read dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            rust_sources(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

fn is_test_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == "tests.rs" || name.ends_with("_tests.rs"))
}

fn indent(line: &str) -> usize {
    line.len() - line.trim_start_matches(' ').len()
}

/// The last line of the item that starts at `start`: the line that ends it with `;` before any
/// brace, a line that opens and closes it, or else the closing brace at `indent`.
fn item_end(lines: &[&str], start: usize, indent: usize) -> usize {
    let last = lines.len().saturating_sub(1);
    let closing = format!("{}}}", " ".repeat(indent));
    for (index, line) in lines.iter().enumerate().skip(start) {
        let line = line.trim_end();
        if (line.ends_with(';') && !line.contains('{'))
            || (line.ends_with('}') && line.contains('{'))
        {
            return index;
        }
        if line.ends_with('{') {
            return lines
                .iter()
                .enumerate()
                .skip(index + 1)
                .find(|(_, line)| line.trim_end() == closing)
                .map_or(last, |(end, _)| end);
        }
    }
    last
}

fn is_fn_header(line: &str) -> bool {
    line.split_whitespace()
        .find(|word| !QUALIFIERS.contains(word))
        == Some("fn")
}

/// Which lines lie in an item under one of [`OFF_WINDOWS`].
fn off_windows_lines(lines: &[&str]) -> Vec<bool> {
    let mut off = vec![false; lines.len()];
    for (index, line) in lines.iter().enumerate() {
        if !OFF_WINDOWS.contains(&line.trim()) {
            continue;
        }
        let item = (index + 1..lines.len())
            .find(|&next| {
                let next = lines[next].trim_start();
                !next.starts_with("#[") && !next.starts_with("//")
            })
            .unwrap_or(index);
        let end = item_end(lines, item, indent(line));
        off[index..=end].fill(true);
    }
    off
}

/// How many spawns `source` has that are compiled for Windows, and the line numbers of those
/// whose function applies none of [`ACCEPTED`].
fn unguarded_spawns(source: &str) -> (usize, Vec<usize>) {
    let lines = source.lines().collect::<Vec<_>>();
    let off = off_windows_lines(&lines);
    let functions = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| is_fn_header(line))
        .map(|(start, line)| (start, item_end(&lines, start, indent(line))))
        .collect::<Vec<_>>();
    let mut spawns = 0;
    let mut unguarded = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if !line.contains("Command::new(") || line.trim_start().starts_with("//") || off[index] {
            continue;
        }
        spawns += 1;
        // The innermost function around the spawn: the one that starts last.
        let guarded = functions
            .iter()
            .filter(|(start, end)| (*start..=*end).contains(&index))
            .max()
            .is_some_and(|(start, end)| {
                let body = lines[*start..=*end].join("\n");
                ACCEPTED.iter().any(|accepted| body.contains(accepted))
            });
        if !guarded {
            unguarded.push(index + 1);
        }
    }
    (spawns, unguarded)
}

#[test]
fn no_spawn_opens_a_console_window() {
    let root = workspace_root();
    let mut sources = Vec::new();
    for entry in std::fs::read_dir(root.join("crates")).expect("read crates") {
        let src = entry.expect("dir entry").path().join("src");
        if src.is_dir() {
            rust_sources(&src, &mut sources);
        }
    }
    let mut spawns = 0;
    let mut unguarded = Vec::new();
    let mut flags = Vec::new();
    for path in sources.iter().filter(|path| !is_test_file(path)) {
        let relative = path.strip_prefix(&root).expect("relative");
        // `Path` equality compares components, so the `/` spelling matches a Windows `\`.
        let listed = |list: &[&str]| list.iter().any(|entry| relative == Path::new(entry));
        let source = std::fs::read_to_string(path).expect("read source");
        if source.contains("creation_flags(") && !listed(CREATION_FLAGS_FILES) {
            flags.push(relative.display().to_string());
        }
        if listed(OFF_WINDOWS_FILES) {
            continue;
        }
        let (found, lines) = unguarded_spawns(&source);
        spawns += found;
        unguarded.extend(
            lines
                .into_iter()
                .map(|line| format!("{}:{line}", relative.display())),
        );
    }
    assert!(
        spawns >= 30,
        "expected to find the workspace's spawn sites, found {spawns}"
    );
    assert!(
        unguarded.is_empty(),
        "a spawn without rd_files::NoConsoleWindow opens a console window on Windows:\n{}",
        unguarded.join("\n")
    );
    assert!(
        flags.is_empty(),
        "creation_flags replaces the console-window flag; use rd_files::NoConsoleWindow:\n{}",
        flags.join("\n")
    );
}

/// The scan itself: a bare spawn is found, at any depth, and a guarded or Linux-only one is not.
#[test]
fn the_scan_finds_a_bare_spawn() {
    let source = r#"fn bare() {
    let _ = std::process::Command::new("reg.exe").status();
}

fn guarded() {
    let _ = std::process::Command::new("reg.exe").no_console_window().status();
}

#[cfg(target_os = "linux")]
fn linux_only() {
    let _ = std::process::Command::new("systemctl").status();
}

mod platform {
    pub(super) fn nested() -> Result<()> {
        run(Command::new("reg.exe")
            .no_console_window())
    }

    fn one_liner() -> u32 { 1 }

    fn inner_bare() {
        let _ = Command::new("icacls.exe").status();
    }
}
"#;
    assert_eq!(unguarded_spawns(source), (4, vec![2, 23]));
}
