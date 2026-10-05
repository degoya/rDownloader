//! Accounts, proxy profiles and Usenet servers: secret references and precedence.

use rd_core::ProxyKind;

use super::{SELECTION, probe_url};
use crate::{Database, NewAccount, NewProxyProfile, NewUsenetServer, UpdateAccount};

#[tokio::test]
async fn account_and_proxy_lists_never_serialize_secret_references() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("network.sqlite"))
        .await
        .expect("database");
    let proxy = database
        .create_proxy_profile(NewProxyProfile {
            name: "Local SOCKS".to_owned(),
            kind: ProxyKind::Socks5,
            endpoint: "socks5h://127.0.0.1:1080".parse().expect("URL"),
            username: Some("proxy-user".to_owned()),
            secret_ref: Some("secret://proxy/password".to_owned()),
        })
        .await
        .expect("proxy");
    database
        .create_account(NewAccount {
            provider: "premiumize".to_owned(),
            label: "Premiumize".to_owned(),
            username: None,
            credential_mode: None,
            secret_ref: Some("secret://premiumize/api-key".to_owned()),
            cookie_ref: None,
            proxy_profile_id: Some(proxy.id),
            enabled: true,
        })
        .await
        .expect("account");

    let proxy_json = serde_json::to_value(database.list_proxy_profiles().await.expect("proxies"))
        .expect("proxy JSON");
    let account_json = serde_json::to_value(database.list_accounts().await.expect("accounts"))
        .expect("account JSON");
    assert_eq!(proxy_json[0]["has_credentials"], true);
    assert!(proxy_json[0].get("secret_ref").is_none());
    assert_eq!(account_json[0]["has_secret"], true);
    assert_eq!(account_json[0]["has_cookies"], false);
    assert!(account_json[0].get("secret_ref").is_none());
}

#[tokio::test]
async fn account_updates_preserve_selected_secret_references() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("account-update.sqlite"))
        .await
        .expect("database");
    let account = database
        .create_account(NewAccount {
            provider: "ddownload".to_owned(),
            label: "Old label".to_owned(),
            username: Some("reader".to_owned()),
            credential_mode: None,
            secret_ref: Some("vault://password".to_owned()),
            cookie_ref: Some("vault://cookies".to_owned()),
            proxy_profile_id: None,
            enabled: true,
        })
        .await
        .expect("account");

    let updated = database
        .update_account(
            account.id,
            UpdateAccount {
                provider: "ddownload".to_owned(),
                label: "New label".to_owned(),
                username: Some("reader".to_owned()),
                credential_mode: None,
                secret_ref: Some("vault://password".to_owned()),
                cookie_ref: None,
                proxy_profile_id: None,
                enabled: false,
            },
        )
        .await
        .expect("update account");

    assert_eq!(updated.label, "New label");
    assert!(updated.has_secret);
    assert!(!updated.has_cookies);
    assert!(!updated.enabled);
    assert_eq!(
        database
            .account_secret_refs(account.id)
            .await
            .expect("secret refs"),
        Some((Some("vault://password".to_owned()), None))
    );
}

#[tokio::test]
async fn proxy_precedence_is_job_then_account_then_global() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("precedence.sqlite"))
        .await
        .expect("database");
    let mut proxies = Vec::new();
    for name in ["global", "account", "job"] {
        proxies.push(
            database
                .create_proxy_profile(NewProxyProfile {
                    name: name.to_owned(),
                    kind: ProxyKind::Http,
                    endpoint: format!("http://{name}.example.test:8080")
                        .parse()
                        .expect("URL"),
                    username: None,
                    secret_ref: None,
                })
                .await
                .expect("proxy"),
        );
    }
    let account = database
        .create_account(NewAccount {
            provider: "premiumize".to_owned(),
            label: "account".to_owned(),
            username: None,
            credential_mode: None,
            secret_ref: None,
            cookie_ref: None,
            proxy_profile_id: Some(proxies[1].id),
            enabled: true,
        })
        .await
        .expect("account");

    let global = database
        .network_client_config(None, None, Some(proxies[0].id), SELECTION, &probe_url())
        .await
        .expect("global config");
    let account_config = database
        .network_client_config(
            Some(account.id),
            None,
            Some(proxies[0].id),
            SELECTION,
            &probe_url(),
        )
        .await
        .expect("account config");
    let job = database
        .network_client_config(
            Some(account.id),
            Some(proxies[2].id),
            Some(proxies[0].id),
            SELECTION,
            &probe_url(),
        )
        .await
        .expect("job config");

    assert_eq!(global.proxy.expect("global proxy").id, proxies[0].id);
    assert_eq!(
        account_config.proxy.expect("account proxy").id,
        proxies[1].id
    );
    assert_eq!(job.proxy.expect("job proxy").id, proxies[2].id);
}

#[tokio::test]
async fn usenet_server_password_is_redacted_and_priority_is_stable() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("usenet.sqlite"))
        .await
        .expect("database");
    let created = database
        .create_usenet_server(NewUsenetServer {
            name: "Primary".to_owned(),
            host: "news.example.test".to_owned(),
            port: 563,
            tls: true,
            username: Some("reader".to_owned()),
            password_ref: Some("vault://password".to_owned()),
            proxy_profile_id: None,
            priority: 10,
            max_connections: 8,
            enabled: true,
        })
        .await
        .expect("server");
    let servers = database.list_usenet_servers().await.expect("servers");
    assert_eq!(servers[0].priority, 10);
    assert!(servers[0].has_password);
    let json = serde_json::to_value(&servers[0]).expect("JSON");
    assert!(json.get("password_ref").is_none());
    let runtime = database
        .usenet_connection_config(created.id)
        .await
        .expect("runtime config")
        .expect("server exists");
    assert_eq!(runtime.password_ref.as_deref(), Some("vault://password"));

    let updated = database
        .update_usenet_server(
            created.id,
            NewUsenetServer {
                name: "Primary updated".to_owned(),
                host: "news2.example.test".to_owned(),
                port: 119,
                tls: false,
                username: Some("reader".to_owned()),
                password_ref: runtime.password_ref,
                proxy_profile_id: None,
                priority: 20,
                max_connections: 4,
                enabled: false,
            },
        )
        .await
        .expect("update server");
    assert_eq!(updated.name, "Primary updated");
    assert_eq!(updated.priority, 20);
    assert!(updated.has_password);
    assert!(!updated.enabled);
}
