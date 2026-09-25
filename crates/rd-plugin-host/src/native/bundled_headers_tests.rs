//! Every request header a bundled plugin can set passes the host's `allowed_header` (RD-120-60).
//!
//! The host refuses any other header with "Resolver HTTP header is not allowed", and it does
//! so below every contract test: those answer at the `ResolverHost` boundary, so a plugin could
//! ask for a header the host would never send and still pass all of them. Telegram's
//! `Content-Length`, ntfy's `Title`/`Priority`/`Tags`, Box's `boxapi` and KrakenFiles' `hash`
//! did exactly that, and each of those deliveries or downloads failed from the day it shipped.
//!
//! The real host path cannot reach every branch — a header added only for a password-protected
//! link or a free-download POST behind a captcha — so this reads the sources instead. It
//! knows the four ways a plugin names a header, and a name it cannot resolve to a literal is a
//! failure rather than a pass: a guard that skipped what it could not read would pass exactly
//! the header nobody looked at.

use std::path::{Path, PathBuf};

use super::expand::allowed_header;

/// Places that forward a header name they did not choose. Each is an adapter between two
/// representations, and the name it forwards is checked where it was written.
const PASS_THROUGH: &[(&str, &str)] = &[
    // `plugin_common::Header` to the WIT `request-header`: the guest bridge every component
    // resolver goes through. The names come from `with_header` calls, which are read.
    ("guest/src/lib.rs", "value.name"),
    // The `with_header` builder itself; its callers are read.
    ("common/src/types.rs", "name"),
];

/// Whether `offset` starts a word, so `HttpRequest {` is not found inside `HostHttpRequest {`.
fn word_start(source: &str, offset: usize) -> bool {
    source[..offset]
        .chars()
        .next_back()
        .is_none_or(|character| !(character.is_alphanumeric() || character == '_'))
}

fn plugins_root() -> PathBuf {
    // At run time, not `env!`: the binary may outlive the checkout that built it.
    let manifest_dir = std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest_dir).join("../../plugins")
}

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|name| name == "target") {
                continue;
            }
            rust_sources(&path, out);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            out.push(path);
        }
    }
}

/// One header name as a plugin wrote it, with where.
#[derive(Debug)]
struct Found {
    at: String,
    expression: String,
    name: Option<String>,
}

/// The expression that ends at the first top-level `,` or closing bracket.
fn argument(text: &str) -> String {
    let mut depth = 0_i32;
    let mut in_string = false;
    let mut escaped = false;
    for (index, character) in text.char_indices() {
        if in_string {
            match (escaped, character) {
                (true, _) => escaped = false,
                (false, '\\') => escaped = true,
                (false, '"') => in_string = false,
                _ => {}
            }
            continue;
        }
        match character {
            '"' => in_string = true,
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' if depth == 0 => return text[..index].trim().to_owned(),
            ')' | ']' | '}' => depth -= 1,
            ',' if depth == 0 => return text[..index].trim().to_owned(),
            _ => {}
        }
    }
    text.trim().to_owned()
}

/// The literal a name expression stands for, looking up a `const` in the same file.
fn resolve(expression: &str, source: &str) -> Option<String> {
    let expression = expression.trim().trim_end_matches(".to_owned()");
    let expression = expression.trim_end_matches(".to_string()");
    if let Some(literal) = expression
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
    {
        return Some(literal.to_owned());
    }
    // `NAME` for `const NAME: &str = "…";`, `NAME.0` for `const NAME: (&str, &str) = ("…", …);`.
    let (constant, tuple) = match expression.strip_suffix(".0") {
        Some(constant) => (constant, true),
        None => (expression, false),
    };
    if constant.is_empty()
        || !constant
            .chars()
            .all(|character| character.is_ascii_uppercase() || character == '_')
    {
        return None;
    }
    let declaration = source
        .lines()
        .find(|line| line.trim_start().starts_with(&format!("const {constant}:")))?;
    let value = declaration.split_once('=')?.1.trim();
    let value = if tuple {
        value.strip_prefix('(')?
    } else {
        value
    };
    let value = value.strip_prefix('"')?;
    Some(value[..value.find('"')?].to_owned())
}

fn scan(relative: &str, source: &str, found: &mut Vec<Found>) {
    let line_of = |offset: usize| source[..offset].lines().count().max(1);
    let mut record = |offset: usize, expression: String| {
        found.push(Found {
            at: format!("{relative}:{}", line_of(offset)),
            name: resolve(&expression, source),
            expression,
        });
    };
    // `.with_header(name, value)` and `headers.push(Header::new(name, value))` — the
    // `plugin_common` builders every resolver uses.
    for pattern in ["with_header(", "headers.push(Header::new("] {
        for (offset, _) in source.match_indices(pattern) {
            let rest = &source[offset + pattern.len()..];
            let expression = argument(rest);
            // The builder's own definition, not a call.
            if expression.starts_with("mut self") || expression.starts_with("self") {
                continue;
            }
            record(offset, expression);
        }
    }
    // `RequestHeader { name: …, value_template: … }` — the WIT record the components build.
    for (offset, _) in source.match_indices("RequestHeader {") {
        if !word_start(source, offset) {
            continue;
        }
        let rest = &source[offset + "RequestHeader {".len()..];
        let Some(field) = rest.find("name:") else {
            continue;
        };
        // Only this literal's own field, not one further down the file.
        if rest[..field].contains('}') {
            continue;
        }
        record(offset, argument(&rest[field + "name:".len()..]));
    }
    // `HttpRequest { …, headers: …, … }` built by hand: only the empty list is readable here,
    // and the builders above are how a header is added to it.
    for (offset, _) in source.match_indices("HttpRequest {") {
        let before = source[..offset].trim_end();
        if !word_start(source, offset)
            || ["struct", "impl", "->", "for"]
                .iter()
                .any(|keyword| before.ends_with(keyword))
        {
            continue;
        }
        let rest = &source[offset + "HttpRequest {".len()..];
        let Some(field) = rest.find("headers:") else {
            continue;
        };
        let expression = argument(&rest[field + "headers:".len()..]);
        if expression != "Vec::new()" && expression != "vec![]" {
            found.push(Found {
                at: format!("{relative}:{}", line_of(offset)),
                expression: format!("headers: {expression}"),
                name: None,
            });
        }
    }
}

#[test]
fn every_header_a_bundled_plugin_can_set_is_one_the_host_sends() {
    let root = plugins_root();
    let mut files = Vec::new();
    rust_sources(&root, &mut files);
    files.sort();
    let mut found = Vec::new();
    for file in &files {
        let source = std::fs::read_to_string(file).expect("read plugin source");
        // `cargo component` leaves wit-bindgen's output in `src/bindings.rs` (ignored by git):
        // the contract's own types, not a request any plugin makes.
        if source.starts_with("// Generated by `wit-bindgen`") {
            continue;
        }
        let relative = file
            .strip_prefix(&root)
            .expect("under plugins/")
            .to_string_lossy()
            .replace('\\', "/");
        scan(&relative, &source, &mut found);
    }

    let mut refused = Vec::new();
    let mut unreadable = Vec::new();
    for entry in &found {
        match &entry.name {
            Some(name) if allowed_header(name) => {}
            Some(name) => refused.push(format!("{} sets `{name}`", entry.at)),
            None if PASS_THROUGH.iter().any(|(file, expression)| {
                entry.at.starts_with(file) && entry.expression == *expression
            }) => {}
            None => unreadable.push(format!("{} names `{}`", entry.at, entry.expression)),
        }
    }
    assert!(
        refused.is_empty() && unreadable.is_empty(),
        "headers the host refuses with `plugin.http_header_not_allowed`:\n  {}\n\
         header names this guard cannot read (write a literal or a same-file `const`):\n  {}",
        refused.join("\n  "),
        unreadable.join("\n  "),
    );

    // The scan found what it is meant to find. Without this a pattern that stopped matching —
    // a renamed builder, a moved directory — would pass with nothing checked.
    let named = |header: &str, plugin: &str| {
        found.iter().any(|entry| {
            entry.at.starts_with(plugin)
                && entry
                    .name
                    .as_deref()
                    .is_some_and(|name| name.eq_ignore_ascii_case(header))
        })
    };
    for (header, plugin) in [
        ("Content-Type", "discord-notifier/"),
        ("Authorization", "ntfy-notifier/"),
        ("Referer", "krakenfiles/"),
        ("X-Requested-With", "rapidgator/"),
        ("hash", "krakenfiles/"),
        ("boxapi", "box/"),
        ("boxapi", "box-crawler/"),
        ("Depth", "webdav-storage/"),
    ] {
        assert!(
            named(header, plugin),
            "{plugin} no longer seen setting {header}"
        );
    }
    assert!(
        found.len() > 100,
        "only {} header names found under {}",
        found.len(),
        root.display()
    );
}

#[test]
fn the_reader_resolves_literals_and_same_file_constants() {
    let source = "const XRW: (&str, &str) = (\"X-Requested-With\", \"XMLHttpRequest\");\n\
                  const NAME: &str = \"Accept\";\n";
    assert_eq!(resolve("\"Range\"", source).as_deref(), Some("Range"));
    assert_eq!(
        resolve("\"Title\".to_owned()", source).as_deref(),
        Some("Title")
    );
    assert_eq!(
        resolve("XRW.0", source).as_deref(),
        Some("X-Requested-With")
    );
    assert_eq!(resolve("NAME", source).as_deref(), Some("Accept"));
    assert_eq!(resolve("name", source), None);
    assert_eq!(resolve("MISSING", source), None);
    assert_eq!(
        argument("\"a,b\", format!(\"{x}, {y}\"))"),
        "\"a,b\"".to_owned()
    );
}
