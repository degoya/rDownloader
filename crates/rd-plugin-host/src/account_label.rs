//! The account label as a component reports it, checked and bounded on its way to the core.
//!
//! `account-status.label` is a list of `label-part { code, params, message }` records, the
//! shape `failure` already travels in. The code is the channel: the interface translates it in
//! the active language, then in English, and prints `message` only when no catalogue knows the
//! code. A part *without* a code would make the English text the channel again, which is the
//! second path RD-110-28 decided against -- so the host refuses it instead of passing it on.

use rd_core::{Failure, FailureKind};
use rd_plugin_api::LabelPart;

use crate::component::rdownloader::plugin::types as wit_types;

/// Parts beyond this are dropped: a label is a line next to an account, not a report.
const MAX_PARTS: usize = 8;
/// The same bounds `failure` codes and parameters are held to.
const MAX_CODE_LEN: usize = 128;
const MAX_PARAMS: usize = 16;
const MAX_PARAM_NAME_LEN: usize = 64;
const MAX_PARAM_VALUE_LEN: usize = 512;
const MAX_MESSAGE_LEN: usize = 512;

/// Converts the guest's label parts, refusing any part that has no code.
///
/// # Errors
///
/// `plugin.account_label_invalid` when a part's code is empty or longer than a failure code
/// may be: the plugin tried to send free text where the contract asks for a code.
pub(crate) fn from_wit_label(parts: Vec<wit_types::LabelPart>) -> Result<Vec<LabelPart>, Failure> {
    parts
        .into_iter()
        .take(MAX_PARTS)
        .map(from_wit_part)
        .collect()
}

fn from_wit_part(part: wit_types::LabelPart) -> Result<LabelPart, Failure> {
    if part.code.is_empty() || part.code.len() > MAX_CODE_LEN {
        return Err(Failure::coded(
            FailureKind::Permanent,
            "plugin.account_label_invalid",
            "The plugin sent an account label part without a translation code",
        ));
    }
    let mut message = part.message;
    if message.len() > MAX_MESSAGE_LEN {
        let cut = (0..=MAX_MESSAGE_LEN)
            .rev()
            .find(|index| message.is_char_boundary(*index))
            .unwrap_or(0);
        message.truncate(cut);
    }
    Ok(LabelPart {
        code: part.code,
        params: part
            .params
            .into_iter()
            .take(MAX_PARAMS)
            .filter(|(key, value)| {
                key.len() <= MAX_PARAM_NAME_LEN && value.len() <= MAX_PARAM_VALUE_LEN
            })
            .collect(),
        message,
    })
}

#[cfg(test)]
mod tests {
    use super::{MAX_PARTS, from_wit_label, wit_types};

    fn part(code: &str, message: &str) -> wit_types::LabelPart {
        wit_types::LabelPart {
            code: code.to_owned(),
            params: vec![("user".to_owned(), "alice".to_owned())],
            message: message.to_owned(),
        }
    }

    #[test]
    fn a_coded_part_arrives_with_its_parameters_and_text() {
        let parts = from_wit_label(vec![part("plugin.account.user", "Signed in as alice")])
            .expect("a coded part is accepted");
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].code, "plugin.account.user");
        assert_eq!(
            parts[0].params.get("user").map(String::as_str),
            Some("alice")
        );
        assert_eq!(parts[0].message, "Signed in as alice");
    }

    #[test]
    fn a_part_without_a_code_is_refused_rather_than_printed() {
        // The English text must never become the channel: a plugin that sends only prose gets
        // a failure it can read in its own tests, not a label the user sees verbatim.
        let error = from_wit_label(vec![part("", "Premium until 2027-01-01")])
            .expect_err("free text without a code is a contract violation");
        assert_eq!(error.code.as_deref(), Some("plugin.account_label_invalid"));
    }

    #[test]
    fn an_overlong_code_is_refused_like_a_missing_one() {
        let error = from_wit_label(vec![part(&"x".repeat(129), "")]).expect_err("too long");
        assert_eq!(error.code.as_deref(), Some("plugin.account_label_invalid"));
    }

    #[test]
    fn an_empty_label_is_a_valid_answer() {
        assert!(
            from_wit_label(Vec::new())
                .expect("nothing to say")
                .is_empty()
        );
    }

    #[test]
    fn parts_parameters_and_text_are_bounded() {
        let mut oversized = part("plugin.account.user", &"m".repeat(600));
        oversized.params = (0..20)
            .map(|index| (format!("p{index}"), "v".to_owned()))
            .chain(std::iter::once(("long".to_owned(), "v".repeat(513))))
            .collect();
        let parts = from_wit_label(std::iter::repeat_n(oversized, MAX_PARTS + 2).collect())
            .expect("bounded, not refused");
        assert_eq!(parts.len(), MAX_PARTS);
        assert_eq!(parts[0].params.len(), 16);
        assert!(!parts[0].params.contains_key("long"));
        assert_eq!(parts[0].message.len(), 512);
    }
}
