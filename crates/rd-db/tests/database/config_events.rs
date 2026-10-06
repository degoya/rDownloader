//! Configuration writes that used to change state silently (RD-140 audit, pass seven).
//!
//! Three families of writes committed a row and told no open client: every automation write,
//! the managed-tool and tool-manifest writes, and the plugin trust store. An interface that
//! edited one of them in a second tab kept showing the old value until somebody reloaded the
//! page, which is indistinguishable from the write having been lost. These tests hold the
//! event each write now publishes, and hold its payload to what a `Config`-scoped stream
//! subscriber may see.

use rd_core::EventKind;
use rd_db::{Database, NewAutomation, NewManagedTool, NewPluginTrustedKey};
use tempfile::TempDir;

async fn database(directory: &TempDir) -> Database {
    Database::open(directory.path().join("events.sqlite"))
        .await
        .expect("database")
}

fn automation(name: &str) -> NewAutomation {
    NewAutomation {
        name: name.to_owned(),
        enabled: true,
        trigger: rd_automation::Trigger::DownloadCompleted,
        condition: rd_automation::ConditionNode::Always,
        actions: Vec::new(),
    }
}

/// Drains the bus and returns every event of one kind, so a test can assert the count.
///
/// The count is the point: a write that published twice would look correct to any assertion
/// that only checks the first event, and a duplicate makes an interface refetch twice.
fn drain(
    events: &mut tokio::sync::broadcast::Receiver<rd_core::EventEnvelope>,
    kind: &EventKind,
) -> Vec<serde_json::Value> {
    let mut seen = Vec::new();
    while let Ok(event) = events.try_recv() {
        if event.kind == *kind {
            seen.push(event.payload);
        }
    }
    seen
}

#[tokio::test]
async fn every_automation_write_announces_exactly_one_change() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = database(&directory).await;
    let mut events = database.subscribe();

    let created = database
        .upsert_automation(None, automation("Move finished packages"))
        .await
        .expect("create");
    let payloads = drain(&mut events, &EventKind::AutomationChanged);
    assert_eq!(payloads.len(), 1, "creating published {payloads:?}");
    assert_eq!(
        payloads[0].get("automation_id").and_then(|id| id.as_str()),
        Some(created.id.to_string().as_str()),
        "the event did not name the automation that changed"
    );

    database
        .set_automation_enabled(created.id, false)
        .await
        .expect("disable");
    assert_eq!(
        drain(&mut events, &EventKind::AutomationChanged).len(),
        1,
        "disabling an automation published the wrong number of events"
    );

    database
        .delete_automation(created.id)
        .await
        .expect("delete");
    assert_eq!(
        drain(&mut events, &EventKind::AutomationChanged).len(),
        1,
        "deleting an automation published the wrong number of events"
    );
}

/// A failed write must stay silent, or a client refetches and finds nothing changed.
#[tokio::test]
async fn deleting_an_automation_that_is_gone_publishes_nothing() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = database(&directory).await;
    let mut events = database.subscribe();

    database
        .delete_automation(rd_core::AutomationId::new())
        .await
        .expect_err("deleting an unknown automation has to fail");
    assert!(
        drain(&mut events, &EventKind::AutomationChanged).is_empty(),
        "a rejected delete announced a change"
    );
}

/// The tool events name the tool, and never where its bytes came from.
///
/// `source_url` can carry a token in an installation that mirrors the downloads, and the
/// digest belongs to the verification record rather than to a broadcast channel; both are
/// behind `GET /api/v1/system/tools` for a caller allowed to read them.
#[tokio::test]
async fn a_managed_tool_write_names_the_tool_and_not_its_source() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = database(&directory).await;
    let mut events = database.subscribe();

    database
        .record_managed_tool(NewManagedTool {
            name: "yt-dlp".to_owned(),
            version: "2026.01.01".to_owned(),
            source_url: "https://mirror.test/secret-token/yt-dlp".to_owned(),
            sha256: "00".repeat(32),
        })
        .await
        .expect("record");
    let payloads = drain(&mut events, &EventKind::ManagedToolChanged);
    assert_eq!(payloads.len(), 1, "recording published {payloads:?}");
    assert_eq!(
        payloads[0].get("name").and_then(|name| name.as_str()),
        Some("yt-dlp")
    );
    assert_eq!(
        payloads[0].get("version").and_then(|name| name.as_str()),
        Some("2026.01.01")
    );
    let serialised = payloads[0].to_string();
    assert!(
        !serialised.contains("mirror.test") && !serialised.contains("secret-token"),
        "the download URL reached the event bus: {serialised}"
    );
    assert!(
        !serialised.contains(&"00".repeat(32)),
        "the digest reached the event bus: {serialised}"
    );

    database
        .forget_managed_tool("yt-dlp".to_owned(), "2026.01.01".to_owned())
        .await
        .expect("forget");
    assert_eq!(
        drain(&mut events, &EventKind::ManagedToolChanged).len(),
        1,
        "forgetting a tool published the wrong number of events"
    );

    database
        .accept_tool_manifest(7, "2026-01-01T00:00:00Z".to_owned())
        .await
        .expect("accept");
    let payloads = drain(&mut events, &EventKind::ManagedToolChanged);
    assert_eq!(payloads.len(), 1, "accepting published {payloads:?}");
    assert_eq!(
        payloads[0].get("sequence").and_then(|value| value.as_i64()),
        Some(7),
        "the manifest event did not say how far the sequence advanced"
    );
}

/// Trusting a signing key announces the decision without republishing the key.
///
/// `plugin_trust.changed` is `Secrets`-scoped, like `/api/v1/plugins/keys` itself, so the
/// event reaches the scope that made the write and no wider. Even there the payload stays the
/// key id: an event must not become a second read path for a table that deliberately has a
/// narrow one, and a stream is kept by whoever subscribed, long after the request ended.
/// The kind is asserted as well as the payload -- filed under `PluginChanged` this went to
/// `Admin` subscribers instead, and the suite had nothing that would have said so.
#[tokio::test]
async fn a_trust_decision_announces_the_key_id_and_nothing_else() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = database(&directory).await;
    let mut events = database.subscribe();

    database
        .trust_plugin_key(NewPluginTrustedKey {
            key_id: "key-1".to_owned(),
            public_key: "cHVibGljLWtleS1ieXRlcw==".to_owned(),
            fingerprint: "ff".repeat(32),
            plugin_name: Some("rd-plugin-example".to_owned()),
        })
        .await
        .expect("trust");
    let payloads = drain(&mut events, &EventKind::PluginTrustChanged);
    assert_eq!(payloads.len(), 1, "trusting a key published {payloads:?}");
    assert_eq!(
        payloads[0].get("key_id").and_then(|id| id.as_str()),
        Some("key-1")
    );
    let serialised = payloads[0].to_string();
    assert!(
        !serialised.contains("cHVibGljLWtleS1ieXRlcw==") && !serialised.contains(&"ff".repeat(32)),
        "the key material reached the event bus: {serialised}"
    );

    database
        .revoke_plugin_key("key-1".to_owned())
        .await
        .expect("revoke");
    assert_eq!(
        drain(&mut events, &EventKind::PluginTrustChanged).len(),
        1,
        "revoking a key published the wrong number of events"
    );
}
