//! RD-1420-01: a port refused on this machine alone is checked on the addresses a name resolves
//! to when the connection is made — a redirect hop's too — and nowhere else.

use std::{
    net::{IpAddr, Ipv4Addr},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use axum::{Router, response::Redirect, routing::get};

use crate::{AddressPolicy, GuardedResolver, HostLookup, LookupFuture};

/// `app.test` answers with loopback, as a name pointing at this machine does.
struct Loopback;

impl HostLookup for Loopback {
    fn lookup<'a>(&'a self, host: &'a str) -> LookupFuture<'a> {
        let answer = if host == "app.test" {
            vec![IpAddr::V4(Ipv4Addr::LOCALHOST)]
        } else {
            Vec::new()
        };
        Box::pin(async move { Ok(answer) })
    }
}

/// A guarded client as the pool builds one, loopback permitted, `port` refused on this machine.
fn client(port: u16) -> reqwest::Client {
    let policy = AddressPolicy::new(true)
        .with_loopback()
        .refusing_redirects_on_this_machine(&[port], None);
    let builder = crate::client_builder(crate::ClientPurpose::Direct).dns_resolver(
        GuardedResolver::with_lookup(policy.clone(), Arc::new(Loopback)),
    );
    super::refusing_this_machine(builder, &policy)
        .build()
        .expect("client")
}

#[tokio::test]
async fn a_name_resolving_here_is_refused_on_the_port_and_reached_on_any_other() {
    let hits = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&hits);
    let guarded = rd_testkit::serve(Router::new().fallback(move || {
        counted.fetch_add(1, Ordering::SeqCst);
        async { "the service behind the proxy" }
    }))
    .await;
    let port = guarded.addr().port();
    let hop = format!("http://app.test:{port}/api/v1/settings");
    let front = rd_testkit::serve(Router::new().route("/", get(|| async { "ok" })).route(
        "/start",
        get(move || {
            let hop = hop.clone();
            async move { Redirect::temporary(&hop) }
        }),
    ))
    .await;
    let front = front.addr().port();
    let client = client(port);

    let body = client
        .get(format!("http://app.test:{front}/"))
        .send()
        .await
        .expect("another port on this machine")
        .text()
        .await
        .expect("body");
    assert_eq!(body, "ok");

    for target in [
        format!("http://app.test:{port}/"),
        format!("http://app.test:{front}/start"),
    ] {
        let error = client.get(&target).send().await.expect_err("refused");
        assert!(crate::is_refusal(&error), "{target}: {error:?}");
    }
    assert_eq!(hits.load(Ordering::SeqCst), 0, "the port was never reached");
}
