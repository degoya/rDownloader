//! The steps a rule may take, as data.
//!
//! The seven kinds are the ones RD-110-05 names; what each does at run time is that job's
//! to decide, and it may add fields. This module fixes the shape and checks what can be
//! checked without a network: a pattern compiles, a variable name is well-formed, a template
//! closes every placeholder it opens. A rule that fails here is refused at load, not at the
//! first page it is asked about.
//!
//! **Variables.** A step reads from and writes to named variables. `fetch` writes the page
//! into `page` unless told otherwise; `regex` reads from `page` unless told otherwise; a
//! template such as a `url` field may embed `${name}`. The link list a rule produces is the
//! variable `links` when the last step has run.

use std::collections::BTreeMap;

use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};

use crate::{
    format::RuleError,
    text::{is_slug, is_variable, template_variables},
};

/// The variable `fetch` writes and `regex` reads when neither names one.
pub const PAGE_VARIABLE: &str = "page";
/// The variable the executor reads the result from.
pub const LINKS_VARIABLE: &str = "links";
/// Most steps a rule may take, and its `groups` again (PL-09). The shipped rules take six at
/// most; the run's own limits bound what the steps fetch, this bounds what is checked and
/// compiled before anything is.
pub const MAX_STEPS: usize = 32;
/// Longest regular expression a rule may carry, in characters (PL-09). The shipped rules'
/// longest has 63.
pub const MAX_PATTERN_LENGTH: usize = 1024;
/// Most memory one compiled pattern may take (PL-09). The `regex` crate's default is 10 MiB; a
/// few characters of a repeated Unicode class (`\w{60}`) already take more than this.
const PATTERN_SIZE_LIMIT: usize = 2 * 1024 * 1024;

/// One step of a rule.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Step {
    /// Fetches a page as text. `url` defaults to the claimed address. When a placeholder in
    /// `url` holds a list, one page per entry, and `into` holds the list of bodies.
    Fetch {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        url: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        into: Option<String>,
    },
    /// Fetches JSON and takes the value at `path`, a JSON pointer (RFC 6901). When a
    /// placeholder in `url` holds a list, once per entry, the values flattened in order.
    FetchJson {
        url: String,
        path: String,
        into: String,
    },
    /// Applies a pattern to a variable. The first capture group is the value; with `all`,
    /// every match is taken and the variable becomes a list.
    Regex {
        pattern: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from: Option<String>,
        into: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        all: bool,
    },
    /// Decodes a variable.
    Decode {
        encoding: Decoding,
        from: String,
        into: String,
    },
    /// Submits a form and keeps the response. With `json` (RD-1170-03) the fields go out as
    /// one JSON object of strings rather than as `application/x-www-form-urlencoded`, which is
    /// what an API behind a page's script reads.
    Form {
        url: String,
        #[serde(default)]
        fields: BTreeMap<String, String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        into: Option<String>,
        /// Absent in every rule written before this existed, and absent in the serialized form
        /// when false, so a rule written before it stays byte-identical.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        json: bool,
    },
    /// Follows the redirect an address answers with and keeps the target.
    Redirect { from: String, into: String },
    /// Hands a challenge to the captcha broker. `challenge` names the challenge kind, such
    /// as `recaptcha-v2`; `sitekey` is a template.
    Captcha {
        challenge: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sitekey: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        into: Option<String>,
        /// The page the widget sits on, a template; the page in hand unless named
        /// (RD-1170-03). A rule that read an API after the page names the page here, since the
        /// person solves the challenge on the page and not on the API's answer.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        page: Option<String>,
        /// Whether the widget is an invisible one, which a solver service is told.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        invisible: bool,
    },
}

/// What `decode` understands.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Decoding {
    Base64,
    Hex,
    Rot13,
    Url,
    /// Concatenated JavaScript string literals, `"a" + 'b'`.
    JsString,
}

impl Step {
    /// The kind name as it appears in JSON, for messages.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Fetch { .. } => "fetch",
            Self::FetchJson { .. } => "fetch-json",
            Self::Regex { .. } => "regex",
            Self::Decode { .. } => "decode",
            Self::Form { .. } => "form",
            Self::Redirect { .. } => "redirect",
            Self::Captcha { .. } => "captcha",
        }
    }

    /// Refuses a step whose static parts cannot work.
    pub fn validate(&self) -> Result<(), RuleError> {
        match self {
            Self::Fetch { url, into } => {
                check_optional_template(url.as_deref())?;
                check_optional_variable(into.as_deref())
            }
            Self::FetchJson { url, path, into } => {
                check_template(url)?;
                if !path.is_empty() && !path.starts_with('/') {
                    return Err(RuleError::JsonPointer(path.clone()));
                }
                check_variable(into)
            }
            Self::Regex {
                pattern,
                from,
                into,
                ..
            } => {
                check_pattern(pattern)?;
                check_optional_variable(from.as_deref())?;
                check_variable(into)
            }
            Self::Decode { from, into, .. } => {
                check_variable(from)?;
                check_variable(into)
            }
            Self::Form {
                url, fields, into, ..
            } => {
                check_template(url)?;
                for value in fields.values() {
                    check_template(value)?;
                }
                check_optional_variable(into.as_deref())
            }
            Self::Redirect { from, into } => {
                check_variable(from)?;
                check_variable(into)
            }
            Self::Captcha {
                challenge,
                sitekey,
                into,
                page,
                ..
            } => {
                if !is_slug(challenge, 32) {
                    return Err(RuleError::CaptchaKind(challenge.clone()));
                }
                check_optional_template(sitekey.as_deref())?;
                check_optional_template(page.as_deref())?;
                check_optional_variable(into.as_deref())
            }
        }
    }
}

/// Compiles a rule's pattern within [`PATTERN_SIZE_LIMIT`]: what validation proves is what
/// the executor compiles again.
pub(crate) fn compile_pattern(pattern: &str) -> Result<Regex, regex::Error> {
    RegexBuilder::new(pattern)
        .size_limit(PATTERN_SIZE_LIMIT)
        .build()
}

/// Compiles `pattern` once to prove it can be compiled, refusing one longer than
/// [`MAX_PATTERN_LENGTH`] before it is.
pub(crate) fn check_pattern(pattern: &str) -> Result<(), RuleError> {
    if pattern.chars().count() > MAX_PATTERN_LENGTH {
        return Err(RuleError::PatternLength(MAX_PATTERN_LENGTH));
    }
    compile_pattern(pattern)
        .map(drop)
        .map_err(|error| RuleError::Pattern {
            pattern: pattern.to_owned(),
            reason: error.to_string(),
        })
}

pub(crate) fn check_variable(name: &str) -> Result<(), RuleError> {
    if is_variable(name) {
        Ok(())
    } else {
        Err(RuleError::Variable(name.to_owned()))
    }
}

fn check_optional_variable(name: Option<&str>) -> Result<(), RuleError> {
    name.map_or(Ok(()), check_variable)
}

pub(crate) fn check_template(template: &str) -> Result<(), RuleError> {
    template_variables(template)
        .map(drop)
        .ok_or_else(|| RuleError::Template(template.to_owned()))
}

fn check_optional_template(template: Option<&str>) -> Result<(), RuleError> {
    template.map_or(Ok(()), check_template)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> Result<Step, serde_json::Error> {
        serde_json::from_str(json)
    }

    #[test]
    fn every_kind_reads_from_its_json_form() {
        let steps = [
            r#"{"kind":"fetch"}"#,
            r#"{"kind":"fetch-json","url":"${base}/api","path":"/items","into":"items"}"#,
            r#"{"kind":"regex","pattern":"href=\"(.+?)\"","into":"links","all":true}"#,
            r#"{"kind":"decode","encoding":"base64","from":"raw","into":"links"}"#,
            r#"{"kind":"form","url":"${page_url}","fields":{"go":"1"}}"#,
            r#"{"kind":"redirect","from":"links","into":"links"}"#,
            r#"{"kind":"captcha","challenge":"recaptcha-v2","extra":"x"}"#,
        ];
        let kinds: Vec<_> = steps
            .iter()
            .map(|json| parse(json).map(|step| step.kind()))
            .collect();
        assert_eq!(kinds[0].as_deref().ok(), Some("fetch"));
        assert_eq!(kinds[1].as_deref().ok(), Some("fetch-json"));
        assert_eq!(kinds[2].as_deref().ok(), Some("regex"));
        assert_eq!(kinds[3].as_deref().ok(), Some("decode"));
        assert_eq!(kinds[4].as_deref().ok(), Some("form"));
        assert_eq!(kinds[5].as_deref().ok(), Some("redirect"));
        // The seventh carries a field this build does not know, and that is refused.
        assert!(kinds[6].is_err());
        for step in steps.iter().take(6) {
            parse(step).expect("step").validate().expect("valid");
        }
    }

    #[test]
    fn a_captcha_step_names_its_challenge_kind() {
        let step = parse(r#"{"kind":"captcha","challenge":"recaptcha-v2","sitekey":"${key}"}"#)
            .expect("parse");
        step.validate().expect("valid");
        let step = parse(r#"{"kind":"captcha","challenge":"reCAPTCHA"}"#).expect("parse");
        assert!(matches!(step.validate(), Err(RuleError::CaptchaKind(_))));
    }

    #[test]
    fn a_json_form_and_a_captcha_page_are_optional_and_left_out_when_unused() {
        let form = r#"{"kind":"form","url":"${api}","fields":{"token":"${captcha}"}}"#;
        let step = parse(form).expect("parse");
        assert_eq!(serde_json::to_string(&step).expect("encode"), form);
        let json = r#"{"kind":"form","url":"${api}","fields":{"token":"${captcha}"},"json":true}"#;
        let step = parse(json).expect("parse");
        assert!(matches!(step, Step::Form { json: true, .. }));
        assert_eq!(serde_json::to_string(&step).expect("encode"), json);
        let captcha = r#"{"kind":"captcha","challenge":"recaptcha-v2","sitekey":"${key}","page":"${url}","invisible":true}"#;
        let step = parse(captcha).expect("parse");
        step.validate().expect("valid");
        assert_eq!(serde_json::to_string(&step).expect("encode"), captcha);
        let step = parse(r#"{"kind":"captcha","challenge":"recaptcha-v2","page":"${url"}"#)
            .expect("parse");
        assert!(matches!(step.validate(), Err(RuleError::Template(_))));
    }

    #[test]
    fn a_pattern_that_does_not_compile_is_refused() {
        let step = parse(r#"{"kind":"regex","pattern":"(","into":"links"}"#).expect("parse");
        assert!(matches!(step.validate(), Err(RuleError::Pattern { .. })));
    }

    #[test]
    fn a_bad_variable_name_is_refused_wherever_it_appears() {
        let step =
            parse(r#"{"kind":"decode","encoding":"hex","from":"Raw","into":"x"}"#).expect("parse");
        assert!(matches!(step.validate(), Err(RuleError::Variable(name)) if name == "Raw"));
        let step = parse(r#"{"kind":"fetch","into":"2page"}"#).expect("parse");
        assert!(matches!(step.validate(), Err(RuleError::Variable(_))));
    }

    #[test]
    fn an_unterminated_placeholder_is_refused() {
        let step = parse(r#"{"kind":"form","url":"https://x.example/${id"}"#).expect("parse");
        assert!(matches!(step.validate(), Err(RuleError::Template(_))));
    }

    #[test]
    fn a_json_pointer_starts_with_a_slash() {
        let step =
            parse(r#"{"kind":"fetch-json","url":"u","path":"items","into":"i"}"#).expect("parse");
        assert!(matches!(step.validate(), Err(RuleError::JsonPointer(_))));
    }

    #[test]
    fn an_unknown_encoding_is_refused() {
        assert!(parse(r#"{"kind":"decode","encoding":"zip","from":"a","into":"b"}"#).is_err());
    }
}
