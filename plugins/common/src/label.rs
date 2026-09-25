//! The account label as translatable parts, and the parts every plugin says the same way.
//!
//! `account-status.label` is a list of `{ code, params, message }` records the interface
//! translates one by one and joins with a separator of its own. The codes here are the core's
//! (`plugin.account.*`), carried by `web/src/locales/{de,en,es,fr}/server.json`, so no plugin
//! formulates "Premium until ..." itself and none has to translate it. A provider-specific
//! part uses [`LabelPart::coded`] with a code from the plugin's own `<slug>.` namespace and
//! its own catalogue (RD-110-28).

/// Longest user name or address a label carries; a provider-supplied string on its way into
/// the accounts list is shown, not parsed, so it is bounded and stripped of controls.
const MAX_USER_LEN: usize = 80;

/// `Signed in as {user}`.
pub const USER: &str = "plugin.account.user";
/// `Premium until {until}`, the date as the provider states it.
pub const PREMIUM_UNTIL: &str = "plugin.account.premium_until";
/// A subscription that never runs out.
pub const PREMIUM_LIFETIME: &str = "plugin.account.premium_lifetime";
/// A subscription the provider reports as run out.
pub const PREMIUM_EXPIRED: &str = "plugin.account.premium_expired";
/// The check proved the credentials and read nothing about the subscription (RD-109-34).
pub const PREMIUM_UNCHECKED: &str = "plugin.account.premium_unchecked";
/// `{count}` cookies loaded for downloads; zero says a cookie session is required.
pub const COOKIES: &str = "plugin.account.cookies";
/// The plugin signed in with the stored credentials during this check.
pub const SIGNED_IN: &str = "plugin.account.signed_in";
/// The check found the session already established and left it alone.
pub const SESSION_ACTIVE: &str = "plugin.account.session_active";

/// One translatable part of an account label.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LabelPart {
    /// `plugin.account.*` or `<provider.slug>.*`; never empty, the host refuses that.
    pub code: String,
    /// Flat parameters the translated text references.
    pub params: Vec<(String, String)>,
    /// English, redaction-safe text for a code no catalogue translates.
    pub message: String,
}

impl LabelPart {
    /// A provider-specific part: the code from the plugin's own namespace and its English text.
    #[must_use]
    pub fn coded(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_owned(),
            params: Vec::new(),
            message: message.into(),
        }
    }

    /// Adds one parameter the translated text can reference.
    #[must_use]
    pub fn with_param(mut self, name: &str, value: impl Into<String>) -> Self {
        self.params.push((name.to_owned(), value.into()));
        self
    }
}

/// Builds a label from the parts a check found, in the order it found them.
///
/// Every method that takes an `Option` adds nothing for `None` or a blank value, so a plugin
/// can write the label in one expression without deciding per field whether it has a value.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Label {
    parts: Vec<LabelPart>,
}

impl Label {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// `Signed in as {user}` for a non-blank name or address, bounded and stripped of
    /// control characters.
    #[must_use]
    pub fn user(mut self, name: Option<&str>) -> Self {
        let cleaned: String = name
            .unwrap_or_default()
            .trim()
            .chars()
            .filter(|character| !character.is_control())
            .take(MAX_USER_LEN)
            .collect();
        if !cleaned.is_empty() {
            self.parts.push(
                LabelPart::coded(USER, format!("Signed in as {cleaned}"))
                    .with_param("user", cleaned),
            );
        }
        self
    }

    /// `Premium until {until}` for a non-blank date.
    #[must_use]
    pub fn premium_until(mut self, until: Option<&str>) -> Self {
        if let Some(until) = until.map(str::trim).filter(|value| !value.is_empty()) {
            self.parts.push(
                LabelPart::coded(PREMIUM_UNTIL, format!("Premium until {until}"))
                    .with_param("until", until),
            );
        }
        self
    }

    #[must_use]
    pub fn premium_lifetime(mut self) -> Self {
        self.parts
            .push(LabelPart::coded(PREMIUM_LIFETIME, "Lifetime premium"));
        self
    }

    #[must_use]
    pub fn premium_expired(mut self) -> Self {
        self.parts
            .push(LabelPart::coded(PREMIUM_EXPIRED, "Premium expired"));
        self
    }

    /// Says that `premium: false` next to it was never measured, not found to be false.
    #[must_use]
    pub fn premium_unchecked(mut self) -> Self {
        self.parts.push(LabelPart::coded(
            PREMIUM_UNCHECKED,
            "the subscription was not checked",
        ));
        self
    }

    /// How many cookies the session holds; zero states that downloads need a cookie session.
    #[must_use]
    pub fn cookies(mut self, count: usize) -> Self {
        let message = match count {
            0 => "no cookies loaded - downloads need a cookie session".to_owned(),
            1 => "1 cookie loaded".to_owned(),
            count => format!("{count} cookies loaded"),
        };
        self.parts
            .push(LabelPart::coded(COOKIES, message).with_param("count", count.to_string()));
        self
    }

    #[must_use]
    pub fn signed_in(mut self) -> Self {
        self.parts.push(LabelPart::coded(
            SIGNED_IN,
            "signed in with the stored credentials",
        ));
        self
    }

    #[must_use]
    pub fn session_active(mut self) -> Self {
        self.parts
            .push(LabelPart::coded(SESSION_ACTIVE, "session still signed in"));
        self
    }

    /// A provider-specific part, built with [`LabelPart::coded`].
    #[must_use]
    pub fn part(mut self, part: LabelPart) -> Self {
        self.parts.push(part);
        self
    }

    /// Adds `part` when it is `Some`.
    #[must_use]
    pub fn maybe(self, part: Option<LabelPart>) -> Self {
        match part {
            Some(part) => self.part(part),
            None => self,
        }
    }

    #[must_use]
    pub fn into_parts(self) -> Vec<LabelPart> {
        self.parts
    }
}

impl From<Label> for Vec<LabelPart> {
    fn from(label: Label) -> Self {
        label.into_parts()
    }
}

#[cfg(test)]
mod tests {
    use super::{COOKIES, Label, LabelPart, PREMIUM_UNTIL, USER};

    fn codes(label: Label) -> Vec<String> {
        label
            .into_parts()
            .into_iter()
            .map(|part| part.code)
            .collect()
    }

    #[test]
    fn a_user_part_carries_the_name_as_a_parameter_and_in_its_text() {
        let parts = Label::new().user(Some("  alice ")).into_parts();
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].code, USER);
        assert_eq!(
            parts[0].params,
            vec![("user".to_owned(), "alice".to_owned())]
        );
        assert_eq!(parts[0].message, "Signed in as alice");
    }

    #[test]
    fn blank_values_add_nothing() {
        assert!(
            Label::new()
                .user(None)
                .user(Some("  "))
                .into_parts()
                .is_empty()
        );
        assert!(
            Label::new()
                .premium_until(None)
                .premium_until(Some(""))
                .into_parts()
                .is_empty()
        );
    }

    #[test]
    fn a_user_name_is_bounded_and_stripped_of_controls() {
        let long = format!("a\u{7}b{}", "c".repeat(100));
        let parts = Label::new().user(Some(&long)).into_parts();
        let (_, user) = &parts[0].params[0];
        assert_eq!(user.len(), 80);
        assert!(!user.contains('\u{7}'));
        assert!(user.starts_with("ab"));
    }

    #[test]
    fn parts_keep_the_order_they_were_added_in() {
        let label = Label::new()
            .user(Some("bob"))
            .premium_until(Some("2027-01-01"))
            .cookies(3)
            .premium_unchecked();
        assert_eq!(
            codes(label),
            vec![
                USER,
                PREMIUM_UNTIL,
                COOKIES,
                "plugin.account.premium_unchecked"
            ]
        );
    }

    #[test]
    fn the_cookie_count_is_a_parameter_the_catalogue_pluralises() {
        for (count, text) in [
            (0, "no cookies loaded - downloads need a cookie session"),
            (1, "1 cookie loaded"),
            (3, "3 cookies loaded"),
        ] {
            let parts = Label::new().cookies(count).into_parts();
            assert_eq!(
                parts[0].params,
                vec![("count".to_owned(), count.to_string())]
            );
            assert_eq!(parts[0].message, text);
        }
    }

    #[test]
    fn a_provider_part_keeps_its_own_code_and_parameters() {
        let part =
            LabelPart::coded("fastshare.account.tier", "Tier gold").with_param("tier", "gold");
        let parts = Label::new()
            .part(part.clone())
            .maybe(None)
            .maybe(Some(part))
            .into_parts();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].code, "fastshare.account.tier");
        assert_eq!(
            parts[0].params,
            vec![("tier".to_owned(), "gold".to_owned())]
        );
    }
}
