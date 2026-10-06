//! The trace of an unrecognized session page ([`crate::session_trace`]), and the canary that
//! proves its body stays out.
//!
//! The canary page is not a recording. No signed-in page of these sites has been measured; this
//! one carries a title the plugin chooses and, around it, the kinds of value an account page does
//! carry — text, an address, the API key in a form field — each stamped with [`CANARY`], so a
//! line that leaks any of them is visible.
//!
//! A plugin's case runs its own account check against a host built here and hands the result back
//! for the assertions, so the check under test is the plugin's and the expectations are shared.

use plugin_common::{Account, Failure, FailureKind};

use super::{LogHost, html};
use crate::session_trace::unconfirmed_page_line;

/// Stamped on every value of the canary page.
const CANARY: &str = "c4n4ry7f3a";

/// An `account/info` answer for an account whose premium runs until 2099.
const ACCOUNT_INFO: &str = r#"{"status":200,"msg":"OK","result":{"email":"user@example.test","premium_expire":"2099-01-01 00:00:00","traffic_left":"204800"}}"#;

/// One plugin's session-trace cases.
#[derive(Clone, Copy)]
pub struct TraceCase {
    /// The provider name the line opens with.
    pub provider: &'static str,
    /// How the line names the page, such as `the homepage`.
    pub page: &'static str,
    /// The address the plugin asks about the session.
    pub page_url: &'static str,
    /// The canary page's title.
    pub title: &'static str,
    /// The code a cookie-only check reports an unconfirmed session under.
    pub unconfirmed_code: &'static str,
}

impl TraceCase {
    /// A title over a body that settles nothing and carries three canaries.
    #[must_use]
    pub fn canary_page(&self) -> String {
        format!(
            r#"<html><head><title>{}</title></head>
<body><h1>Welcome back</h1><p>Balance for c4n4ry7f3a-account</p>
<span class="mail">c4n4ry7f3a@example.test</span>
<input type="text" name="api_key" value="c4n4ry7f3a-api-key-value" readonly>
<a href="/?op=my_account">My Account</a> <b>Premium</b></body></html>"#,
            self.title
        )
    }

    /// The line names the page's title, length and markers, and nothing else.
    pub fn the_line_names_title_length_and_markers_and_nothing_else(&self) {
        let page = self.canary_page();
        let line = unconfirmed_page_line(self.provider, self.page, &page);
        assert_no_canary(&line);
        assert!(
            line.starts_with(&format!("{}: {} settled nothing", self.provider, self.page)),
            "{line}"
        );
        assert!(
            line.contains(&format!("title \"{}\"", self.title)),
            "{line}"
        );
        assert!(line.contains(&format!("{} bytes", page.len())), "{line}");
        assert!(
            line.ends_with("markers: op=my_account, my account, premium"),
            "{line}"
        );
    }

    /// A page without a title or a marker says so.
    pub fn a_page_without_title_or_markers_says_so(&self) {
        let line = unconfirmed_page_line(self.provider, self.page, "<p>c4n4ry7f3a</p>");
        assert_no_canary(&line);
        assert!(line.contains("no title"), "{line}");
        assert!(line.ends_with("markers: none"), "{line}");
    }

    /// The title is cut to eighty characters and carries no control characters.
    pub fn the_title_is_bounded_and_carries_no_control_characters(&self) {
        let long = format!("<title>{}\u{7}\n tail</title>", "x".repeat(200));
        let line = unconfirmed_page_line(self.provider, self.page, &long);
        assert!(
            line.contains(&format!("title \"{}\"", "x".repeat(80))),
            "{line}"
        );
        assert!(!line.chars().any(char::is_control), "{line:?}");
    }

    /// A session cookie, no credential, and the canary page as the only answer.
    #[must_use]
    pub fn cookie_only_host(&self) -> LogHost {
        LogHost::new(vec![Ok(html(self.page_url, &self.canary_page()))]).with_session()
    }

    /// The cookie-only branch: nothing but the session proves the account, so an unrecognized
    /// page is reported as unconfirmed — retryable, not invalid, not a pass — and traced.
    pub fn assert_cookie_only_unconfirmed(&self, host: &LogHost, failure: &Failure) {
        assert_eq!(failure.kind, FailureKind::Transient(None));
        assert_eq!(failure.code.as_deref(), Some(self.unconfirmed_code));
        assert_eq!(
            host.requests(),
            [self.page_url],
            "{}: the session is asked on this page and no other",
            self.page
        );
        let line = single_warn_line(host);
        assert_no_canary(&line);
        assert!(line.contains(self.title), "{line}");
    }

    /// The API key under `key_reference`, a session cookie, then `account/info` from
    /// `account_info_url` and the canary page.
    #[must_use]
    pub fn proven_key_host(&self, account_info_url: &str, key_reference: &'static str) -> LogHost {
        LogHost::new(vec![
            Ok(html(account_info_url, ACCOUNT_INFO)),
            Ok(html(self.page_url, &self.canary_page())),
        ])
        .with_session()
        .with_secret(key_reference)
    }

    /// The `api_key` branch: the key proved the account, the check passes with the session
    /// labelled `session_unconfirmed`, and the page that settled nothing is traced the same way.
    pub fn assert_proven_key_unconfirmed(
        &self,
        host: &LogHost,
        account: &Account,
        session_unconfirmed: &str,
    ) {
        assert!(account.valid);
        assert!(account.premium, "premium comes from the API");
        let codes: Vec<&str> = account
            .label
            .iter()
            .map(|part| part.code.as_str())
            .collect();
        assert!(codes.contains(&session_unconfirmed), "{codes:?}");
        assert!(
            !codes.contains(&"plugin.account.session_active"),
            "nothing confirmed the session, so nothing claims it: {codes:?}"
        );
        let line = single_warn_line(host);
        assert_no_canary(&line);
        assert!(line.contains("markers: op=my_account"), "{line}");
    }

    /// A session cookie and a page carrying the sign-out link.
    #[must_use]
    pub fn recognized_host(&self) -> LogHost {
        let page = format!(
            r#"<title>{}</title><a href="/?op=logout">Logout</a>"#,
            self.provider
        );
        LogHost::new(vec![Ok(html(self.page_url, &page))]).with_session()
    }
}

/// A page that settles the question has nothing to trace.
pub fn assert_nothing_written(host: &LogHost) {
    assert!(host.logs().is_empty());
}

/// The one warn line the check left, after asserting there is exactly one.
fn single_warn_line(host: &LogHost) -> String {
    let logs = host.logs();
    assert_eq!(logs.len(), 1, "exactly one line: {logs:?}");
    assert_eq!(logs[0].0, "warn");
    logs[0].1.clone()
}

fn assert_no_canary(line: &str) {
    assert!(
        !line.to_ascii_lowercase().contains(CANARY),
        "the page body must not reach the log: {line}"
    );
}
