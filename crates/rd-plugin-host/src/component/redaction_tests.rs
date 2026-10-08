//! PL-02: a credential the host expanded into a plugin's request is masked in that plugin's
//! log, like a cookie the plugin read. A provider that echoes it in its answer hands it to the
//! plugin, and from there `host::log` wrote it out as it was.

use std::sync::Arc;

use rd_http::{ClientPool, NetworkDefaults};
use rd_plugin_api::ClientIdentity;
use tokio::sync::RwLock;

use super::wit_http;
use crate::{OwnEndpoints, PluginLimits, ResolverService, SandboxEngine};

const TOKEN: &str = "webhook-token-4711";

#[tokio::test]
async fn a_secret_expanded_into_a_request_is_masked_in_the_plugins_log() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = rd_db::Database::open(directory.path().join("db.sqlite"))
        .await
        .expect("database");
    let secrets = rd_secrets::SecretStore::open(directory.path().join("secrets"))
        .await
        .expect("secret store");
    let reference = secrets
        .put_string(TOKEN.to_owned())
        .await
        .expect("put secret");
    let service = ResolverService::new(
        database,
        ClientPool::default(),
        secrets,
        Arc::new(RwLock::new(NetworkDefaults::default())),
        None,
        OwnEndpoints::default(),
    );
    let sandbox = SandboxEngine::new(PluginLimits::default()).expect("sandbox");
    let mut store = sandbox
        .create_extension_store(
            vec!["127.0.0.1".to_owned()],
            Some(service.host()),
            ClientIdentity {
                account_id: None,
                proxy_profile_id: None,
                tls_revision: 0,
            },
            Some(reference),
            false,
        )
        .expect("store");
    assert_eq!(store.data().redact_log(TOKEN), TOKEN, "nothing known yet");

    // This machine is refused only after the request was built, so the secret was expanded
    // and nothing was sent.
    let refused = wit_http::Host::http_request(
        store.data_mut(),
        "GET".to_owned(),
        "http://127.0.0.1:9/hook".to_owned(),
        Vec::new(),
        vec![wit_http::RequestHeader {
            name: "Authorization".to_owned(),
            value_template: "Bearer {{secret}}".to_owned(),
        }],
        Vec::new(),
    )
    .await;

    assert!(refused.is_err(), "a request to this machine is refused");
    assert_eq!(
        store.data().redact_log(&format!("echo: {TOKEN}")),
        "echo: [REDACTED]"
    );
}
