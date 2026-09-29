use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    str::FromStr,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use reqwest::dns::Resolve as _;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use url::Url;

use super::*;

fn url(text: &str) -> Url {
    Url::parse(text).expect("url")
}

fn ip(text: &str) -> IpAddr {
    text.parse().expect("address")
}

/// A resolver that answers from a table, so a test decides what a name points at.
struct Table(HashMap<&'static str, Vec<IpAddr>>);

impl Table {
    fn new(entries: &[(&'static str, &[&str])]) -> Arc<Self> {
        Arc::new(Self(
            entries
                .iter()
                .map(|(name, addresses)| (*name, addresses.iter().map(|text| ip(text)).collect()))
                .collect(),
        ))
    }
}

impl HostLookup for Table {
    fn lookup<'a>(&'a self, host: &'a str) -> LookupFuture<'a> {
        let answer = self.0.get(host).cloned().unwrap_or_default();
        Box::pin(async move { Ok(answer) })
    }
}

/// A plain HTTP listener that counts every connection it accepts.
async fn counting_listener() -> (u16, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("address").port();
    let reached = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&reached);
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            counter.fetch_add(1, Ordering::SeqCst);
            let mut buffer = [0_u8; 1024];
            let _ = stream.read(&mut buffer).await;
            let _ = stream
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok")
                .await;
        }
    });
    (port, reached)
}

#[test]
fn this_machine_and_special_purpose_ranges_are_local() {
    for text in [
        "127.0.0.1",
        "127.13.13.13",
        "169.254.169.254",
        "169.254.0.1",
        "0.0.0.0",
        "0.1.2.3",
        "192.0.0.1",
        "198.18.0.1",
        "240.0.0.1",
        "255.255.255.255",
        "224.0.0.1",
        "::1",
        "::",
        "fe80::1",
        "ff02::1",
        "::ffff:127.0.0.1",
        "::ffff:169.254.169.254",
        "::127.0.0.1",
        "2002:7f00:0001::1",
        "2002:a9fe:a9fe::",
        "64:ff9b::7f00:1",
        "64:ff9b:1::1",
        "2001:0:4136:e378:8000:63bf:3fff:fdd2",
        "2001:2::1",
        "2001:10::1",
        "2001:db8::1",
        "3fff::1",
        "100::1",
    ] {
        assert_eq!(address_scope(ip(text)), AddressScope::Local, "{text}");
    }
}

#[test]
fn the_persons_own_network_is_private() {
    for text in [
        "10.0.0.5",
        "172.16.4.1",
        "172.31.255.255",
        "192.168.1.1",
        "100.64.0.1",
        "fc00::1",
        "fd12:3456::1",
        "fec0::1",
        "::ffff:10.1.2.3",
        "2002:0a00:0001::",
        "64:ff9b::10.0.0.1",
        "2606:4700:1:2:0:5efe:10.0.0.1",
    ] {
        assert_eq!(address_scope(ip(text)), AddressScope::Private, "{text}");
    }
}

#[test]
fn a_routable_address_is_public() {
    for text in [
        "93.184.216.34",
        "8.8.8.8",
        "172.32.0.1",
        "100.128.0.1",
        "2606:4700::1111",
        "2002:5db8:d822::1",
        "64:ff9b::93.184.216.34",
    ] {
        assert_eq!(address_scope(ip(text)), AddressScope::Public, "{text}");
    }
}

#[test]
fn the_private_network_is_reached_only_when_the_person_asked_for_it() {
    let internet = AddressPolicy::new(false);
    let lan = AddressPolicy::new(true);
    for text in ["192.168.1.20", "10.0.0.1", "fd00::1"] {
        assert!(!internet.permits(ip(text)), "{text}");
        assert!(lan.permits(ip(text)), "{text}");
    }
    for text in ["127.0.0.1", "169.254.169.254", "::1", "fe80::1", "0.0.0.0"] {
        assert!(!internet.permits(ip(text)), "{text}");
        assert!(!lan.permits(ip(text)), "{text}");
    }
    assert!(internet.permits(ip("93.184.216.34")));
}

#[test]
fn the_address_the_service_listens_on_is_refused_even_inside_the_lan() {
    let listen = SocketAddr::from(([192, 168, 1, 5], 8710));
    let lan = AddressPolicy::new(true).listening_on(Some(listen));
    assert!(!lan.permits(ip("192.168.1.5")));
    // The same address in its IPv6 costume is the same machine.
    assert!(!lan.permits(ip("::ffff:192.168.1.5")));
    assert!(lan.permits(ip("192.168.1.6")));
    // Not listening anywhere in particular changes nothing.
    assert!(
        AddressPolicy::new(true)
            .listening_on(None)
            .permits(ip("192.168.1.5"))
    );
}

#[tokio::test]
async fn every_literal_shape_of_an_internal_address_is_refused() {
    let nothing = Table::new(&[]);
    for text in [
        "http://127.0.0.1:8710/api/v1/downloads",
        "http://[::1]/f",
        "http://0.0.0.0/f",
        "http://169.254.169.254/latest/meta-data/",
        "http://[fe80::1]/f",
        "http://[::ffff:127.0.0.1]/f",
        // The URL parser reads both as 127.0.0.1, and so does every resolver.
        "http://2130706433/f",
        "http://0x7f.1/f",
        "http://224.0.0.1/f",
        "https://255.255.255.255/f",
    ] {
        for policy in [AddressPolicy::new(false), AddressPolicy::new(true)] {
            let refused = check_target(&policy, nothing.as_ref(), &url(text)).await;
            assert!(
                matches!(refused, Err(TargetRefusal::Refused(_))),
                "{text} was not refused: {refused:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_name_is_judged_by_every_address_it_answers_with() {
    let table = Table::new(&[
        ("mirror.test", &["127.0.0.1"]),
        ("metadata.test", &["169.254.169.254"]),
        // One routable and one loopback answer: the shape a rebinding attack takes.
        ("rebind.test", &["93.184.216.34", "127.0.0.1"]),
        ("lan.test", &["192.168.1.20"]),
        ("public.test", &["93.184.216.34"]),
    ]);
    let internet = AddressPolicy::new(false);
    let lan = AddressPolicy::new(true);
    for name in ["mirror.test", "metadata.test", "rebind.test"] {
        let target = url(&format!("http://{name}/f"));
        for policy in [&internet, &lan] {
            assert!(
                matches!(
                    check_target(policy, table.as_ref(), &target).await,
                    Err(TargetRefusal::Refused(_))
                ),
                "{name}"
            );
        }
    }
    let lan_target = url("http://lan.test/f");
    assert!(matches!(
        check_target(&internet, table.as_ref(), &lan_target).await,
        Err(TargetRefusal::Refused(_))
    ));
    assert_eq!(
        check_target(&lan, table.as_ref(), &lan_target)
            .await
            .expect("permitted"),
        vec![ip("192.168.1.20")]
    );
    assert!(
        check_target(&internet, table.as_ref(), &url("https://public.test/f"))
            .await
            .is_ok()
    );
    // A name without an address is not a refusal; it simply cannot be fetched.
    assert!(matches!(
        check_target(&internet, table.as_ref(), &url("http://nowhere.test/f")).await,
        Err(TargetRefusal::Unresolved(_))
    ));
}

/// FTP and SFTP open their own sockets: the addresses they are handed are the ones checked,
/// with the port on them, and a refusal comes back as the I/O error a connect returns.
#[tokio::test]
async fn a_transport_with_its_own_sockets_gets_only_checked_addresses() {
    let table = Table::new(&[
        ("mirror.test", &["127.0.0.1"]),
        ("rebind.test", &["93.184.216.34", "127.0.0.1"]),
        ("public.test", &["93.184.216.34"]),
    ]);
    let internet = AddressPolicy::new(false);
    for host in [
        "mirror.test",
        "rebind.test",
        "127.0.0.1",
        "[::1]",
        "::ffff:127.0.0.1",
    ] {
        let refused = connect_addresses(&internet, table.as_ref(), host, 21)
            .await
            .expect_err(host);
        assert_eq!(
            refused.kind(),
            std::io::ErrorKind::PermissionDenied,
            "{host}"
        );
        assert!(refusal_in(&refused).is_some(), "{host}");
    }
    assert_eq!(
        connect_addresses(&internet, table.as_ref(), "public.test", 2121)
            .await
            .expect("permitted"),
        vec![SocketAddr::new(ip("93.184.216.34"), 2121)]
    );
    // Nothing refused, nothing to connect to: an ordinary I/O error, not the guard's.
    let unresolved = connect_addresses(&internet, table.as_ref(), "nowhere.test", 22)
        .await
        .expect_err("no address");
    assert!(refusal_in(&unresolved).is_none());
}

#[tokio::test]
async fn localhost_is_refused_through_the_system_resolver() {
    let refused = check_target(
        &AddressPolicy::new(true),
        &SystemLookup,
        &url("http://localhost:8710/f"),
    )
    .await;
    assert!(
        matches!(refused, Err(TargetRefusal::Refused(_))),
        "{refused:?}"
    );
}

#[test]
fn a_redirect_may_only_go_to_http_and_to_a_permitted_literal_address() {
    let internet = AddressPolicy::new(false);
    let lan = AddressPolicy::new(true);
    for text in [
        "file:///etc/passwd",
        "ftp://mirror.example/f",
        "gopher://mirror.example/f",
        "http://127.0.0.1:8710/api",
        "http://169.254.169.254/latest",
        "http://[::1]/f",
    ] {
        assert!(internet.hop_refusal(&url(text)).is_some(), "{text}");
        assert!(lan.hop_refusal(&url(text)).is_some(), "{text}");
    }
    assert!(internet.hop_refusal(&url("http://192.168.0.1/f")).is_some());
    assert!(lan.hop_refusal(&url("http://192.168.0.1/f")).is_none());
    // A name is left to the resolver a guarded client always has.
    assert!(
        internet
            .hop_refusal(&url("https://cdn.example/f"))
            .is_none()
    );
}

#[tokio::test]
async fn the_resolver_refuses_at_connect_time() {
    let table = Table::new(&[
        ("mirror.test", &["127.0.0.1"]),
        ("public.test", &["93.184.216.34"]),
    ]);
    let resolver = GuardedResolver::with_lookup(AddressPolicy::new(true), table);
    let refused = resolver
        .resolve(reqwest::dns::Name::from_str("mirror.test").expect("name"))
        .await
        .err()
        .expect("refused");
    assert!(is_refusal(&*refused));
    let answered: Vec<SocketAddr> = resolver
        .resolve(reqwest::dns::Name::from_str("public.test").expect("name"))
        .await
        .expect("permitted")
        .collect();
    assert_eq!(answered, vec![SocketAddr::from(([93, 184, 216, 34], 0))]);
}

/// The rebinding case end to end: the name passed nothing, the client asks the guarded
/// resolver, and the connection to the loopback listener is never made. The same request over
/// a client that resolves the name without the guard does reach the listener, which is what
/// makes the zero count mean something.
#[tokio::test]
async fn a_guarded_client_never_connects_to_a_name_that_points_inside() {
    let (port, reached) = counting_listener().await;
    let table = Table::new(&[("mirror.test", &["127.0.0.1"])]);
    let guarded = reqwest::Client::builder()
        .no_proxy()
        .dns_resolver(GuardedResolver::with_lookup(
            AddressPolicy::new(true),
            table,
        ))
        .build()
        .expect("client");
    let target = format!("http://mirror.test:{port}/f");
    let error = guarded.get(&target).send().await.expect_err("refused");
    assert!(is_refusal(&error), "{error:?}");
    let crate::HttpDownloadError::Failure(failure) = crate::engine::network_failure(error) else {
        panic!("a refusal is a coded failure");
    };
    assert_eq!(
        failure.code.as_deref(),
        Some(rd_core::CODE_INTERNAL_ADDRESS)
    );
    assert_eq!(reached.load(Ordering::SeqCst), 0);

    let unguarded = reqwest::Client::builder()
        .no_proxy()
        .resolve("mirror.test", SocketAddr::from(([127, 0, 0, 1], port)))
        .build()
        .expect("client");
    unguarded.get(&target).send().await.expect("reached");
    assert_eq!(reached.load(Ordering::SeqCst), 1);
}

/// The pool builds the guard into a client whose key carries a policy, and only into that one.
#[tokio::test]
async fn the_pool_builds_the_guard_into_a_client_with_a_policy() {
    let (port, reached) = counting_listener().await;
    let pool = crate::ClientPool::default();
    let context = |address_policy| crate::ClientContext {
        key: crate::ClientKey {
            proxy_profile_id: None,
            account_id: None,
            cookie_ref: None,
            auth_profile_id: None,
            auth_revision: 0,
            replay_scope: None,
            tls_revision: 0,
            address_policy,
        },
        proxy: None,
        proxy_credentials: None,
        cookie_jar: Arc::new(reqwest::cookie::Jar::default()),
        custom_ca_pem: Vec::new(),
        auth: None,
        replay_scope: None,
    };
    let guarded = pool
        .get_or_create(context(Some(AddressPolicy::new(true))))
        .await
        .expect("guarded client");
    let target = format!("http://localhost:{port}/f");
    let error = guarded.get(&target).send().await.expect_err("refused");
    assert!(is_refusal(&error), "{error:?}");
    assert_eq!(reached.load(Ordering::SeqCst), 0);

    // The literal address, because a name may answer with `::1` first and the listener is on
    // IPv4 only. A literal never reaches a resolver; `check_target` is what judges it.
    let ordinary = pool.get_or_create(context(None)).await.expect("client");
    ordinary
        .get(format!("http://127.0.0.1:{port}/f"))
        .send()
        .await
        .expect("reached");
    assert_eq!(reached.load(Ordering::SeqCst), 1);
}
