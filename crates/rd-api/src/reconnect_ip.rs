//! Finding out what our public address is, so a reconnect can tell whether it worked.
//!
//! This is the one part of the feature that talks to somebody outside, which is why the
//! addresses asked are configurable and why the whole feature is off by default. A failed
//! lookup is not an error: it only costs the ability to confirm the change, so the attempt
//! falls back to trusting the script's exit code plus a short settling pause.

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

/// The current public address, or `None` if nobody could be asked.
pub(crate) async fn public_address(configured: &[String]) -> Option<String> {
    let client = reqwest::Client::builder()
        .timeout(LOOKUP_TIMEOUT)
        .user_agent(concat!("rDownloader/", env!("CARGO_PKG_VERSION")))
        .build()
        .ok()?;
    for endpoint in endpoints(configured) {
        if let Some(address) = ask(&client, &endpoint).await {
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
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
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

async fn ask(client: &reqwest::Client, endpoint: &str) -> Option<String> {
    let body = client.get(endpoint).send().await.ok()?.text().await.ok()?;
    parse_address(&body)
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
    use super::{DEFAULT_ENDPOINTS, endpoints, parse_address};

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
