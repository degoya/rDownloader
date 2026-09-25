//! `rdownloader links …`.

use std::io::Read;

use anyhow::Result;
use serde_json::json;

use super::{
    Client, CommandError, Failure,
    output::{Format, emit, shorten, table},
};

/// Where `links add` puts the links (RD-130-19).
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Target<'a> {
    /// A category name or id.
    pub category: Option<&'a str>,
    pub package: Option<&'a str>,
    /// The download queue instead of the LinkGrabber.
    pub enqueue: bool,
}

pub(crate) async fn list(client: &Client, format: Format) -> Result<()> {
    let packages: Vec<serde_json::Value> = client.get("/api/v1/collector/packages").await?;
    emit(format, &packages, |packages| {
        let rows: Vec<Vec<String>> = packages
            .iter()
            .map(|package| {
                vec![
                    string(package, "id"),
                    shorten(&string(package, "name"), 46),
                    string(package, "state"),
                ]
            })
            .collect();
        table(&["ID", "PACKAGE", "STATE"], &rows);
    })
}

pub(crate) async fn add(
    client: &Client,
    format: Format,
    links: &[String],
    target: Target<'_>,
) -> Result<()> {
    let text = if links.len() == 1 && links[0] == "-" {
        // `-` reads from standard input so a list of links can be piped in, which is the
        // only practical way to hand over more than a handful.
        let mut buffer = String::new();
        std::io::stdin().read_to_string(&mut buffer)?;
        buffer
    } else {
        links.join("\n")
    };
    anyhow::ensure!(!text.trim().is_empty(), "no links given");
    let category_id = match target.category {
        Some(category) => Some(category_id(client, category).await?),
        None => None,
    };
    if target.enqueue {
        return enqueue_directly(
            client,
            format,
            &text,
            category_id.as_deref(),
            target.package,
        )
        .await;
    }
    // `source` is required by the intake contract and identifies where the links came from;
    // the LinkGrabber's routing rules can narrow on it, so a CLI hand-over has to be
    // distinguishable from a clipboard capture rather than pretending to be one.
    let response: serde_json::Value = client
        .post(
            "/api/v1/collector/batches",
            &json!({
                "text": text,
                "source": "api",
                "source_label": "cli",
                "package_name": target.package,
            }),
        )
        .await?;
    // The intake takes no category -- routing rules decide there -- so a chosen one is set
    // on the packages it made, through the same bulk edit the LinkGrabber's toolbar uses.
    if let Some(category_id) = &category_id {
        let ids: Vec<String> = response
            .get("packages")
            .and_then(serde_json::Value::as_array)
            .map(|packages| {
                packages
                    .iter()
                    .map(|package| string(package, "id"))
                    .collect()
            })
            .unwrap_or_default();
        if !ids.is_empty() {
            let _: serde_json::Value = client
                .post(
                    "/api/v1/collector/packages/bulk",
                    &json!({ "ids": ids, "category_id": category_id }),
                )
                .await?;
        }
    }
    emit(format, &response, |response| {
        println!(
            "{} link(s) in {} package(s)",
            count(response, "candidates"),
            count(response, "packages")
        );
    })
}

/// Adds every link in `text` to the queue, with the category and package asked for.
///
/// The links are found the way the LinkGrabber finds them in pasted text, so `--enqueue`
/// takes exactly what the default would have taken; the route is `queue add`'s.
async fn enqueue_directly(
    client: &Client,
    format: Format,
    text: &str,
    category_id: Option<&str>,
    package: Option<&str>,
) -> Result<()> {
    let urls = rd_collector::extract_urls(text);
    if urls.is_empty() {
        return Err(CommandError::new(Failure::Usage, "no links found in the input").into());
    }
    let mut created = Vec::with_capacity(urls.len());
    for url in urls {
        let download: serde_json::Value = client
            .post(
                "/api/v1/downloads",
                &json!({
                    "url": url.as_str(),
                    "category_id": category_id,
                    "package_name": package,
                }),
            )
            .await?;
        created.push(download);
    }
    emit(format, &created, |created| {
        let rows: Vec<Vec<String>> = created
            .iter()
            .map(|download| {
                vec![
                    string(download, "id"),
                    shorten(&string(download, "file_name"), 60),
                ]
            })
            .collect();
        table(&["ID", "FILE"], &rows);
    })
}

/// The id of the category `wanted` names: an id as it is, a name looked up (ignoring case).
async fn category_id(client: &Client, wanted: &str) -> Result<String> {
    if uuid::Uuid::parse_str(wanted).is_ok() {
        return Ok(wanted.to_owned());
    }
    let categories: Vec<serde_json::Value> = client.get("/api/v1/categories").await?;
    categories
        .iter()
        .find(|category| string(category, "name").eq_ignore_ascii_case(wanted))
        .map(|category| string(category, "id"))
        .ok_or_else(|| {
            CommandError::new(
                Failure::Usage,
                format!("there is no category named '{wanted}'"),
            )
            .into()
        })
}

pub(crate) async fn enqueue(client: &Client, format: Format, ids: &[String]) -> Result<()> {
    let mut results = Vec::new();
    for id in ids {
        let response: serde_json::Value = client
            .post(
                &format!("/api/v1/collector/packages/{id}/enqueue"),
                &json!({}),
            )
            .await?;
        results.push(response);
    }
    emit(format, &results, |results| {
        let rows: Vec<Vec<String>> = results
            .iter()
            .map(|result| vec![string(result, "id"), shorten(&string(result, "name"), 60)])
            .collect();
        table(&["ID", "PACKAGE"], &rows);
    })
}

pub(crate) async fn remove(client: &Client, format: Format, ids: &[String]) -> Result<()> {
    let mut removed = Vec::new();
    for id in ids {
        let response: serde_json::Value = client
            .delete(&format!("/api/v1/collector/packages/{id}"))
            .await?;
        removed.push(response);
    }
    emit(format, &removed, |removed| {
        println!("{} package(s) removed", removed.len());
    })
}

fn string(value: &serde_json::Value, key: &str) -> String {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn count(value: &serde_json::Value, key: &str) -> usize {
    value
        .get(key)
        .and_then(serde_json::Value::as_array)
        .map_or(0, Vec::len)
}
