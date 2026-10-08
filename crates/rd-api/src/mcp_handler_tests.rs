//! Every MCP tool calls the handler of the route `TOOL_POLICY` prices it by (RD-1190-21).
//!
//! The policy names a route and the tool pays that route's price, but nothing held the tool's
//! body to the route: a tool priced as `GET /api/v1/downloads` could call any function at all and
//! its price would still read right. This reads the tools' sources and the OpenAPI document --
//! whose operation ids are the handlers' names -- and holds each tool's body to its route's
//! handler. The few tools that reach the same work another way share a named function with that
//! handler instead, and the handler is held to calling it too; one reads a row of the table its
//! list route reads, and says so.

use std::path::{Path, PathBuf};

/// Tools that do not call their route's handler, and the function both of them call instead.
const SHARED: &[(&str, &str)] = &[
    ("add_downloads", "create_download_as("),
    ("check_links", "link_check.check("),
    ("control_downloads", "apply_download_action_to("),
    ("create_notification_target", "save_target("),
    ("enqueue_collector", "collector_enqueue::enqueue_package("),
    ("get_status_summary", "summarize_downloads("),
    ("update_notification_target", "save_target("),
    ("update_settings", "save_settings("),
];

/// Tools that read the store directly, and why the route that prices them is still the right one.
const READS_THE_STORE: &[(&str, &str)] = &[(
    "get_download",
    "one row, by id, of the table GET /api/v1/downloads lists; that route has no single-row form",
)];

/// The crates directory, read at run time like every other source-reading test.
fn crates_dir() -> PathBuf {
    let manifest = std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    Path::new(&manifest)
        .parent()
        .expect("the crates directory")
        .to_path_buf()
}

/// Every `.rs` file below `dir`.
fn sources(dir: &Path, found: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read a source directory") {
        let path = entry.expect("a directory entry").path();
        if path.is_dir() {
            sources(&path, found);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            found.push(path);
        }
    }
}

/// The text from `pub async fn <name>(` to the first `end` marker after it, for every definition.
fn bodies_of(text: &str, name: &str, ends: &[&str]) -> Vec<String> {
    let head = format!("pub async fn {name}(");
    text.match_indices(&head)
        .map(|(start, _)| {
            let rest = &text[start + head.len()..];
            let end = ends
                .iter()
                .filter_map(|marker| rest.find(marker))
                .min()
                .unwrap_or(rest.len());
            text[start..start + head.len() + end].to_owned()
        })
        .collect()
}

/// The text of every tool module, read once.
fn tool_modules() -> Vec<String> {
    let mut files = Vec::new();
    sources(&crates_dir().join("rd-api-mcp/src"), &mut files);
    files
        .iter()
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("tools_"))
        })
        .map(|path| std::fs::read_to_string(path).expect("read a tool module"))
        .collect()
}

/// The body of one tool, up to the next tool's attribute.
fn tool_body(modules: &[String], tool: &str) -> String {
    modules
        .iter()
        .flat_map(|text| bodies_of(text, tool, &["#[tool("]))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The body of one handler in the HTTP crates the tools call into.
fn handler_body(handler: &str) -> String {
    let mut files = Vec::new();
    for area in ["access", "admin", "compat", "core", "intake", "queue"] {
        sources(&crates_dir().join(format!("rd-api-{area}/src")), &mut files);
    }
    files
        .iter()
        .flat_map(|path| {
            let text = std::fs::read_to_string(path).expect("read a handler module");
            bodies_of(
                &text,
                handler,
                &["\n#[utoipa::path", "\npub async fn ", "\npub fn "],
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Whether `body` calls `function` (or its `_inner` form): the name as a whole word, then `(`.
fn calls(body: &str, function: &str) -> bool {
    body.match_indices(function).any(|(at, _)| {
        let before = body[..at].chars().next_back();
        if before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_') {
            return false;
        }
        let after = &body[at + function.len()..];
        let after = after.strip_prefix("_inner").unwrap_or(after).trim_start();
        after.starts_with('(') || after.starts_with("::<")
    })
}

/// The handler behind one route: its operation id.
fn operation_id(document: &serde_json::Value, path: &str, method: &str) -> String {
    document["paths"][path][method.to_ascii_lowercase()]["operationId"]
        .as_str()
        .unwrap_or_else(|| panic!("{method} {path} has no operation id"))
        .to_owned()
}

#[test]
fn every_tool_calls_the_handler_of_the_route_it_is_priced_by() {
    let document = serde_json::to_value(crate::openapi_document()).expect("serialise");
    let modules = tool_modules();
    let mut failures = Vec::new();
    for entry in rd_api_mcp::TOOL_POLICY {
        let handler = operation_id(&document, entry.path, entry.method.as_str());
        let body = tool_body(&modules, entry.tool);
        assert!(!body.is_empty(), "no source for the tool {}", entry.tool);
        if READS_THE_STORE.iter().any(|(tool, _)| *tool == entry.tool) {
            continue;
        }
        if let Some((_, shared)) = SHARED.iter().find(|(tool, _)| *tool == entry.tool) {
            if calls(&body, &handler) {
                failures.push(format!(
                    "{} calls {handler} now: drop it from SHARED",
                    entry.tool
                ));
            }
            if !body.contains(shared) {
                failures.push(format!("{} no longer calls {shared}", entry.tool));
            }
            if !handler_body(&handler).contains(shared) {
                failures.push(format!(
                    "{handler} no longer calls {shared}, as {} does",
                    entry.tool
                ));
            }
        } else if !calls(&body, &handler) {
            failures.push(format!(
                "{} is priced as {} {} but does not call its handler {handler}",
                entry.tool, entry.method, entry.path
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn every_exception_names_a_priced_tool() {
    for tool in SHARED
        .iter()
        .map(|(tool, _)| tool)
        .chain(READS_THE_STORE.iter().map(|(tool, _)| tool))
    {
        assert!(
            rd_api_mcp::TOOL_POLICY
                .iter()
                .any(|entry| entry.tool == *tool),
            "{tool} is no tool"
        );
    }
}

#[test]
fn a_call_is_the_whole_name_followed_by_its_arguments() {
    assert!(calls(
        "crate::x::update_package(State(s))",
        "update_package"
    ));
    assert!(calls("x::update_package_inner(&s)", "update_package"));
    assert!(calls("x::update_package ::<T>(a)", "update_package"));
    assert!(!calls("x::bulk_update_package(a)", "update_package"));
    assert!(!calls("x::update_packages(a)", "update_package"));
    assert!(!calls("// update_package is not called", "update_package"));
}
