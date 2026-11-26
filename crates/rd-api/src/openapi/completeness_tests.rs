//! What the document says about every operation, and what it still leaves out (RD-1480-09).

use std::collections::BTreeSet;

use axum::http::Method;
use rd_api_core::scope_policy::{Requirement, requirement};
use rd_core::EventKind;
use serde_json::Value;

use super::{access, events, refusals};

fn document() -> Value {
    serde_json::to_value(super::document()).expect("serialise the document")
}

/// Every operation: path, method and the operation itself.
fn operations(document: &Value) -> Vec<(String, Method, &Value)> {
    let mut found = Vec::new();
    for (path, item) in document["paths"].as_object().expect("paths") {
        for (method, operation) in item.as_object().expect("path item") {
            if let Ok(method) = Method::from_bytes(method.to_uppercase().as_bytes())
                && operation.get("responses").is_some()
            {
                found.push((path.clone(), method, operation));
            }
        }
    }
    assert!(found.len() > 480, "only {} operations found", found.len());
    found
}

/// A response, through the reference to a shared one.
fn resolved<'a>(document: &'a Value, response: &'a Value) -> &'a Value {
    match response["$ref"].as_str() {
        Some(reference) => {
            let name = reference.rsplit('/').next().unwrap_or_default();
            &document["components"]["responses"][name]
        }
        None => response,
    }
}

/// Every operation says which credential it takes and which permission area it costs, as the
/// table the service decides with does.
#[test]
fn every_operation_names_its_credential_and_its_area() {
    let document = document();
    let schemes = document["components"]["securitySchemes"]
        .as_object()
        .expect("security schemes");
    for (path, method, operation) in operations(&document) {
        let security = operation["security"]
            .as_array()
            .unwrap_or_else(|| panic!("{method} {path} says nothing about credentials"));
        let area = operation[access::REQUIRED_SCOPE].as_str();
        match requirement(&path, &method).expect("a policy entry") {
            Requirement::Public => {
                assert!(security.is_empty(), "{method} {path}");
                assert_eq!(area, None, "{method} {path}");
            }
            Requirement::Scope(scope) => {
                assert_eq!(area, Some(scope.as_str()), "{method} {path}");
                assert!(!security.is_empty(), "{method} {path}");
                for alternative in security {
                    for (scheme, areas) in alternative.as_object().expect("requirement") {
                        assert!(schemes.contains_key(scheme), "{method} {path}: {scheme}");
                        assert_eq!(areas[0], scope.as_str(), "{method} {path}");
                    }
                }
                let capture = access::is_capture(scope);
                assert_eq!(
                    security.iter().any(|one| one.get("captureToken").is_some()),
                    capture,
                    "{method} {path}"
                );
                assert!(
                    operation["responses"].get("401").is_some(),
                    "{method} {path}"
                );
                assert!(
                    capture || operation["responses"].get("429").is_some(),
                    "{method} {path}"
                );
            }
        }
    }
}

/// A refusal has one body, whoever declared it.
#[test]
fn every_refusal_is_the_error_body() {
    let document = document();
    let mut refusals = 0;
    for (path, method, operation) in operations(&document) {
        for (status, response) in operation["responses"].as_object().expect("responses") {
            // The probe answers with its own body whether the service works or not.
            if !refusals::is_refusal(status) || (path == "/api/v1/health" && status == "503") {
                continue;
            }
            refusals += 1;
            let response = resolved(&document, response);
            assert_eq!(
                response["content"]["application/json"]["schema"]["$ref"],
                "#/components/schemas/ErrorBody",
                "{method} {path} {status}"
            );
            assert!(
                response["description"]
                    .as_str()
                    .is_some_and(|text| !text.is_empty()),
                "{method} {path} {status}"
            );
        }
    }
    assert!(refusals > 1_200, "only {refusals} refusals found");
}

/// What reads a body, a path or a query string can fail to read it.
#[test]
fn an_operation_that_reads_something_declares_the_refusal_of_it() {
    let document = document();
    for (path, method, operation) in operations(&document) {
        let responses = &operation["responses"];
        let reads_a_parameter = operation["parameters"]
            .as_array()
            .is_some_and(|parameters| {
                parameters
                    .iter()
                    .any(|one| one["in"] == "path" || one["in"] == "query")
            });
        let content = operation["requestBody"]["content"].as_object();
        if content.is_some() || reads_a_parameter {
            assert!(responses.get("400").is_some(), "{method} {path}");
        }
        if let Some(content) = content
            && content.len() == 1
            && content.contains_key("application/json")
        {
            for status in ["413", "415"] {
                assert!(responses.get(status).is_some(), "{method} {path} {status}");
            }
        }
    }
}

/// The streams are in the document with every event's name and area, and with both markers.
#[test]
fn the_event_streams_name_every_event() {
    let document = document();
    let declared: BTreeSet<String> = document["components"]["schemas"]["EventKind"]["enum"]
        .as_array()
        .expect("the kinds")
        .iter()
        .filter_map(|kind| kind.as_str().map(str::to_owned))
        .collect();
    let described: BTreeSet<String> = events::KINDS
        .iter()
        .filter_map(|kind| serde_json::to_value(kind).ok()?.as_str().map(str::to_owned))
        .collect();
    assert_eq!(described, declared, "events::KINDS is not every kind");

    let text = |path: &str| {
        document["paths"][path]["get"]["description"]
            .as_str()
            .expect("a description")
            .to_owned()
    };
    let web = text("/api/v1/events");
    for kind in events::KINDS {
        let name = crate::event_stream::event_name(kind);
        assert!(web.contains(&format!("| `{name}` | `")), "{name}");
    }
    let capture = text("/api/v1/capture/events");
    for carried in [EventKind::CollectorIntake, EventKind::CaptchaChanged] {
        let name = crate::event_stream::event_name(&carried);
        assert!(capture.contains(name), "{name}");
    }
    for stream in [&web, &capture] {
        for marker in [
            crate::event_stream::LAGGED_EVENT,
            crate::event_stream::EXPIRED_EVENT,
        ] {
            assert!(stream.contains(marker), "{marker}");
        }
    }
    for path in ["/api/v1/events", "/api/v1/capture/events"] {
        assert!(
            document["paths"][path]["get"]["responses"]["200"]["content"]
                .get("text/event-stream")
                .is_some(),
            "{path}"
        );
    }
}

/// Every tag is described and none is described for nothing; the document introduces itself.
#[test]
fn every_tag_is_described() {
    let document = document();
    let described: BTreeSet<&str> = document["tags"]
        .as_array()
        .expect("tags")
        .iter()
        .filter(|tag| {
            tag["description"]
                .as_str()
                .is_some_and(|text| !text.is_empty())
        })
        .filter_map(|tag| tag["name"].as_str())
        .collect();
    let used: BTreeSet<&str> = operations(&document)
        .into_iter()
        .flat_map(|(path, method, operation)| {
            let tags = operation["tags"]
                .as_array()
                .unwrap_or_else(|| panic!("{method} {path} has no tag"));
            tags.iter().filter_map(Value::as_str)
        })
        .collect();
    assert_eq!(described, used);
    assert!(
        document["info"]["description"]
            .as_str()
            .is_some_and(|text| text.contains("ErrorBody"))
    );
}

/// What the document still leaves without a text. Each number may only shrink: a new operation,
/// parameter or field comes with its text, and whoever writes one lowers the number.
#[test]
fn what_is_left_undescribed_only_shrinks() {
    const SUMMARIES: usize = 275;
    const DESCRIPTIONS: usize = 421;
    const PARAMETERS: usize = 180;
    const FIELDS: usize = 1_875;
    const DATES: usize = 18;

    let document = document();
    let operations = operations(&document);
    let without = |key: &str| {
        operations
            .iter()
            .filter(|(_, _, operation)| operation.get(key).is_none())
            .count()
    };
    let parameters = operations
        .iter()
        .filter_map(|(_, _, operation)| operation["parameters"].as_array())
        .flatten()
        .filter(|parameter| parameter.get("description").is_none())
        .count();
    let fields: Vec<(&String, &Value)> = document["components"]["schemas"]
        .as_object()
        .expect("schemas")
        .values()
        .filter_map(|schema| schema["properties"].as_object())
        .flatten()
        .collect();
    let undescribed = fields
        .iter()
        .filter(|(_, field)| field.get("description").is_none())
        .count();
    let dates = fields
        .iter()
        .filter(|(name, field)| {
            name.ends_with("_at") && is_string(field) && field["format"] != "date-time"
        })
        .count();

    for (what, found, limit) in [
        (
            "operations without a summary",
            without("summary"),
            SUMMARIES,
        ),
        (
            "operations without a description",
            without("description"),
            DESCRIPTIONS,
        ),
        ("parameters without a description", parameters, PARAMETERS),
        ("fields without a description", undescribed, FIELDS),
        ("`*_at` strings that are no date-time", dates, DATES),
    ] {
        assert!(
            found <= limit,
            "{found} {what}, {limit} allowed: write a doc comment for what is new"
        );
    }
}

/// Whether a field is a string, or a string or `null`.
fn is_string(field: &Value) -> bool {
    field["type"] == "string"
        || field["type"]
            .as_array()
            .is_some_and(|types| types.iter().any(|one| one == "string"))
}
