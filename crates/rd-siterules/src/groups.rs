//! One package per entry of a list (RD-1170-02): the shape of a page that lists several
//! releases -- two seasons in two qualities, each posted to two hosters -- rather than one.
//!
//! A rule without `groups` yields one link list and one package name, which is all a release
//! page needed until such pages arrived. `groups` keeps that whole shape and runs it once more
//! per entry: the rule's own steps leave a list in a variable, one entry per package, and the
//! group's steps turn one entry into that package's links, with a name of its own. The same
//! seven step kinds, the same variables and the same budget -- a group is not a second
//! language, it is the rule's tail repeated.
//!
//! **Mirrors are stated per group**, because the page states them per release: the n-th link
//! at one hoster is the same file as the n-th link at every other hoster of that release
//! (`by-host`), or every link of the release is the same file (`all`). Matching by position is
//! what the measured pages allow -- warez.cx names no file per link, only the hoster lists in
//! the same episode order -- and it is what the LinkGrabber's declared mirror groups
//! (RD-110-18) take as they are.
//!
//! Absent in every rule written before this existed, and absent in the serialized form when
//! absent, so a signed pack and every exported rule stay byte-identical.
//!
//! **Pick before resolving** (RD-1170-03). A series page lists thirty releases, and fetching the
//! links of one costs a captcha. With `pick` the run stops after the rule's own steps: each
//! entry is listed with its name and the attributes `pick` reads from it -- season, episode,
//! resolution, language, hoster -- and nothing else is fetched. The group's steps run later,
//! once per entry somebody chose ([`crate::Executor::resolve`]), from the variables the first
//! stage left.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{
    format::{PackageSource, RuleError},
    step::{Step, check_pattern, check_variable},
};

/// Most attributes one `pick` may read per entry.
pub const MAX_PICK_ATTRIBUTES: usize = 16;

/// The variable a group's steps find their entry in when `into` names no other.
pub const ENTRY_VARIABLE: &str = "entry";

/// How a rule turns one page into several packages.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Groups {
    /// The variable the rule's own steps left with one entry per package.
    pub from: String,
    /// The variable each entry is handed to the group's steps in; [`ENTRY_VARIABLE`] unless
    /// named.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub into: Option<String>,
    /// List the entries and let somebody choose before any of them is resolved (RD-1170-03).
    /// Absent: every entry is resolved in the same run, as RD-1170-02 does it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pick: Option<Pick>,
    /// What to do with one entry, in order. They see every variable the rule's steps wrote
    /// and end with the entry's links in `links`, as a rule without groups does.
    pub steps: Vec<Step>,
    /// Where one entry's package name comes from, read after its steps. The rule's own
    /// `package` stands in when this finds nothing.
    pub package: PackageSource,
    /// Which links of one entry are copies of the same file. Absent: none are.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mirrors: Option<GroupMirrors>,
}

/// What the first stage of a two-stage rule reads from each entry (RD-1170-03).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Pick {
    /// Name to pattern: the first capture of the pattern, applied to one entry, is that entry's
    /// value. An entry the pattern does not match has no value for it -- a season pack has no
    /// episode. The interface groups by `season` and filters by `season`, `episode`,
    /// `resolution` and `language` and shows `hoster`; any other name is shown as it is.
    pub attributes: BTreeMap<String, String>,
}

impl Pick {
    fn validate(&self) -> Result<(), RuleError> {
        if self.attributes.len() > MAX_PICK_ATTRIBUTES {
            return Err(RuleError::PickAttributes(MAX_PICK_ATTRIBUTES));
        }
        for (name, pattern) in &self.attributes {
            check_variable(name)?;
            check_pattern(pattern)?;
        }
        Ok(())
    }
}

/// Which links of one group are copies of the same file.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum GroupMirrors {
    /// Every link of the group is a copy of one file: one release, one file, many hosters.
    All,
    /// The n-th link at one host is a copy of the n-th link at every other host of the group:
    /// one release of many parts, the same part list posted to each hoster in the same order.
    ByHost,
}

impl Groups {
    /// The variable one entry is handed over in.
    #[must_use]
    pub fn entry_variable(&self) -> &str {
        self.into.as_deref().unwrap_or(ENTRY_VARIABLE)
    }

    /// Refuses a group description whose static parts cannot work.
    pub(crate) fn validate(&self) -> Result<(), RuleError> {
        check_variable(&self.from)?;
        check_variable(self.entry_variable())?;
        if self.steps.is_empty() {
            return Err(RuleError::NoGroupSteps);
        }
        for step in &self.steps {
            step.validate()?;
        }
        if let Some(pick) = &self.pick {
            pick.validate()?;
        }
        self.package.validate()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: serde_json::Value) -> Result<Groups, serde_json::Error> {
        serde_json::from_value(json)
    }

    fn groups() -> serde_json::Value {
        serde_json::json!({
            "from": "releases",
            "steps": [{ "kind": "regex", "from": "entry", "pattern": "(https?://\\S+)",
                        "into": "links", "all": true }],
            "package": { "from": "regex", "pattern": "\"name\":\"([^\"]+)\"", "source": "entry" },
            "mirrors": "by-host"
        })
    }

    #[test]
    fn a_group_description_reads_validates_and_round_trips() {
        let parsed = parse(groups()).expect("parse");
        parsed.validate().expect("valid");
        assert_eq!(parsed.entry_variable(), ENTRY_VARIABLE);
        assert_eq!(parsed.mirrors, Some(GroupMirrors::ByHost));
        assert_eq!(serde_json::to_value(&parsed).expect("encode"), groups());
    }

    #[test]
    fn the_mirror_modes_are_spelled_as_documented() {
        let mut json = groups();
        json["mirrors"] = "all".into();
        assert_eq!(parse(json).expect("parse").mirrors, Some(GroupMirrors::All));
        let mut json = groups();
        json["mirrors"] = "by-name".into();
        assert!(parse(json).is_err(), "an unknown mode is refused");
        let mut json = groups();
        json["extra"] = 1.into();
        assert!(parse(json).is_err(), "an unknown field is refused");
    }

    #[test]
    fn a_pick_reads_validates_and_is_left_out_when_absent() {
        assert!(parse(groups()).expect("parse").pick.is_none());
        let mut json = groups();
        json["pick"] = serde_json::json!({ "attributes": {
            "season": "\"season\":(\\d+)", "resolution": "\"resolution\":\"([^\"]+)\"" } });
        let parsed = parse(json.clone()).expect("parse");
        parsed.validate().expect("valid");
        assert_eq!(
            parsed.pick.as_ref().map(|pick| pick.attributes.len()),
            Some(2)
        );
        assert_eq!(serde_json::to_value(&parsed).expect("encode"), json);
        let mut bad = json.clone();
        bad["pick"]["attributes"]["Season"] = "(x)".into();
        assert!(matches!(
            parse(bad).expect("parse").validate(),
            Err(RuleError::Variable(_))
        ));
        let mut bad = json.clone();
        bad["pick"]["attributes"]["episode"] = "(".into();
        assert!(matches!(
            parse(bad).expect("parse").validate(),
            Err(RuleError::Pattern { .. })
        ));
        let mut bad = json.clone();
        bad["pick"]["extra"] = 1.into();
        assert!(parse(bad).is_err(), "an unknown field is refused");
        let mut bad = json;
        bad["pick"]["attributes"] = (0..=MAX_PICK_ATTRIBUTES)
            .map(|index| (format!("a{index}"), serde_json::Value::from("(x)")))
            .collect::<serde_json::Map<_, _>>()
            .into();
        assert_eq!(
            parse(bad).expect("parse").validate(),
            Err(RuleError::PickAttributes(MAX_PICK_ATTRIBUTES))
        );
    }

    #[test]
    fn what_cannot_work_is_refused_before_anything_is_fetched() {
        let mut json = groups();
        json["steps"] = serde_json::json!([]);
        assert_eq!(
            parse(json).expect("parse").validate(),
            Err(RuleError::NoGroupSteps)
        );
        let mut json = groups();
        json["from"] = "Releases".into();
        assert!(matches!(
            parse(json).expect("parse").validate(),
            Err(RuleError::Variable(_))
        ));
        let mut json = groups();
        json["into"] = "2nd".into();
        assert!(matches!(
            parse(json).expect("parse").validate(),
            Err(RuleError::Variable(_))
        ));
        let mut json = groups();
        json["package"] = serde_json::json!({ "from": "regex", "pattern": "(" });
        assert!(matches!(
            parse(json).expect("parse").validate(),
            Err(RuleError::Pattern { .. })
        ));
    }
}
