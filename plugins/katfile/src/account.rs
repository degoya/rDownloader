//! The account label both builds render once an API key confirmed the account.
//!
//! This module used to carry a clock-free `is_premium`, which decided premium from the *shape*
//! of `premium_expire` rather than from the date it holds, because the WebAssembly guest had no
//! clock and the two builds had to agree. They agreed on the wrong answer: an account whose
//! premium lapsed still reported premium. The host now offers `now-unix-seconds`, so the real
//! comparison happens in `crate::resolver` and only the cosmetic part is left here.

use plugin_common::Label;

/// The address the key confirmed, then the cookie count, since only cookies (not the API
/// key) can perform a download — and, when a request proved the session live, that it did.
///
/// `session_verified` is a measurement, never an assumption: the count alone was what made a
/// green check meaningless, because eight cookies the site had forgotten look exactly like
/// eight that work (RD-120-13).
pub(crate) fn account_label(email: &str, cookies: usize, session_verified: bool) -> Label {
    let label = Label::new().user(Some(email)).cookies(cookies);
    if session_verified {
        label.session_active()
    } else {
        label
    }
}

#[cfg(test)]
mod tests {
    use super::account_label;

    #[test]
    fn label_states_the_address_then_the_cookie_count() {
        let parts = account_label("user@example.test", 3, false).into_parts();
        let codes: Vec<&str> = parts.iter().map(|part| part.code.as_str()).collect();
        assert_eq!(codes, ["plugin.account.user", "plugin.account.cookies"]);
        assert_eq!(parts[1].params, vec![("count".to_owned(), "3".to_owned())]);
    }

    /// A verified session is said, not implied by a number.
    #[test]
    fn a_verified_session_is_named_in_the_label() {
        let parts = account_label("user@example.test", 3, true).into_parts();
        let codes: Vec<&str> = parts.iter().map(|part| part.code.as_str()).collect();
        assert_eq!(
            codes,
            [
                "plugin.account.user",
                "plugin.account.cookies",
                "plugin.account.session_active"
            ]
        );
    }
}
