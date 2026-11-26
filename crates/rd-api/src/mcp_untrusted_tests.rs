//! Every MCP tool whose route answers third-party text is marked as such (RD-1480-06).
//!
//! `rd_api_mcp::UNTRUSTED_TEXT_TOOLS` is a list by name, and a tool added without an entry used
//! to answer unmarked. What a tool answers is read from the OpenAPI document: the types its
//! routes answer with, followed through their references.

use std::collections::BTreeSet;

use serde_json::Value;

/// The answer types that carry a name, title or message somebody other than the person wrote:
/// a package, file or release name, a provider's, tracker's, plugin's or indexer's message, a log
/// line. Sorted.
const THIRD_PARTY_TYPES: &[&str] = &[
    "AuditRecordResponse",
    "CandidateStayed",
    "CollectorPackage",
    "CollectorPickResponse",
    "CollisionPromptResponse",
    "Delivery",
    "DownloadFile",
    "DownloadPackage",
    "DownloadSourceView",
    "HistoryDuplicate",
    "HistoryEntry",
    "IndexerSearchHit",
    "LinkCandidate",
    "LogRecordResponse",
    "NzbFileStatus",
    "NzbImport",
    "PackageClearSkip",
    "PluginExecutionResponse",
    "PostprocessQueueEntry",
    "PostprocessStep",
    "QueueSearchResponse",
    "QueueStopMarkResponse",
    "RemoteJob",
    "RemoteJobClearItem",
    "ReresolveResult",
    "Run",
    "SourceDuplicate",
    "StorageOperationResponse",
    "StreamChannel",
    "Subscription",
    "SubscriptionItem",
    "SubscriptionRun",
    "TestSiteRuleResponse",
    "TorrentDetailResponse",
    "TrackerView",
];

/// Tools whose answer has such a type and still quotes nobody, each with its reason.
const EXEMPT: &[(&str, &str)] = &[
    (
        "create_stream_channel",
        "a channel just written: its name is the call's, and nothing was recorded yet",
    ),
    (
        "create_subscription",
        "a subscription just written: its name is the call's, and no source answered yet",
    ),
];

/// Every schema `value` refers to, followed through the references.
fn referenced(schemas: &Value, value: &Value, found: &mut BTreeSet<String>) {
    match value {
        Value::Object(map) => {
            if let Some(name) = map
                .get("$ref")
                .and_then(Value::as_str)
                .and_then(|reference| reference.rsplit('/').next())
                && found.insert(name.to_owned())
            {
                referenced(schemas, &schemas[name], found);
            }
            for inner in map.values() {
                referenced(schemas, inner, found);
            }
        }
        Value::Array(items) => {
            for item in items {
                referenced(schemas, item, found);
            }
        }
        _ => {}
    }
}

/// The third-party types the successful answers of a tool's routes carry.
fn third_party_types(document: &Value, tool: &str) -> BTreeSet<String> {
    let schemas = &document["components"]["schemas"];
    let mut found = BTreeSet::new();
    for entry in rd_api_mcp::TOOL_POLICY
        .iter()
        .chain(rd_api_mcp::TOOL_ALSO_REACHES)
        .filter(|entry| entry.tool == tool)
    {
        let method = entry.method.as_str().to_ascii_lowercase();
        let responses = document["paths"][entry.path][&method]["responses"]
            .as_object()
            .unwrap_or_else(|| panic!("{method} {} answers nothing", entry.path));
        for (status, response) in responses {
            if status.starts_with('2') {
                referenced(schemas, response, &mut found);
            }
        }
    }
    found.retain(|name| THIRD_PARTY_TYPES.contains(&name.as_str()));
    found
}

#[test]
fn every_tool_answering_third_party_text_is_marked() {
    let document = serde_json::to_value(crate::openapi_document()).expect("serialise");
    let unmarked: Vec<String> = rd_api_mcp::TOOL_POLICY
        .iter()
        .map(|entry| entry.tool)
        .filter(|tool| !rd_api_mcp::UNTRUSTED_TEXT_TOOLS.contains(tool))
        .filter(|tool| !EXEMPT.iter().any(|(exempt, _)| exempt == tool))
        .filter_map(|tool| {
            let types = third_party_types(&document, tool);
            (!types.is_empty()).then(|| format!("{tool}: {types:?}"))
        })
        .collect();
    assert!(
        unmarked.is_empty(),
        "these tools answer third-party text and are neither in UNTRUSTED_TEXT_TOOLS nor \
         exempt here:\n  {}",
        unmarked.join("\n  ")
    );
}

/// A renamed type would check nothing, and an exemption that is no longer one reads as one.
#[test]
fn the_types_and_the_exemptions_are_real() {
    let document = serde_json::to_value(crate::openapi_document()).expect("serialise");
    for name in THIRD_PARTY_TYPES {
        assert!(
            document["components"]["schemas"][name].is_object(),
            "{name} is no schema"
        );
    }
    let mut sorted = THIRD_PARTY_TYPES.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted, THIRD_PARTY_TYPES);
    for (tool, reason) in EXEMPT {
        assert!(!reason.is_empty(), "{tool}");
        assert!(
            !third_party_types(&document, tool).is_empty(),
            "{tool} carries no such type"
        );
        assert!(
            !rd_api_mcp::UNTRUSTED_TEXT_TOOLS.contains(tool),
            "{tool} is marked and exempt"
        );
    }
}
