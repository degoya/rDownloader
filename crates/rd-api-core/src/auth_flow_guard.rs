//! What a provider sign-in has to satisfy before the host acts on it (security audit
//! 2026-09-30, findings 5 and web-8).
//!
//! Two checks, both on values a plugin hands the host:
//!
//! * **The callback.** The OAuth redirect arrives from the provider's site, so the session
//!   cookie — `SameSite=Strict` — does not come with it, and a callback that demanded a session
//!   refused every sign-in with the login switched on. The callback is public now, and the
//!   `state` it echoes is the whole credential: stored server-side when the flow started (by a
//!   caller holding `api:secrets`), answered once, within [`CALLBACK_WINDOW`], and never shorter
//!   than [`MIN_CALLBACK_STATE_CHARS`] — a guessable state on a public route would let anybody
//!   bind their own provider account to somebody else's rDownloader account.
//! * **The address the person is sent to.** It is rendered as a link in the interface, so it
//!   has to be `https` — `http` only to this machine, where a test double or a local provider
//!   listens. `rd-plugin-ext` already refuses an undeclared host; this holds for whatever
//!   reaches the flow store, however it got there.

use chrono::{DateTime, Utc};
use rd_core::{AuthFlow, AuthFlowState};

/// The shortest `state` a flow may be started with. A PKCE verifier is 43 characters; 32
/// base64 characters are 192 bits, far beyond guessing over a network.
pub const MIN_CALLBACK_STATE_CHARS: usize = 32;

/// How long after a flow started its callback is still accepted, in seconds, whatever the
/// provider said about its own window.
pub const CALLBACK_WINDOW_SECONDS: i64 = 900;

/// Refuses a callback `state` too short to be a secret.
pub fn checked_callback_state(state: &str) -> anyhow::Result<()> {
    if state.chars().count() < MIN_CALLBACK_STATE_CHARS {
        anyhow::bail!(
            "the plugin's OAuth state is shorter than {MIN_CALLBACK_STATE_CHARS} characters; a \
             callback anybody can reach needs one nobody can guess"
        );
    }
    Ok(())
}

/// Whether a callback for `flow`, taken from the store just now, may still complete it.
pub fn callback_usable(flow: &AuthFlow, now: DateTime<Utc>) -> bool {
    flow.state == AuthFlowState::WaitingForUser
        && !flow.is_expired(now)
        && now - flow.started_at <= chrono::Duration::seconds(CALLBACK_WINDOW_SECONDS)
}

/// The address a sign-in sends the person to, or why it may not be shown as a link.
pub fn checked_sign_in_address(address: String) -> anyhow::Result<String> {
    let url = url::Url::parse(&address)
        .map_err(|_| anyhow::anyhow!("the plugin's sign-in address is not a URL"))?;
    let secure = match url.scheme() {
        "https" => true,
        "http" => match url.host() {
            Some(url::Host::Domain(name)) => name.eq_ignore_ascii_case("localhost"),
            Some(url::Host::Ipv4(address)) => address.is_loopback(),
            Some(url::Host::Ipv6(address)) => address.is_loopback(),
            None => false,
        },
        _ => false,
    };
    if !secure {
        anyhow::bail!("the plugin's sign-in address is not https");
    }
    Ok(address)
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use rd_core::{AuthFlow, AuthFlowState};

    use super::{
        CALLBACK_WINDOW_SECONDS, callback_usable, checked_callback_state, checked_sign_in_address,
    };

    fn flow(started_at: chrono::DateTime<Utc>) -> AuthFlow {
        AuthFlow {
            account_id: rd_core::AccountId::new(),
            plugin_id: "019d0000-0000-7000-8000-0000000001ff".to_owned(),
            state: AuthFlowState::WaitingForUser,
            verification_url: Some("https://accounts.example.com/authorize".to_owned()),
            user_code: None,
            expires_at: None,
            next_poll_at: None,
            message: None,
            started_at,
            token_expires_at: None,
            refresh_ref: None,
            access_ref: None,
            key_ref: None,
            callback_state: None,
            flow_state: None,
        }
    }

    /// Only an address that is safe to render as a link survives.
    #[test]
    fn only_a_secure_address_is_a_sign_in_address() {
        for good in [
            "https://accounts.example.com/authorize?client_id=x",
            "http://127.0.0.1:8080/authorize",
            "http://[::1]:8080/authorize",
            "http://localhost:8080/authorize",
        ] {
            assert!(checked_sign_in_address(good.to_owned()).is_ok(), "{good}");
        }
        for bad in [
            "javascript:alert(1)",
            "data:text/html,<script>alert(1)</script>",
            "http://accounts.example.com/authorize",
            "http://10.0.0.1/authorize",
            "ftp://accounts.example.com/",
            "/relative/path",
            "not a url",
        ] {
            assert!(checked_sign_in_address(bad.to_owned()).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_short_state_is_refused() {
        assert!(checked_callback_state("account-42").is_err());
        assert!(checked_callback_state(&"s".repeat(31)).is_err());
        assert!(checked_callback_state(&"s".repeat(43)).is_ok());
    }

    /// A callback is accepted for a flow still waiting for it, within the window, and only then.
    #[test]
    fn a_callback_is_usable_only_while_its_flow_waits_for_it() {
        let now = Utc::now();
        assert!(callback_usable(&flow(now), now));

        let stale = flow(now - chrono::Duration::seconds(CALLBACK_WINDOW_SECONDS + 1));
        assert!(!callback_usable(&stale, now), "past the host's own window");

        let mut expired = flow(now);
        expired.expires_at = Some(now - chrono::Duration::seconds(1));
        assert!(
            !callback_usable(&expired, now),
            "past the provider's window"
        );

        let mut answered = flow(now);
        answered.state = AuthFlowState::Authorized;
        assert!(!callback_usable(&answered, now), "not waiting any more");
    }
}
