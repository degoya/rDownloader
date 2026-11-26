//! Every path of the OpenAPI document has a test, or a written reason why not (RD-1480-25).
//!
//! The audit of 2026-11-25 (TST-7) found paths no test names, the revocation of a capture agent
//! among them. A path counts as tested when a Rust test source does one of three things:
//!
//! - it names the path in a string, segment for segment, where a placeholder of either side --
//!   `{id}` in the document, `{id}` or `{}` in a `format!` -- stands for any segment; a constant
//!   holding a path is followed within its file (`"{PAGE}/test"`);
//! - it calls the handler, whose name is the operation id (`handlers::name(`);
//! - it is an MCP test that calls the tool [`rd_api_mcp::TOOL_POLICY`] prices by the path, since
//!   the tool's body is the handler's (`mcp_handler_tests`).
//!
//! What walks every route for one property -- the scopes, the methods, the origins -- names each
//! path without saying what it does, so those sources do not count. Whether a test is a good one
//! is not for a lint to say.

use std::path::{Path, PathBuf};

/// Paths without a test, each with the reason. Empty, and meant to stay so.
const EXEMPT: &[(&str, &str)] = &[];

const PREFIX: &str = "/api/v1/";

/// Sources that walk the routes or name them in a table, by a part of their path.
const WALKERS: &[&str] = &[
    "scope_matrix",
    "scope_policy_tests",
    "mcp_coverage",
    "mcp_handler_tests",
    "untested_routes_tests",
    "cross_site",
    "wrong_method",
    "public_surface",
    "repo_lints",
];

/// Every `.rs` file below `dir`.
fn sources(dir: &Path, found: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read a directory") {
        let path = entry.expect("a directory entry").path();
        if path.is_dir() {
            sources(&path, found);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            found.push(path);
        }
    }
}

/// The test sources of every crate, by their path below `crates/` with `/` on every platform,
/// without their comment lines: a path named in prose is not asked for.
fn test_sources() -> Vec<(String, String)> {
    let manifest = std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let crates = Path::new(&manifest).parent().expect("the crates directory");
    let mut files = Vec::new();
    sources(crates, &mut files);
    files
        .into_iter()
        .filter_map(|file| {
            let relative = file
                .strip_prefix(crates)
                .expect("below crates")
                .components()
                .map(|part| part.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>();
            let name = relative.last().expect("a file name").clone();
            let is_test = relative.iter().any(|part| part == "tests")
                || name == "tests.rs"
                || name.ends_with("_tests.rs");
            let path = relative.join("/");
            if !is_test || WALKERS.iter().any(|walker| path.contains(walker)) {
                return None;
            }
            let text = std::fs::read_to_string(&file).expect("read a test source");
            let code = text
                .lines()
                .filter(|line| !line.trim_start().starts_with("//"))
                .collect::<Vec<_>>()
                .join("\n");
            Some((path, code))
        })
        .collect()
}

/// `text` up to where a path in a string ends: the quote, a query, a fragment, an escape.
fn path_end(text: &str) -> &str {
    let end = text
        .find(|c: char| matches!(c, '"' | '?' | '#' | '`' | ')' | '\\') || c.is_whitespace())
        .unwrap_or(text.len());
    text[..end].trim_end_matches('/')
}

/// The constants of a source that hold a path: name and value.
fn path_constants(code: &str) -> Vec<(&str, &str)> {
    code.match_indices("const ")
        .filter_map(|(at, word)| {
            let (name, rest) = code[at + word.len()..].split_once(':')?;
            let value = rest.split_once(';')?.0.split('"').nth(1)?;
            let is_name = name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
            (is_name && value.starts_with(PREFIX)).then_some((name, value))
        })
        .collect()
}

/// Every path a source names, constants followed.
fn named_paths(code: &str) -> Vec<String> {
    let mut paths: Vec<String> = code
        .match_indices(PREFIX)
        .map(|(at, _)| path_end(&code[at..]).to_owned())
        .collect();
    for (name, value) in path_constants(code) {
        let opening = format!("\"{{{name}}}");
        paths.extend(
            code.match_indices(&opening)
                .map(|(at, _)| format!("{value}{}", path_end(&code[at + opening.len()..]))),
        );
    }
    paths
}

/// Whether a path a test names is the document's `template`.
fn names(template: &str, named: &str) -> bool {
    let (Some(template), Some(named)) = (template.strip_prefix(PREFIX), named.strip_prefix(PREFIX))
    else {
        return false;
    };
    let template: Vec<&str> = template.split('/').collect();
    let named: Vec<&str> = named.split('/').collect();
    let mut literal = 0;
    template.len() == named.len()
        && template.iter().zip(&named).all(|(expected, segment)| {
            if expected.starts_with('{') || segment.contains('{') {
                return true;
            }
            literal += 1;
            expected == segment
        })
        // `/api/v1/{area}` in a test names no path in particular.
        && literal > 0
}

/// The operation ids behind one path of the document.
fn operation_ids(operations: &serde_json::Value) -> Vec<&str> {
    operations
        .as_object()
        .expect("the operations of a path")
        .values()
        .filter_map(|operation| operation["operationId"].as_str())
        .collect()
}

#[test]
fn every_path_has_a_test_or_a_reason() {
    let document = serde_json::to_value(crate::openapi_document()).expect("serialise");
    let sources = test_sources();
    let named: Vec<String> = sources
        .iter()
        .flat_map(|(_, code)| named_paths(code))
        .collect();
    let mcp: String = sources
        .iter()
        .filter(|(path, _)| path.starts_with("rd-api/tests/mcp/"))
        .map(|(_, code)| code.as_str())
        .collect();
    let mut untested = Vec::new();
    let mut needless = Vec::new();
    for (path, operations) in document["paths"].as_object().expect("paths") {
        let tested = named.iter().any(|name| names(path, name))
            || operation_ids(operations).iter().any(|id| {
                let call = format!("::{id}(");
                sources.iter().any(|(_, code)| code.contains(&call))
            })
            || rd_api_mcp::TOOL_POLICY
                .iter()
                .any(|entry| entry.path == path && mcp.contains(&format!("\"{}\"", entry.tool)));
        let exempt = EXEMPT.iter().any(|(exempt, _)| exempt == path);
        if !tested && !exempt {
            untested.push(path.clone());
        } else if tested && exempt {
            needless.push(path.clone());
        }
    }
    assert!(
        untested.is_empty(),
        "no test names these paths, calls their handler or drives their MCP tool; write one (a \
         suite of crates/rd-api/tests), or put the path into EXEMPT with the reason:\n  {}",
        untested.join("\n  ")
    );
    assert!(
        needless.is_empty(),
        "these paths have a test now; take them out of EXEMPT:\n  {}",
        needless.join("\n  ")
    );
}

#[test]
fn every_exemption_names_a_path_and_a_reason() {
    let document = serde_json::to_value(crate::openapi_document()).expect("serialise");
    for (path, reason) in EXEMPT {
        assert!(
            document["paths"].get(path).is_some(),
            "{path} is no path of the document"
        );
        assert!(!reason.trim().is_empty(), "{path} gives no reason");
    }
}

/// The reader itself: blind, it would pass any document.
#[test]
fn a_path_is_named_segment_for_segment() {
    let agent = "/api/v1/capture/agents/{id}";
    assert!(names(agent, "/api/v1/capture/agents/{lost_id}"));
    assert!(names(agent, "/api/v1/capture/agents/{}"));
    assert!(names(agent, "/api/v1/capture/agents/nothing"));
    assert!(names("/api/v1/streams/export", "/api/v1/{area}/export"));
    assert!(!names(agent, "/api/v1/capture/agents"));
    assert!(!names(agent, "/api/v1/capture/agents/{id}/more"));
    assert!(!names(agent, "/api/v1/capture/tokens/{id}"));
    assert!(!names("/api/v1/about", "/api/v1/{path}"));

    let code = "const PAGE: &str = \"/api/v1/streams\";\n\
                get(&format!(\"{PAGE}/{id}?page=1\"));\n\
                get(\"/api/v1/about\");";
    let paths = named_paths(code);
    for expected in ["/api/v1/streams", "/api/v1/streams/{id}", "/api/v1/about"] {
        assert!(paths.iter().any(|path| path == expected), "{paths:?}");
    }
}
