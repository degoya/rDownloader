//! Why a run produced no links, as a stable code rather than a sentence.
//!
//! Every refusal names one reason and one reason only, because two consumers read these:
//! RD-110-06 has to tell "this address was never mine" from "my page changed" — the first
//! lets the selection keep looking, the second is a finding — and RD-110-09 sorts a rule into
//! `ok`, `strukturell`, `blockiert` or `tot` from the same codes. A refusal that merged the
//! cases would make both of them guess.

use std::net::IpAddr;

/// Why a rule produced nothing.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum RunError {
    #[error("the rule does not claim {0}")]
    NotClaimed(String),
    #[error("{url} is gone ({reason})")]
    PageDead { url: String, reason: String },
    #[error("{url} refused the request with status {status}")]
    Blocked { url: String, status: u16 },
    #[error("{url} could not be fetched: {reason}")]
    FetchFailed { url: String, reason: String },
    #[error("{url} is not a host this rule may reach")]
    TargetNotAllowed { url: String },
    #[error("{host} resolves to {address}, which is not a public address")]
    AddressNotPublic { host: String, address: IpAddr },
    #[error("{url} answered with more than {limit} bytes")]
    ResponseTooLarge { url: String, limit: usize },
    #[error("step {step} ({kind}) found nothing: {detail}")]
    Structure {
        step: usize,
        kind: &'static str,
        detail: String,
    },
    #[error("step {step} could not decode the value as {encoding}")]
    DecodeFailed { step: usize, encoding: String },
    #[error("step {step} got no captcha answer: {reason}")]
    CaptchaFailed { step: usize, reason: String },
    #[error("{url} had already been fetched in this run")]
    Cycle { url: String },
    #[error("the run went deeper than {0} requests from the address it was given")]
    LimitDepth(u32),
    #[error("the run wanted more than {0} pages")]
    LimitPages(u32),
    #[error("the rule produced more than {0} links")]
    LimitLinks(usize),
    #[error("the run passed its budget of {0} seconds")]
    LimitTime(u64),
    #[error("the rule matched but produced no link")]
    NoLinks,
}

impl RunError {
    /// The stable code, translated by the interface.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotClaimed(_) => "site_rules.not_claimed",
            Self::PageDead { .. } => "site_rules.page_dead",
            Self::Blocked { .. } => "site_rules.blocked",
            Self::FetchFailed { .. } => "site_rules.fetch_failed",
            Self::TargetNotAllowed { .. } => "site_rules.target_not_allowed",
            Self::AddressNotPublic { .. } => "site_rules.address_not_public",
            Self::ResponseTooLarge { .. } => "site_rules.response_too_large",
            Self::Structure { .. } => "site_rules.structure",
            Self::DecodeFailed { .. } => "site_rules.decode_failed",
            Self::CaptchaFailed { .. } => "site_rules.captcha_failed",
            Self::Cycle { .. } => "site_rules.cycle",
            Self::LimitDepth(_) => "site_rules.limit_depth",
            Self::LimitPages(_) => "site_rules.limit_pages",
            Self::LimitLinks(_) => "site_rules.limit_links",
            Self::LimitTime(_) => "site_rules.limit_time",
            Self::NoLinks => "site_rules.no_links",
        }
    }

    /// Whether the selection should keep looking for another source (RD-110-06).
    ///
    /// Exactly one refusal says nothing about the page: the rule was asked about an address
    /// its own `match` does not claim. Everything else is a statement about *this* page —
    /// dead, changed, guarded, over a limit — and a statement is reported rather than
    /// swallowed by the next crawler in line.
    #[must_use]
    pub fn not_mine(&self) -> bool {
        matches!(self, Self::NotClaimed(_))
    }

    /// Whether this refusal is a limit rather than a finding about the service. A run that
    /// hit a limit says nothing about whether the rule still fits its site.
    #[must_use]
    pub fn is_limit(&self) -> bool {
        matches!(
            self,
            Self::LimitDepth(_)
                | Self::LimitPages(_)
                | Self::LimitLinks(_)
                | Self::LimitTime(_)
                | Self::ResponseTooLarge { .. }
                | Self::Cycle { .. }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_refusal_has_its_own_code() {
        let errors = [
            RunError::NotClaimed("u".into()),
            RunError::PageDead {
                url: "u".into(),
                reason: "404".into(),
            },
            RunError::Blocked {
                url: "u".into(),
                status: 403,
            },
            RunError::FetchFailed {
                url: "u".into(),
                reason: "x".into(),
            },
            RunError::TargetNotAllowed { url: "u".into() },
            RunError::AddressNotPublic {
                host: "h".into(),
                address: IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
            },
            RunError::ResponseTooLarge {
                url: "u".into(),
                limit: 1,
            },
            RunError::Structure {
                step: 0,
                kind: "regex",
                detail: "x".into(),
            },
            RunError::DecodeFailed {
                step: 0,
                encoding: "hex".into(),
            },
            RunError::CaptchaFailed {
                step: 0,
                reason: "x".into(),
            },
            RunError::Cycle { url: "u".into() },
            RunError::LimitDepth(1),
            RunError::LimitPages(1),
            RunError::LimitLinks(1),
            RunError::LimitTime(1),
            RunError::NoLinks,
        ];
        let mut codes: Vec<_> = errors.iter().map(RunError::code).collect();
        let count = codes.len();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), count, "two refusals share a code");
        assert!(codes.iter().all(|code| code.starts_with("site_rules.")));
    }

    #[test]
    fn only_an_unclaimed_address_lets_the_selection_keep_looking() {
        assert!(RunError::NotClaimed("u".into()).not_mine());
        assert!(!RunError::NoLinks.not_mine());
        assert!(
            !RunError::PageDead {
                url: "u".into(),
                reason: "404".into(),
            }
            .not_mine()
        );
    }
}
