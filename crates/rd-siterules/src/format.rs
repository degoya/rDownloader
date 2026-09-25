//! One rule: what it claims, what it does, where the package name comes from, and what has
//! to be true about it before it is allowed in.
//!
//! The shape follows what JDownloader's page decrypters carry once the Java is stripped
//! away — an address pattern, a container, two regular expressions and a list of former
//! domains — plus what this project needs to keep a rule honest: a real address to probe
//! (RD-110-09) and the date the service was last measured alive.

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::{
    step::{Step, check_pattern, check_variable},
    text::{MAX_GROUP_LENGTH, MAX_ID_LENGTH, host_matches, is_host, is_host_pattern, is_slug},
};

/// Longest display name a rule may carry.
pub const MAX_NAME_LENGTH: usize = 120;

/// One rule for one service.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    /// Stable identifier, lowercase kebab-case. A user rule may not reuse a shipped one's.
    pub id: String,
    /// What the interface shows.
    pub name: String,
    /// `board`, `paste`, `adult`, ...: what the interface groups and switches by.
    pub group: String,
    /// The rule's own revision, from 1. Bumped when the rule changes, so a user rule that
    /// was copied from a shipped one can say which revision it started from.
    pub version: u32,
    /// The addresses this rule claims.
    #[serde(rename = "match")]
    pub matches: Match,
    /// Hosts the service once had and that no longer answer. An address on one of them is
    /// rewritten to the first host in `match.hosts` instead of being refused.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dead: Vec<String>,
    /// What to do, in order.
    pub steps: Vec<Step>,
    /// Where the package name comes from.
    pub package: PackageSource,
    /// Whether the links behind one address of this rule are copies of the same file
    /// (RD-110-18).
    ///
    /// True for the shape a release page has: one file, posted to five hosters, listed once
    /// each. It is a statement about the *page*, not about a link, which is why it sits on
    /// the rule and not on a step -- the rule format carries no per-link metadata, so a page
    /// that lists several different files each with its own mirrors cannot be described this
    /// way and leaves this false.
    ///
    /// Absent in every rule written before this existed, and absent in the serialized form
    /// when false, so a pack signed before it stays byte-identical and keeps its signature.
    #[serde(default, skip_serializing_if = "is_false")]
    pub mirrors: bool,
    /// A real address the self-test (RD-110-09) fetches. Must be one this rule claims.
    pub probe: String,
    /// The date the service was last measured alive.
    pub checked: NaiveDate,
}

/// Whether a flag is off, for `skip_serializing_if`.
fn is_false(value: &bool) -> bool {
    !*value
}

/// The addresses a rule claims: a host list and, optionally, path patterns.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Match {
    /// Concrete hosts, or `*.example.org` for a host and everything below it. The first
    /// entry is the canonical host and must be concrete: dead hosts are rewritten to it.
    pub hosts: Vec<String>,
    /// Regular expressions over the path (with query, if any). Empty claims every path.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
}

/// Where a package name comes from.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "from", rename_all = "kebab-case", deny_unknown_fields)]
pub enum PackageSource {
    /// The page's `<title>`.
    Title,
    /// The first capture of `pattern` applied to a variable, `page` unless `source` says.
    Regex {
        pattern: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source: Option<String>,
    },
    /// A variable a step wrote.
    Variable { name: String },
}

/// Why a rule was refused.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum RuleError {
    #[error("rule id {0:?} is not lowercase kebab-case of at most 64 characters")]
    Id(String),
    #[error("rule name is empty or longer than 120 characters")]
    Name,
    #[error("group {0:?} is not lowercase kebab-case of at most 32 characters")]
    Group(String),
    #[error("rule version must be at least 1")]
    Version,
    #[error("match.hosts is empty")]
    NoHosts,
    #[error("{0:?} is not a host name")]
    Host(String),
    #[error("the first host in match.hosts must be concrete, not a wildcard")]
    WildcardCanonical,
    #[error("{0:?} is listed as dead and as live")]
    DeadIsLive(String),
    #[error("pattern {pattern:?} does not compile: {reason}")]
    Pattern { pattern: String, reason: String },
    #[error("steps is empty")]
    NoSteps,
    #[error("{0:?} is not a variable name")]
    Variable(String),
    #[error("template {0:?} has an unterminated or malformed ${{...}} placeholder")]
    Template(String),
    #[error("JSON pointer {0:?} does not start with a slash")]
    JsonPointer(String),
    #[error("captcha challenge {0:?} is not lowercase kebab-case")]
    CaptchaKind(String),
    #[error("probe {0:?} is not an absolute http(s) address")]
    Probe(String),
    #[error("probe {0:?} is not claimed by the rule's own match")]
    ProbeUnclaimed(String),
}

impl Rule {
    /// Refuses a rule that cannot be right, before anything is fetched.
    pub fn validate(&self) -> Result<(), RuleError> {
        if !is_slug(&self.id, MAX_ID_LENGTH) {
            return Err(RuleError::Id(self.id.clone()));
        }
        if self.name.trim().is_empty() || self.name.chars().count() > MAX_NAME_LENGTH {
            return Err(RuleError::Name);
        }
        if !is_slug(&self.group, MAX_GROUP_LENGTH) {
            return Err(RuleError::Group(self.group.clone()));
        }
        if self.version == 0 {
            return Err(RuleError::Version);
        }
        self.matches.validate()?;
        for host in &self.dead {
            if !is_host(host) {
                return Err(RuleError::Host(host.clone()));
            }
            if self
                .matches
                .hosts
                .iter()
                .any(|live| host_matches(live, host))
            {
                return Err(RuleError::DeadIsLive(host.clone()));
            }
        }
        if self.steps.is_empty() {
            return Err(RuleError::NoSteps);
        }
        for step in &self.steps {
            step.validate()?;
        }
        self.package.validate()?;
        let probe =
            parse_http_url(&self.probe).ok_or_else(|| RuleError::Probe(self.probe.clone()))?;
        if !self.matches.claims(&probe) {
            return Err(RuleError::ProbeUnclaimed(self.probe.clone()));
        }
        Ok(())
    }

    /// Whether this rule claims `url`.
    #[must_use]
    pub fn claims(&self, url: &Url) -> bool {
        self.matches.claims(url)
    }

    /// The same address on the canonical host when `url` sits on a dead one; `None` when it
    /// does not, so a caller can tell a rewrite from a pass-through.
    #[must_use]
    pub fn revive(&self, url: &Url) -> Option<Url> {
        let host = url.host_str()?;
        if !self.dead.iter().any(|dead| dead == host) {
            return None;
        }
        let canonical = self.matches.hosts.first()?;
        let mut revived = url.clone();
        revived.set_host(Some(canonical)).ok()?;
        Some(revived)
    }
}

impl Match {
    fn validate(&self) -> Result<(), RuleError> {
        let Some(first) = self.hosts.first() else {
            return Err(RuleError::NoHosts);
        };
        if first.starts_with("*.") {
            return Err(RuleError::WildcardCanonical);
        }
        for host in &self.hosts {
            if !is_host_pattern(host) {
                return Err(RuleError::Host(host.clone()));
            }
        }
        for pattern in &self.paths {
            check_pattern(pattern)?;
        }
        Ok(())
    }

    /// Whether `url` is on one of the hosts and, if paths are given, on one of the paths.
    ///
    /// A path pattern that does not compile matches nothing; validation refuses it before a
    /// rule is used, so this is the safe reading rather than an expected case.
    #[must_use]
    pub fn claims(&self, url: &Url) -> bool {
        let Some(host) = url.host_str() else {
            return false;
        };
        if !self.hosts.iter().any(|pattern| host_matches(pattern, host)) {
            return false;
        }
        if self.paths.is_empty() {
            return true;
        }
        let path = match url.query() {
            Some(query) => format!("{}?{query}", url.path()),
            None => url.path().to_owned(),
        };
        self.paths
            .iter()
            .any(|pattern| regex::Regex::new(pattern).is_ok_and(|regex| regex.is_match(&path)))
    }
}

impl PackageSource {
    fn validate(&self) -> Result<(), RuleError> {
        match self {
            Self::Title => Ok(()),
            Self::Regex { pattern, source } => {
                check_pattern(pattern)?;
                source.as_deref().map_or(Ok(()), check_variable)
            }
            Self::Variable { name } => check_variable(name),
        }
    }
}

/// `text` as an absolute `http` or `https` address with a host, or `None`.
fn parse_http_url(text: &str) -> Option<Url> {
    let url = Url::parse(text).ok()?;
    (matches!(url.scheme(), "http" | "https") && url.host_str().is_some()).then_some(url)
}

#[cfg(test)]
#[path = "format_tests.rs"]
pub(crate) mod tests;
