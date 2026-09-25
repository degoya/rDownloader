//! `rdownloader queue …`.

use anyhow::Result;
use serde::Serialize;
use serde_json::json;

use super::{
    Client,
    output::{Format, bytes, emit, shorten, table},
};

pub(crate) async fn list(client: &Client, format: Format) -> Result<()> {
    let downloads: Vec<serde_json::Value> = client.get("/api/v1/downloads").await?;
    emit(format, &downloads, |downloads| {
        let rows: Vec<Vec<String>> = downloads
            .iter()
            .map(|download| {
                let total = number(download, "total_bytes");
                let done = number(download, "committed_bytes");
                vec![
                    text(download, "id"),
                    shorten(&text(download, "file_name"), 42),
                    text(download, "state"),
                    format!("{}/{}", bytes(done), bytes(total)),
                ]
            })
            .collect();
        table(&["ID", "FILE", "STATE", "PROGRESS"], &rows);
    })
}

pub(crate) async fn summary(client: &Client, format: Format) -> Result<()> {
    let summary: serde_json::Value = client.get("/api/v1/downloads/summary").await?;
    emit(format, &summary, |summary| {
        let rows: Vec<Vec<String>> = summary
            .as_object()
            .map(|fields| {
                fields
                    .iter()
                    .map(|(key, value)| vec![key.clone(), render(value)])
                    .collect()
            })
            .unwrap_or_default();
        table(&["FIELD", "VALUE"], &rows);
    })
}

pub(crate) async fn add(client: &Client, format: Format, urls: &[String]) -> Result<()> {
    let mut created = Vec::new();
    for url in urls {
        let download: serde_json::Value = client
            .post("/api/v1/downloads", &json!({ "url": url }))
            .await?;
        created.push(download);
    }
    emit(format, &created, |created| {
        let rows: Vec<Vec<String>> = created
            .iter()
            .map(|download| {
                vec![
                    text(download, "id"),
                    shorten(&text(download, "file_name"), 60),
                ]
            })
            .collect();
        table(&["ID", "FILE"], &rows);
    })
}

/// Applies one bulk action to a list of download ids.
pub(crate) async fn act(
    client: &Client,
    format: Format,
    action: &str,
    ids: &[String],
) -> Result<()> {
    #[derive(Serialize)]
    struct Bulk<'a> {
        ids: &'a [String],
        action: &'a str,
    }
    let response: serde_json::Value = client
        .post("/api/v1/downloads/bulk", &Bulk { ids, action })
        .await?;
    emit(format, &response, |response| {
        println!("{} affected", number(response, "affected"));
        if let Some(errors) = response.get("errors").and_then(serde_json::Value::as_array) {
            // Reported rather than swallowed: a bulk action that partly failed is the case
            // where a script most needs to know something is wrong.
            for error in errors {
                println!("error: {}", render(error));
            }
        }
    })
}

fn text(value: &serde_json::Value, key: &str) -> String {
    value.get(key).map(render).unwrap_or_default()
}

/// Reads a number that the API may send as a JSON number or as a string of digits.
fn number(value: &serde_json::Value, key: &str) -> u64 {
    match value.get(key) {
        Some(serde_json::Value::Number(number)) => number.as_u64().unwrap_or(0),
        Some(serde_json::Value::String(text)) => text.parse().unwrap_or(0),
        _ => 0,
    }
}

fn render(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    }
}
