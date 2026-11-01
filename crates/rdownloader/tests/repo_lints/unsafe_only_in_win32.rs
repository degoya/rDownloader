//! `unsafe` code lives in `crates/rd-win32` and nowhere else in the host (owner, 2026-11-01,
//! RD-1400-07).
//!
//! The workspace denies `unsafe_code`, and for a long time a Windows call was therefore made by a
//! hidden PowerShell child instead -- which is what malware looks like: Microsoft Defender
//! quarantined the capture agent as `Trojan:Win32/Bearfoos.A!ml`. The calls moved into one crate
//! that allows `unsafe` at its root, documents every block (`clippy::undocumented_unsafe_blocks`)
//! and offers safe functions. A second `#![allow(unsafe_code)]` somewhere else would quietly
//! undo that, and code behind `cfg(windows)` is not even compiled by a Linux check; so this test
//! reads every Rust file under `crates/` -- tests included -- and refuses an `unsafe` keyword
//! (block, function, impl, trait, extern block, attribute) and an `allow`, `expect` or `warn` of
//! `unsafe_code` outside `crates/rd-win32/`. Comments and string literals do not count.
//!
//! `plugins/` is not read: a plugin is a WebAssembly guest, and its canonical-ABI glue, generated
//! by `wit-bindgen`, is `unsafe` by construction (each says so where it allows it).

use std::path::{Path, PathBuf};

use crate::workspace_root;

/// The one crate allowed `unsafe`, relative to the workspace root.
const ALLOWED: &str = "crates/rd-win32/";

fn rust_sources(directory: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(directory).expect("read dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            if path.file_name().is_some_and(|name| name == "target") {
                continue;
            }
            rust_sources(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// `source` with its comments and its string and character literals taken out; every line
/// break is kept, so a line number still points at the source.
fn code_only(source: &str) -> String {
    let chars: Vec<char> = source.chars().collect();
    let at = |index: usize| chars.get(index).copied();
    let mut out = String::with_capacity(source.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '/' && at(i + 1) == Some('/') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '/' && at(i + 1) == Some('*') {
            let mut depth = 0_usize;
            while i < chars.len() {
                if chars[i] == '/' && at(i + 1) == Some('*') {
                    depth += 1;
                    i += 2;
                } else if chars[i] == '*' && at(i + 1) == Some('/') {
                    depth -= 1;
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    if chars[i] == '\n' {
                        out.push('\n');
                    }
                    i += 1;
                }
            }
            continue;
        }
        // Raw strings: `r"…"`, `r#"…"#`, `br"…"`, `cr"…"`.
        if i == 0 || !is_ident(chars[i - 1]) {
            let mut j = i;
            if matches!(c, 'b' | 'c') {
                j += 1;
            }
            if at(j) == Some('r') {
                let mut k = j + 1;
                let mut hashes = 0;
                while at(k) == Some('#') {
                    hashes += 1;
                    k += 1;
                }
                if at(k) == Some('"') {
                    k += 1;
                    while k < chars.len() {
                        if chars[k] == '"' && (0..hashes).all(|h| at(k + 1 + h) == Some('#')) {
                            k += 1 + hashes;
                            break;
                        }
                        if chars[k] == '\n' {
                            out.push('\n');
                        }
                        k += 1;
                    }
                    out.push(' ');
                    i = k;
                    continue;
                }
            }
        }
        if c == '"' {
            i += 1;
            while i < chars.len() {
                match chars[i] {
                    '\\' => i += 2,
                    '"' => {
                        i += 1;
                        break;
                    }
                    '\n' => {
                        out.push('\n');
                        i += 1;
                    }
                    _ => i += 1,
                }
            }
            out.push(' ');
            continue;
        }
        if c == '\'' {
            // A character literal; a lifetime or a label (`'a`) stays.
            if at(i + 1) == Some('\\') {
                let mut k = i + 3;
                while k < chars.len() && chars[k] != '\'' {
                    k += 1;
                }
                i = k + 1;
                out.push(' ');
                continue;
            }
            if at(i + 2) == Some('\'') {
                i += 3;
                out.push(' ');
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    out
}

/// The lines (1-based) of `source` that hold `unsafe` code or let it in.
fn unsafe_lines(source: &str) -> Vec<usize> {
    code_only(source)
        .lines()
        .enumerate()
        .filter(|(_, line)| {
            let words: Vec<&str> = line.split(|c: char| !is_ident(c)).collect();
            words.contains(&"unsafe")
                || (words.contains(&"unsafe_code")
                    && ["allow", "expect", "warn"]
                        .iter()
                        .any(|level| words.contains(level)))
        })
        .map(|(index, _)| index + 1)
        .collect()
}

#[test]
fn unsafe_code_lives_only_in_rd_win32() {
    let root = workspace_root();
    let mut files = Vec::new();
    rust_sources(&root.join("crates"), &mut files);
    assert!(
        files
            .iter()
            .any(|path| path.ends_with("crates/rd-win32/src/lib.rs")),
        "rd-win32, the one crate allowed `unsafe`, was not found"
    );
    let mut offenders = Vec::new();
    for path in files {
        let relative = path
            .strip_prefix(&root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        if relative.starts_with(ALLOWED) {
            continue;
        }
        let source = std::fs::read_to_string(&path).expect("read source");
        for line in unsafe_lines(&source) {
            offenders.push(format!("{relative}:{line}"));
        }
    }
    assert!(
        offenders.is_empty(),
        "`unsafe` code, or an allowance for it, outside {ALLOWED} -- put the call behind a safe \
         function in rd-win32 instead:\n  {}",
        offenders.join("\n  ")
    );
}

#[test]
fn the_scan_sees_code_and_skips_comments_and_literals() {
    let found = [
        "    unsafe { call() };",
        "pub unsafe fn raw() {}",
        "unsafe impl Send for Thing {}",
        "unsafe extern \"C\" {",
        "#[unsafe(no_mangle)]",
        "#![allow(unsafe_code)]",
        "#[expect(clippy::all, unsafe_code)]",
        "let x = 'a'; unsafe { f() }",
    ];
    for line in found {
        assert_eq!(unsafe_lines(line), vec![1], "{line}");
    }
    let skipped = [
        "// unsafe { call() }",
        "/* unsafe\n unsafe */ let x = 1;",
        "let s = \"unsafe { call() }\";",
        "let s = r#\"unsafe \" fn\"#;",
        "let s = br\"unsafe\";",
        "let s = \"escaped \\\" unsafe\";",
        "let c = '\\''; let d = '\"'; // unsafe",
        "fn f<'a>(x: &'a str) -> &'a str { x }",
        "#![deny(unsafe_code)]",
        "#![forbid(unsafe_code)]",
        "bail!(\"unsafe plugin version path\");",
        "let unsafe_path = 1;",
    ];
    for line in skipped {
        assert!(unsafe_lines(line).is_empty(), "{line}");
    }
    assert_eq!(
        unsafe_lines("let s = \"a\nb\";\n// c\nunsafe {}\n"),
        vec![4],
        "line numbers survive literals and comments"
    );
}
