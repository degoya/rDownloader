//! The self-test (RD-110-09): a rule is asked about its own `probe` and says how it fared.
//!
//! Rules age. The measurement of 20 September 2026 that led to this module is the whole
//! argument: of 124 domains JDownloader carries for this class of page, 47 no longer
//! answered at all. Without a mechanism the same state walks into rDownloader, and the
//! person notices it as an empty package rather than as a finding.
//!
//! Four verdicts, and no fifth. Every refusal the executor (RD-110-05) can produce sorts
//! into one of them; [`Verdict::of`] is that sorting and nothing else. A limit is counted as
//! [`Verdict::Structural`] — `RunError::is_limit` says a limit tells nothing about the
//! service, but a run that hit one still produced no links, and the exact code travels in the
//! same report, so the reason is not lost.
//!
//! **No network of its own.** This module runs on the same four ports the executor takes, so
//! its tests drive it through recorded answers rather than a live site, and the crate stays
//! the leaf `AGENTS.md` describes.

use url::Url;

use crate::{
    exec::{Executor, RunError},
    format::Rule,
};

/// What a rule with a `probe` that is not an address at all reports. Every other code in a
/// report is one of `RunError`'s.
pub const PROBE_INVALID: &str = "site_rules.probe_invalid";

/// How a rule fared against its own probe.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Verdict {
    /// Reachable, the structure fits, at least one link. The executor never returns a run
    /// with no links, so a success is always this.
    Ok,
    /// Reachable, but nothing came back: the theme or the page layout changed.
    Structural,
    /// Something answered and refused: 401, 403, 429, or a captcha nobody could answer.
    Blocked,
    /// Nothing answered, or the page is gone for good.
    Dead,
}

impl Verdict {
    /// How the verdict is stored and printed: a stable word, not a translated one.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Structural => "structural",
            Self::Blocked => "blocked",
            Self::Dead => "dead",
        }
    }

    /// The stable code the interface translates.
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::Ok => "site_rules.state.ok",
            Self::Structural => "site_rules.state.structural",
            Self::Blocked => "site_rules.state.blocked",
            Self::Dead => "site_rules.state.dead",
        }
    }

    /// The verdict a stored word names; `None` for anything else, so a row written by a
    /// later build is ignored rather than guessed at.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        [Self::Ok, Self::Structural, Self::Blocked, Self::Dead]
            .into_iter()
            .find(|verdict| verdict.as_str() == text)
    }

    /// Whether the rule still does what it claims.
    #[must_use]
    pub fn is_ok(self) -> bool {
        self == Self::Ok
    }

    /// Which verdict a refusal amounts to. The table is the one in the job file; see the
    /// module documentation for why a limit lands on [`Self::Structural`].
    #[must_use]
    pub fn of(error: &RunError) -> Self {
        match error {
            RunError::Blocked { .. } | RunError::CaptchaFailed { .. } => Self::Blocked,
            RunError::PageDead { .. }
            | RunError::FetchFailed { .. }
            | RunError::AddressNotPublic { .. } => Self::Dead,
            RunError::NoLinks
            | RunError::Structure { .. }
            | RunError::DecodeFailed { .. }
            | RunError::NotClaimed(_)
            | RunError::TargetNotAllowed { .. }
            | RunError::Cycle { .. }
            | RunError::ResponseTooLarge { .. }
            | RunError::LimitDepth(_)
            | RunError::LimitPages(_)
            | RunError::LimitLinks(_)
            | RunError::LimitTime(_) => Self::Structural,
        }
    }
}

/// What one probe produced: enough to print a line, to store a row and to decide whether the
/// rule is asked again.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuleReport {
    pub rule_id: String,
    pub rule_name: String,
    /// The address that was fetched, as the rule carries it.
    pub probe: String,
    pub verdict: Verdict,
    /// The refusal's stable code, or [`None`] when the rule answered.
    pub reason: Option<&'static str>,
    pub links: usize,
    pub pages: u32,
}

/// Asks one rule about its own probe.
///
/// The rule is run exactly as the crawler selection runs it, against the address it names
/// itself. A `probe` that is not an address is a finding about the rule rather than about the
/// service, so it is reported as [`Verdict::Structural`] with [`PROBE_INVALID`] instead of
/// being skipped: a rule that cannot be checked is a rule nobody is watching.
pub async fn check(executor: &Executor<'_>, rule: &Rule) -> RuleReport {
    let report = |verdict, reason, links, pages| RuleReport {
        rule_id: rule.id.clone(),
        rule_name: rule.name.clone(),
        probe: rule.probe.clone(),
        verdict,
        reason,
        links,
        pages,
    };
    let Ok(probe) = Url::parse(&rule.probe) else {
        return report(Verdict::Structural, Some(PROBE_INVALID), 0, 0);
    };
    match executor.run(rule, &probe).await {
        Ok(crawl) => report(Verdict::Ok, None, crawl.links.len(), crawl.pages_fetched),
        Err(error) => report(Verdict::of(&error), Some(error.code()), 0, 0),
    }
}

#[cfg(test)]
#[path = "selftest_tests.rs"]
mod tests;
