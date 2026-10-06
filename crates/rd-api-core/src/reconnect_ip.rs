//! Finding out what our public address is, so a reconnect can tell whether it worked.
//!
//! This is the one part of the feature that talks to somebody outside, which is why the
//! addresses asked are configurable and why the whole feature is off by default. A failed
//! lookup is not an error: it only costs the ability to confirm the change, so the attempt
//! falls back to trusting the script's exit code plus a short settling pause.
//!
//! A configured address keeps to the rule for an address the person entered (audit
//! 2026-10-05, S8): their own network and a responder on this machine are fine, a link-local
//! address and rDownloader's own listeners are not, every redirect hop is held to the same rule,
//! and only the head of an answer is read.

use std::time::Duration;

/// Asked in order until one answers. Plain-text responders that return the address and
/// nothing else, so there is no parsing to get wrong.
const DEFAULT_ENDPOINTS: [&str; 3] = [
    "https://api.ipify.org",
    "https://ipv4.icanhazip.com",
    "https://checkip.amazonaws.com",
];

/// A lookup is a formality; anything slower than this is not worth waiting for.
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(10);
/// How often the address is re-checked while waiting for it to change.
const POLL_INTERVAL: Duration = Duration::from_secs(5);
/// Given to a router that answered but has not settled, when the address cannot be read.
const SETTLE_PAUSE: Duration = Duration::from_secs(10);
/// How much of an answer is read. An address is at most 45 characters; a responder that sends
/// more is not answering the question, and one that keeps sending must not fill the memory.
const MAX_ANSWER_BYTES: usize = 1024;
/// Redirects followed before a lookup gives up.
const MAX_REDIRECTS: usize = 5;

/// The current public address, or `None` if nobody could be asked.
pub(crate) async fn public_address(configured: &[String]) -> Option<String> {
    for endpoint in endpoints(configured) {
        let Ok(url) = url::Url::parse(&endpoint) else {
            continue;
        };
        let policy = rd_plugin_host::entered_address_policy(&url);
        if let Some(address) = ask(&policy, url).await {
            return Some(address);
        }
    }
    None
}

/// Waits until the public address differs from `before`, within the caller's overall timeout.
///
/// Returns the new address once it changes. When `before` is unknown there is nothing to
/// compare against, so it settles briefly and reports whatever it can read — the caller
/// already knows the script succeeded.
pub(crate) async fn wait_for_change(configured: &[String], before: Option<&str>) -> Option<String> {
    let Some(before) = before else {
        tokio::time::sleep(SETTLE_PAUSE).await;
        return public_address(configured).await;
    };
    // Bounded by the caller's timeout rather than a count: the router decides how long it
    // takes, and the operator decides how long that may be.
    loop {
        tokio::time::sleep(POLL_INTERVAL).await;
        if let Some(current) = public_address(configured).await
            && current != before
        {
            return Some(current);
        }
    }
}

fn endpoints(configured: &[String]) -> Vec<String> {
    let configured: Vec<String> = configured
        .iter()
        .filter_map(|value| crate::input_checks::optional_text(Some(value)))
        .collect();
    if configured.is_empty() {
        DEFAULT_ENDPOINTS
            .iter()
            .map(|value| (*value).to_owned())
            .collect()
    } else {
        configured
    }
}

async fn ask(policy: &rd_http::AddressPolicy, url: url::Url) -> Option<String> {
    // A literal address never reaches the guarded resolver, and a proxy resolves the name
    // itself; both are judged here.
    if let Err(rd_http::TargetRefusal::Refused(refused)) =
        rd_http::check_target(policy, &rd_http::SystemLookup, &url).await
    {
        tracing::warn!(host = %refused.host, "an IP check address is refused by the address rule");
        return None;
    }
    let response = client(policy)?.get(url).send().await.ok()?;
    let head = crate::input_checks::read_body_prefix(response, MAX_ANSWER_BYTES)
        .await
        .ok()?;
    parse_address(&String::from_utf8_lossy(&head))
}

/// A client for one lookup: names resolved through the guard at connect time, and every
/// redirect hop held to the same rule.
fn client(policy: &rd_http::AddressPolicy) -> Option<reqwest::Client> {
    let hops = policy.clone();
    reqwest::Client::builder()
        .timeout(LOOKUP_TIMEOUT)
        .user_agent(rd_core::user_agent!())
        .dns_resolver(rd_http::GuardedResolver::system(policy.clone()))
        .redirect(reqwest::redirect::Policy::custom(move |attempt| {
            if attempt.previous().len() >= MAX_REDIRECTS {
                return attempt.stop();
            }
            match hops.hop_refusal(attempt.url()) {
                Some(refused) => attempt.error(refused),
                None => attempt.follow(),
            }
        }))
        .build()
        .ok()
}

/// Keeps only an answer that is actually an address.
///
/// A captive portal or an error page would otherwise be stored as "the new address" and
/// compared against next time.
fn parse_address(body: &str) -> Option<String> {
    let trimmed = body.trim();
    trimmed
        .parse::<std::net::IpAddr>()
        .ok()
        .map(|address| address.to_string())
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    use super::{DEFAULT_ENDPOINTS, ask, endpoints, parse_address};

    const ADDRESS_ANSWER: &[u8] =
        b"HTTP/1.1 200 OK\r\ncontent-length: 11\r\nconnection: close\r\n\r\n203.0.113.7";

    /// A responder on loopback that answers every connection with `answer` — followed by blank
    /// chunks for as long as the caller reads, when `endless` — and counts its connections.
    async fn responder(answer: Vec<u8>, endless: bool) -> (url::Url, Arc<AtomicUsize>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let address = listener.local_addr().expect("address");
        let taken = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&taken);
        let answer = Arc::new(answer);
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                count.fetch_add(1, Ordering::SeqCst);
                let answer = Arc::clone(&answer);
                tokio::spawn(async move {
                    let mut request = [0_u8; 4096];
                    let _ = stream.read(&mut request).await;
                    if stream.write_all(&answer).await.is_err() {
                        return;
                    }
                    let chunk = format!("1000\r\n{}\r\n", " ".repeat(4096));
                    while endless && stream.write_all(chunk.as_bytes()).await.is_ok() {}
                });
            }
        });
        let url = format!("http://{address}/ip").parse().expect("URL");
        (url, taken)
    }

    /// The address rule holds (audit 2026-10-05, S8): without loopback the responder on this
    /// machine is never connected to, with it — an entered address — it is asked.
    #[tokio::test]
    async fn a_refused_address_is_never_asked() {
        let (url, taken) = responder(ADDRESS_ANSWER.to_vec(), false).await;
        assert_eq!(
            ask(&rd_http::AddressPolicy::new(true), url.clone()).await,
            None
        );
        assert_eq!(taken.load(Ordering::SeqCst), 0);
        let entered = rd_http::AddressPolicy::new(true).with_loopback();
        assert_eq!(ask(&entered, url).await.as_deref(), Some("203.0.113.7"));
        assert_eq!(taken.load(Ordering::SeqCst), 1);
    }

    /// A redirect to one of the service's own ports is not followed.
    #[tokio::test]
    async fn a_redirect_hop_is_held_to_the_rule() {
        let (inner, reached) = responder(ADDRESS_ANSWER.to_vec(), false).await;
        let redirect = format!(
            "HTTP/1.1 302 Found\r\nlocation: {inner}\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
        );
        let (outer, _) = responder(redirect.into_bytes(), false).await;
        let policy = rd_http::AddressPolicy::new(true)
            .with_loopback()
            .refusing_redirects_to(&[inner.port().expect("port")]);
        assert_eq!(ask(&policy, outer).await, None);
        assert_eq!(reached.load(Ordering::SeqCst), 0);
    }

    /// An answer is read only as far as an address can reach: a responder that keeps sending
    /// neither holds the lookup until its timeout nor fills the memory.
    #[tokio::test]
    async fn only_the_head_of_an_answer_is_read() {
        let head = b"HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\nb\r\n203.0.113.7\r\n";
        let (url, _) = responder(head.to_vec(), true).await;
        let started = std::time::Instant::now();
        let entered = rd_http::AddressPolicy::new(true).with_loopback();
        assert_eq!(ask(&entered, url).await.as_deref(), Some("203.0.113.7"));
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }

    #[test]
    fn an_address_survives_the_whitespace_around_it() {
        assert_eq!(
            parse_address("  203.0.113.7\n"),
            Some("203.0.113.7".to_owned())
        );
        assert_eq!(
            parse_address("2001:db8::1\n"),
            Some("2001:db8::1".to_owned())
        );
    }

    #[test]
    fn an_error_page_is_not_an_address() {
        assert_eq!(parse_address("<html>who are you</html>"), None);
        assert_eq!(parse_address(""), None);
        assert_eq!(parse_address("not.an.address"), None);
    }

    #[test]
    fn the_built_in_list_is_used_only_when_nothing_is_configured() {
        assert_eq!(endpoints(&[]).len(), DEFAULT_ENDPOINTS.len());
        assert_eq!(
            endpoints(&["   ".to_owned()]).len(),
            DEFAULT_ENDPOINTS.len()
        );
        assert_eq!(
            endpoints(&["https://mine.example/ip".to_owned()]),
            ["https://mine.example/ip"]
        );
    }
}
