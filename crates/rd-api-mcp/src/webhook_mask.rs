//! Webhook addresses that keep their secret in the path (RD-1190-21).
//!
//! `rd_core::redact_url` masks user info and signed query parameters. A Slack, Discord or Teams
//! webhook needs neither: whoever knows the path may post to the channel. Two masks close that:
//!
//! * [`mask_known`], applied by the one mask over every answer (`super::mask`), recognises the
//!   services that are known to keep a secret in the path, wherever such an address turns up --
//!   a destination, a log line, a delivery's error.
//! * [`mask_path`], applied by the notification tools to every destination address they answer
//!   with, hides the path and query of any service, a self-hosted one included, because a
//!   destination's address is where such a secret lives by design.

use rd_core::REDACTION_PLACEHOLDER;
use url::{Position, Url};

/// Hosts (and their subdomains) whose webhook addresses carry the secret in the path, and the
/// path prefix after which it starts.
const PATH_SECRET_HOOKS: &[(&str, &str)] = &[
    ("api.telegram.org", "/bot"),
    ("discord.com", "/api/webhooks/"),
    ("discordapp.com", "/api/webhooks/"),
    ("hooks.slack.com", "/"),
    ("webhook.office.com", "/"),
];

/// The address with everything after its prefix masked, if it is a known webhook address.
pub(crate) fn mask_known(address: &Url) -> Option<String> {
    let host = address.host_str()?.to_ascii_lowercase();
    let path = address.path();
    PATH_SECRET_HOOKS.iter().find_map(|(service, prefix)| {
        let ours = host == *service || host.ends_with(&format!(".{service}"));
        (ours && path.len() > prefix.len() && path.starts_with(prefix)).then(|| {
            format!(
                "{}{prefix}{REDACTION_PLACEHOLDER}",
                &address[..Position::BeforePath]
            )
        })
    })
}

/// The address with its whole path and query masked, or `None` when it has neither.
pub(crate) fn mask_path(address: &Url) -> Option<String> {
    let rest = &address[Position::BeforePath..];
    if rest.is_empty() || rest == "/" {
        return None;
    }
    Some(format!(
        "{}/{REDACTION_PLACEHOLDER}",
        &address[..Position::BeforePath]
    ))
}

/// `text` with every known webhook address in it masked, or `None` when it holds none.
pub(crate) fn mask_in_text(text: &str) -> Option<String> {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    let mut changed = false;
    while let Some(start) = next_address(rest) {
        out.push_str(&rest[..start]);
        let candidate = &rest[start..];
        let end = candidate
            .find(|c: char| c.is_whitespace() || "\"'<>()[]{}`\\".contains(c))
            .unwrap_or(candidate.len());
        let address = candidate[..end].trim_end_matches(['.', ',', ';', ':', '!', '?']);
        match Url::parse(address).ok().as_ref().and_then(mask_known) {
            Some(masked) => {
                out.push_str(&masked);
                changed = true;
            }
            None => out.push_str(address),
        }
        rest = &candidate[address.len()..];
    }
    out.push_str(rest);
    changed.then_some(out)
}

/// Where the next `http://` or `https://` address in `text` starts.
fn next_address(text: &str) -> Option<usize> {
    [text.find("https://"), text.find("http://")]
        .into_iter()
        .flatten()
        .min()
}

#[cfg(test)]
mod tests {
    use url::Url;

    use super::{mask_in_text, mask_known, mask_path};

    const SECRET: &str = "Xq7pLm2Rk9";

    fn known(address: &str) -> Option<String> {
        mask_known(&Url::parse(address).expect("an address"))
    }

    #[test]
    fn the_known_services_lose_the_path_after_their_prefix() {
        for (address, kept) in [
            (
                format!("https://discord.com/api/webhooks/123/{SECRET}"),
                "https://discord.com/api/webhooks/[redacted]",
            ),
            (
                format!("https://ptb.discord.com/api/webhooks/123/{SECRET}"),
                "https://ptb.discord.com/api/webhooks/[redacted]",
            ),
            (
                format!("https://hooks.slack.com/services/T0/B0/{SECRET}"),
                "https://hooks.slack.com/[redacted]",
            ),
            (
                format!("https://contoso.webhook.office.com/webhookb2/{SECRET}"),
                "https://contoso.webhook.office.com/[redacted]",
            ),
            (
                format!("https://api.telegram.org/bot{SECRET}/sendMessage"),
                "https://api.telegram.org/bot[redacted]",
            ),
        ] {
            assert_eq!(known(&address).as_deref(), Some(kept), "{address}");
        }
    }

    #[test]
    fn another_address_and_a_masked_one_stay() {
        assert_eq!(known("https://discord.com/invite/abc"), None);
        assert_eq!(known("https://example.com/hooks/services/x"), None);
        assert_eq!(known("https://notdiscord.com/api/webhooks/1/x"), None);
        assert_eq!(known("https://discord.com/api/webhooks/"), None);
        let masked = mask_in_text("https://hooks.slack.com/[redacted]");
        assert_eq!(masked, None, "masking twice changes nothing");
    }

    #[test]
    fn an_address_inside_prose_is_found_and_the_rest_kept() {
        let text = format!(
            "error sending request for url (https://discord.com/api/webhooks/9/{SECRET}): timed out."
        );
        let masked = mask_in_text(&text).expect("masked");
        assert!(!masked.contains(SECRET), "{masked}");
        assert_eq!(
            masked,
            "error sending request for url (https://discord.com/api/webhooks/[redacted]): timed out."
        );
        assert_eq!(
            mask_in_text("see https://example.com/a and http://x.test."),
            None
        );
        assert_eq!(mask_in_text("nothing here"), None);
    }

    #[test]
    fn any_destination_address_loses_path_and_query() {
        let masked = |address: &str| mask_path(&Url::parse(address).expect("an address"));
        assert_eq!(
            masked(&format!("https://chat.example.org/hooks/{SECRET}")).as_deref(),
            Some("https://chat.example.org/[redacted]")
        );
        assert_eq!(
            masked(&format!("http://ha.local:8123/api/webhook/x?k={SECRET}")).as_deref(),
            Some("http://ha.local:8123/[redacted]")
        );
        assert_eq!(masked("https://ntfy.example.org"), None);
        assert_eq!(masked("https://ntfy.example.org/"), None);
    }
}
