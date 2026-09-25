//! RD-120-57: an indexer's key, queued into the LinkGrabber through the real path, comes back
//! out of no tool.
//!
//! The hit carries the canary as the `apikey` of its download address, which is how an indexer
//! really hands one out. `review_subscription_item` queues it exactly as a person approving it
//! would, so the candidate holds the key in clear. Then every tool the server lists is called,
//! and `envelope` searches each whole answer: with the known arguments of the other canary
//! tests where they exist, and with arguments built from the tool's own input schema otherwise,
//! so a tool added later is called here without anyone remembering to add it.

use std::collections::BTreeMap;

use super::remaining::{remaining_tools, seed_indexer_hits};
use super::{
    API_BEARER, CANARY, NOBODY, call, envelope, extract_json, handshake, installation_parts,
    mcp_request, new_tools, ok,
};

/// It goes out to the network; its price is proven by refusal in `everything`.
const NOT_CALLED: &[&str] = &["refresh_tool_manifest"];

/// Every tool the server offers, with its input schema.
async fn listed_tools(router: &axum::Router, session: &str) -> Vec<(String, serde_json::Value)> {
    let body = r#"{"jsonrpc":"2.0","id":3,"method":"tools/list","params":{}}"#;
    let (_, content_type, response) =
        call(router, mcp_request(Some(API_BEARER), Some(session), body)).await;
    let answer = extract_json(&content_type, &response);
    answer["result"]["tools"]
        .as_array()
        .expect("a tool list")
        .iter()
        .map(|tool| {
            let name = tool["name"].as_str().expect("a name").to_owned();
            (name, tool["inputSchema"].clone())
        })
        .collect()
}

/// A value of the shape `schema` asks for: the first choice, an unknown id, the smallest thing.
fn sample(schema: &serde_json::Value, root: &serde_json::Value) -> serde_json::Value {
    use serde_json::{Value, json};
    if let Some(reference) = schema["$ref"].as_str() {
        let name = reference.rsplit('/').next().unwrap_or_default();
        let target = if root["$defs"][name].is_null() {
            &root["definitions"][name]
        } else {
            &root["$defs"][name]
        };
        return sample(target, root);
    }
    if let Some(first) = schema["enum"].as_array().and_then(|values| values.first()) {
        return first.clone();
    }
    if let Some(constant) = schema.get("const") {
        return constant.clone();
    }
    for key in ["oneOf", "anyOf", "allOf"] {
        if let Some(choice) = schema[key]
            .as_array()
            .and_then(|choices| choices.iter().find(|choice| choice["type"] != "null"))
        {
            return sample(choice, root);
        }
    }
    let kind = match &schema["type"] {
        Value::Array(kinds) => kinds
            .iter()
            .find(|kind| *kind != "null")
            .cloned()
            .unwrap_or(Value::Null),
        other => other.clone(),
    };
    match kind.as_str() {
        Some("string") => json!(NOBODY),
        Some("array") => json!([sample(&schema["items"], root)]),
        Some("boolean") => json!(false),
        Some("integer" | "number") => json!(1),
        Some("object") => required_arguments(schema, root),
        _ => Value::Null,
    }
}

/// An object carrying every property the schema requires, and nothing else.
fn required_arguments(schema: &serde_json::Value, root: &serde_json::Value) -> serde_json::Value {
    let mut object = serde_json::Map::new();
    for name in schema["required"].as_array().into_iter().flatten() {
        let name = name.as_str().expect("a property name");
        object.insert(name.to_owned(), sample(&schema["properties"][name], root));
    }
    serde_json::Value::Object(object)
}

/// `NOBODY` replaced by a real id, at any depth.
fn with_id(arguments: &serde_json::Value, id: &str) -> serde_json::Value {
    serde_json::from_str(&arguments.to_string().replace(NOBODY, id)).expect("still JSON")
}

fn reads_only(name: &str) -> bool {
    ["list_", "get_", "preview_"]
        .iter()
        .any(|prefix| name.starts_with(prefix))
}

#[tokio::test]
async fn an_indexer_key_in_the_linkgrabber_reaches_no_tool_answer() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (router, database) = installation_parts(directory.path()).await;
    let session = handshake(&router, API_BEARER).await;

    let subscription = seed_indexer_hits(&router, &session, &database, &["hit"]).await;
    let page = ok(
        &router,
        &session,
        "list_subscription_items",
        serde_json::json!({ "id": subscription }),
    )
    .await;
    let item = page["items"][0]["id"].as_str().expect("item id").to_owned();
    ok(
        &router,
        &session,
        "review_subscription_item",
        serde_json::json!({ "id": item, "state": "queued" }),
    )
    .await;

    // The real path took it, key and all: a tool handing out the stored address would hand out
    // the key, so the test below is not searching an installation that never held it.
    let stored = database.list_candidates().await.expect("candidates");
    let candidate = stored
        .iter()
        .find(|candidate| candidate.url.as_str().contains(CANARY))
        .expect("the queued hit is a candidate carrying the key")
        .id
        .to_string();

    // The two tools the finding named: they list the candidate, and its address keeps its shape.
    for tool in ["list_collector", "list_candidates"] {
        let answer = ok(&router, &session, tool, serde_json::json!({})).await;
        let rendered = answer.to_string();
        assert!(rendered.contains(&candidate), "{tool}: {answer}");
        assert!(
            rendered.contains("indexer.invalid") && rendered.contains("apikey="),
            "{tool} lost the address rather than its key: {answer}"
        );
    }

    let mut known: BTreeMap<&str, serde_json::Value> = BTreeMap::new();
    for (name, arguments) in crate::id_free_reads() {
        known.entry(name).or_insert(arguments);
    }
    for (name, arguments, _) in new_tools().into_iter().chain(remaining_tools()) {
        known.entry(name).or_insert(arguments);
    }
    let tools = listed_tools(&router, &session).await;
    let arguments_for = |name: &str, schema: &serde_json::Value, id: &str| {
        let arguments = known
            .get(name)
            .cloned()
            .unwrap_or_else(|| required_arguments(schema, schema));
        with_id(&arguments, id)
    };

    // Reading first, with the candidate's id wherever a tool takes one.
    let mut called = 0usize;
    for (name, schema) in tools.iter().filter(|(name, _)| reads_only(name)) {
        let arguments = arguments_for(name, schema, &candidate);
        envelope(&router, API_BEARER, &session, name, &arguments).await;
        called += 1;
    }

    // Enqueued, an indexer hit is an NZB to fetch from that very address. The indexer does not
    // exist, so the fetch fails -- and the refusal quotes the address, which is the error path
    // through the mask.
    let enqueued = envelope(
        &router,
        API_BEARER,
        &session,
        "enqueue_candidate",
        &serde_json::json!({ "id": candidate }),
    )
    .await;
    let refusal = enqueued["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default();
    assert!(
        refusal.contains("collector.nzb_fetch_failed") && refusal.contains("apikey="),
        "{enqueued}"
    );

    // Then everything else, acting on the unknown id so the installation stays as it is.
    for (name, schema) in &tools {
        if reads_only(name) || NOT_CALLED.contains(&name.as_str()) {
            continue;
        }
        let arguments = arguments_for(name, schema, NOBODY);
        envelope(&router, API_BEARER, &session, name, &arguments).await;
        called += 1;
    }
    assert_eq!(
        called + NOT_CALLED.len(),
        tools.len(),
        "every listed tool was called"
    );
    assert!(tools.len() > 150, "the whole toolbox: {}", tools.len());
}
