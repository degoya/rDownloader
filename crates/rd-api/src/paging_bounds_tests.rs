//! Every page size says its bounds in the document (RD-1480-10, API-13).
//!
//! Two ways to treat a value outside them, and neither changes: the paged lists refuse it, so a
//! client never takes a shortened page for the end of the list, and the histories that never
//! refused it bring it to the nearest bound and say so in the description -- refusing it there now
//! would break a client that asks for too much today.

/// The bound of each route's `limit`, as its handler enforces it.
const BOUNDS: &[(&str, u64)] = &[
    ("/api/v1/audit/export", 10_000),
    // One query type with the export; the records refuse more than 500, as their text says.
    ("/api/v1/audit/records", 10_000),
    ("/api/v1/automations/runs", 500),
    ("/api/v1/collector/batches", 1_000),
    ("/api/v1/collector/candidates", 1_000),
    ("/api/v1/collector/packages", 1_000),
    ("/api/v1/diagnostics/logs", 500),
    ("/api/v1/downloads", 1_000),
    ("/api/v1/downloads/{id}/torrent/peers", 200),
    ("/api/v1/history", 1_000),
    ("/api/v1/notifications/deliveries", 500),
    ("/api/v1/nzb/imports", 1_000),
    ("/api/v1/packages", 1_000),
    ("/api/v1/queue/search", 50),
    ("/api/v1/storage/operations", 1_000),
    ("/api/v1/streams/runs", 200),
    ("/api/v1/subscriptions/{id}/items/page", 200),
    ("/api/v1/subscriptions/{id}/runs", 200),
];

#[test]
fn every_page_size_names_its_bounds() {
    let document = serde_json::to_value(crate::openapi_document()).expect("serialise");
    let mut found: Vec<(String, u64)> = Vec::new();
    for (path, item) in document["paths"].as_object().expect("paths") {
        let Some(parameters) = item.pointer("/get/parameters").and_then(|it| it.as_array()) else {
            continue;
        };
        for parameter in parameters {
            if parameter["name"] != "limit" || parameter["in"] != "query" {
                continue;
            }
            let schema = &parameter["schema"];
            assert_eq!(schema["minimum"].as_f64(), Some(1.0), "{path}: {schema}");
            let maximum = schema["maximum"]
                .as_f64()
                .unwrap_or_else(|| panic!("GET {path} does not say its largest page"));
            found.push((path.clone(), maximum as u64));
        }
    }
    found.sort();
    let expected: Vec<(String, u64)> = BOUNDS
        .iter()
        .map(|(path, maximum)| ((*path).to_owned(), *maximum))
        .collect();
    assert_eq!(found, expected);
}
