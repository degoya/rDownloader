use std::sync::Arc;

use rd_core::AuthProfileId;
use reqwest::cookie::Jar;
use secrecy::SecretString;

use super::{AuthMaterial, ClientContext, ClientKey, ClientPool};

fn key(profile: Option<AuthProfileId>, revision: i64) -> ClientKey {
    ClientKey {
        proxy_profile_id: None,
        account_id: None,
        cookie_ref: None,
        auth_profile_id: profile,
        auth_revision: revision,
        replay_scope: None,
        tls_revision: 0,
        address_policy: None,
    }
}

fn context(key: ClientKey) -> ClientContext {
    ClientContext {
        key,
        proxy: None,
        proxy_credentials: None,
        cookie_jar: Arc::new(Jar::default()),
        custom_ca_pem: Vec::new(),
        auth: None,
        replay_scope: None,
    }
}

#[tokio::test]
async fn editing_a_profile_evicts_its_previous_client() {
    // Without eviction every profile edit would leak a cached Client, and with it a
    // connection pool still holding the old certificate and redirect scope.
    let pool = ClientPool::default();
    let profile = AuthProfileId::new();
    pool.get_or_create(context(key(Some(profile), 1)))
        .await
        .expect("first client");
    pool.get_or_create(context(key(Some(profile), 2)))
        .await
        .expect("second client");

    let clients = pool.clients.read().await;
    assert_eq!(clients.len(), 1);
    assert_eq!(
        clients.keys().next().expect("key").auth_revision,
        2,
        "the stale revision must be dropped"
    );
}

/// TR-16: one client per replay scope or address rule, and no more of them than the bound.
#[tokio::test]
async fn scoped_clients_are_bounded_and_the_oldest_goes_first() {
    let pool = ClientPool::default();
    let scoped = |scope: u64| ClientKey {
        replay_scope: Some(scope),
        ..key(None, 0)
    };
    pool.get_or_create(context(key(None, 0)))
        .await
        .expect("ordinary client");
    for scope in 0..(super::MAX_SCOPED_CLIENTS as u64 + 5) {
        pool.get_or_create(context(scoped(scope)))
            .await
            .expect("scoped client");
    }

    let clients = pool.clients.read().await;
    assert_eq!(clients.len(), super::MAX_SCOPED_CLIENTS + 1);
    assert!(
        clients.contains_key(&key(None, 0)),
        "an ordinary client stays"
    );
    assert!(
        !clients.contains_key(&scoped(0)),
        "the oldest scoped client went"
    );
    assert!(clients.contains_key(&scoped(super::MAX_SCOPED_CLIENTS as u64 + 4)));
}

/// A hoster sets its session cookie while the plugin resolves, and the download wants it
/// back. The plugin's client carries an address rule and the download's does not, so they
/// are two clients; they share one jar, and another account's client does not.
#[tokio::test]
async fn a_cookie_the_plugin_client_received_is_sent_by_the_download_client() {
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    let server = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = server.local_addr().expect("address").port();
    let seen = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let log = Arc::clone(&seen);
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = server.accept().await {
            let mut buffer = [0_u8; 4096];
            let read = stream.read(&mut buffer).await.unwrap_or(0);
            let request = String::from_utf8_lossy(&buffer[..read]).to_ascii_lowercase();
            let cookie = request
                .lines()
                .find_map(|line| line.strip_prefix("cookie:"))
                .map(|value| value.trim().to_owned())
                .unwrap_or_default();
            log.lock().expect("log").push(cookie);
            let _ = stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nset-cookie: xfss=resolved; Path=/\r\n\
                          content-length: 0\r\nconnection: close\r\n\r\n",
                )
                .await;
        }
    });
    let pool = ClientPool::default();
    let account = |id: rd_core::AccountId, policy: Option<crate::AddressPolicy>| ClientKey {
        account_id: Some(id),
        address_policy: policy,
        ..key(None, 0)
    };
    let own = rd_core::AccountId::new();
    let plugin = pool
        .get_or_create(context(account(
            own,
            Some(crate::AddressPolicy::new(false)),
        )))
        .await
        .expect("plugin client");
    let download = pool
        .get_or_create(context(account(own, None)))
        .await
        .expect("download client");
    let stranger = pool
        .get_or_create(context(account(rd_core::AccountId::new(), None)))
        .await
        .expect("another account's client");

    // A literal address never reaches the guarded resolver, so the plugin client may call
    // the loopback listener here; the host judges literals before the request.
    let base = format!("http://127.0.0.1:{port}");
    plugin
        .get(format!("{base}/resolve"))
        .send()
        .await
        .expect("resolve");
    download
        .get(format!("{base}/file"))
        .send()
        .await
        .expect("download");
    stranger
        .get(format!("{base}/file"))
        .send()
        .await
        .expect("stranger");

    let seen = seen.lock().expect("log").clone();
    assert_eq!(seen.len(), 3);
    assert_eq!(seen[0], "", "nothing was set before the resolve");
    assert_eq!(
        seen[1], "xfss=resolved",
        "the download sends the resolve's cookie"
    );
    assert_eq!(seen[2], "", "another account has a jar of its own");
    assert_eq!(pool.clients.read().await.len(), 3);
    assert_eq!(pool.jars.lock().expect("jars").len(), 2);
}

#[tokio::test]
async fn clients_of_different_profiles_coexist() {
    let pool = ClientPool::default();
    pool.get_or_create(context(key(Some(AuthProfileId::new()), 1)))
        .await
        .expect("first");
    pool.get_or_create(context(key(Some(AuthProfileId::new()), 1)))
        .await
        .expect("second");
    assert_eq!(pool.clients.read().await.len(), 2);
}

/// RD-130-24: a hop the request's gate refuses is never requested. The redirect comes
/// back as the response; the same client without a gate follows it, as every ordinary
/// download always did.
#[tokio::test]
async fn a_gated_redirect_is_handed_back_unfollowed() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    let target = TcpListener::bind("127.0.0.1:0").await.expect("bind target");
    let target_port = target.local_addr().expect("target address").port();
    let reached = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&reached);
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = target.accept().await {
            counter.fetch_add(1, Ordering::SeqCst);
            let mut buffer = [0_u8; 1024];
            let _ = stream.read(&mut buffer).await;
            let _ = stream
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
                .await;
        }
    });
    let origin = TcpListener::bind("127.0.0.1:0").await.expect("bind origin");
    let origin_port = origin.local_addr().expect("origin address").port();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = origin.accept().await {
            let mut buffer = [0_u8; 1024];
            let _ = stream.read(&mut buffer).await;
            let answer = format!(
                "HTTP/1.1 307 Temporary Redirect\r\nlocation: http://127.0.0.1:{target_port}/x\r\n\
                     content-length: 0\r\nconnection: close\r\n\r\n"
            );
            let _ = stream.write_all(answer.as_bytes()).await;
        }
    });

    let client = ClientPool::default()
        .get_or_create(context(key(None, 0)))
        .await
        .expect("client");
    let start = format!("http://127.0.0.1:{origin_port}/start");
    let gate: crate::RedirectGate = Arc::new(move |hop: &url::Url| hop.port() != Some(target_port));
    let refused = crate::with_redirect_gate(gate, client.post(&start).body("payload").send())
        .await
        .expect("the redirect itself is the response");
    assert_eq!(refused.status().as_u16(), 307);
    assert_eq!(
        reached.load(Ordering::SeqCst),
        0,
        "the refused hop was requested"
    );

    let followed = client
        .post(&start)
        .body("payload")
        .send()
        .await
        .expect("followed");
    assert_eq!(followed.status().as_u16(), 200);
    assert_eq!(reached.load(Ordering::SeqCst), 1);
}

#[test]
fn a_client_certificate_must_be_a_pem_bundle() {
    // Identity::from_pem rejects encrypted keys and unknown sections outright, so a
    // bad bundle has to fail here rather than at the first download.
    let context = ClientContext {
        auth: Some(AuthMaterial {
            identity_pem: SecretString::from("-----BEGIN ENCRYPTED PRIVATE KEY-----\nx\n"),
        }),
        ..context(key(Some(AuthProfileId::new()), 1))
    };
    let jar = Arc::clone(&context.cookie_jar);
    assert!(super::build_client(&context, jar).is_err());
}
