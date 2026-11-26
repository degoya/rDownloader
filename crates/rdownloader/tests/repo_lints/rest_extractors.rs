//! The REST surface reads its requests through `rd_api_core::extract`, never through axum's own
//! `Json`, `Path` and `Query` (RD-1480-08).
//!
//! axum's refuse a malformed request with plain text; every REST error is an `ErrorBody` with a
//! stable code. A handler written the way axum's documentation shows compiles, works and answers
//! a malformed request in the wrong format -- which is how 207 of them did.
//!
//! `rd-api-compat` is out: its adapters answer in the format of the program they stand in for.

use std::path::{Path, PathBuf};

use crate::workspace_root;

/// The crates whose handlers, and the tools that call them, use the coded extractors.
const CRATES: &[&str] = &[
    "rd-api",
    "rd-api-access",
    "rd-api-admin",
    "rd-api-core",
    "rd-api-intake",
    "rd-api-mcp",
    "rd-api-queue",
];

/// The files that name axum's own, each with its reason.
const EXEMPT: &[(&str, &str)] = &[
    (
        "crates/rd-api-core/src/extract.rs",
        "wraps them: this is where the codes are given",
    ),
    (
        "crates/rd-api-core/src/container_upload.rs",
        "its JSON document is refused with a status of its own",
    ),
];

fn rust_files(directory: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            out.push(path);
        }
    }
}

/// Whether `source` names one of the three as axum's.
///
/// In a `use axum::...;` any whole word of theirs counts, aliased or not: nothing else of axum's
/// is called `Json`, `Path` or `Query`. Outside one the full path has to be spelled out.
fn names_axum_extractor(source: &str) -> bool {
    let code: String = source
        .lines()
        .map(|line| line.split("//").next().unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n");
    let imported = code.match_indices("use axum::").any(|(start, _)| {
        let statement = code[start..].split(';').next().unwrap_or_default();
        statement
            .split(|character: char| !is_word(character))
            .any(|word| matches!(word, "Json" | "Path" | "Query"))
    });
    imported
        || ["axum::Json", "axum::extract::Path", "axum::extract::Query"]
            .iter()
            .any(|path| {
                code.match_indices(path).any(|(start, _)| {
                    !code[..start].ends_with(is_word)
                        && !code[start + path.len()..].starts_with(is_word)
                })
            })
}

fn is_word(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

#[test]
fn the_rest_surface_uses_the_coded_extractors() {
    let root = workspace_root();
    let mut files = Vec::new();
    for name in CRATES {
        rust_files(&root.join("crates").join(name).join("src"), &mut files);
    }
    assert!(files.len() > 100, "only {} sources found", files.len());

    let offenders: Vec<String> = files
        .iter()
        .filter_map(|path| {
            let name = path
                .strip_prefix(&root)
                .unwrap_or(path)
                .display()
                .to_string()
                .replace('\\', "/");
            let exempt = EXEMPT.iter().any(|(file, _)| *file == name);
            let source = std::fs::read_to_string(path).expect("source");
            match (names_axum_extractor(&source), exempt) {
                (true, false) => Some(format!("{name}: use rd_api_core::extract instead")),
                (false, true) => Some(format!("{name}: exempt, and names none any more")),
                _ => None,
            }
        })
        .collect();
    assert!(
        offenders.is_empty(),
        "axum's Json, Path and Query refuse a malformed request with plain text instead of a \
         coded ErrorBody:\n  {}",
        offenders.join("\n  ")
    );
}

#[test]
fn the_check_tells_the_extractors_from_their_neighbours() {
    for named in [
        "use axum::Json;",
        "use axum::{Json, extract::State};",
        "use axum::{\n    Extension,\n    extract::{Path as AxumPath, State},\n};",
        "use axum::extract::{Query, State};",
        "use axum::{extract::Path as AxumPath, extract::State};",
        "    axum::extract::Query(query): axum::extract::Query<Window>,",
        "axum::extract::Query::<Window>::try_from_uri(&uri)",
        "let axum::Json(body) = answer;",
    ] {
        assert!(names_axum_extractor(named), "{named}");
    }
    for other in [
        "use axum::{extract::State, http::StatusCode};",
        "use axum::extract::{MatchedPath, Multipart, State};",
        "use axum::extract::rejection::JsonRejection;",
        "use rd_api_core::extract::{Json, Path, Query};",
        "// axum::Json would answer in plain text",
        "use std::path::Path;",
        "use axum::response::IntoResponse;\nuse std::path::{Path, PathBuf};",
    ] {
        assert!(!names_axum_extractor(other), "{other}");
    }
}
