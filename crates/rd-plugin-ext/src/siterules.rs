//! Site rules as a source in the crawler selection (RD-110-06).
//!
//! A rule answers the same question a crawler plugin answers — "what lies behind this
//! address?" — from data rather than from code, so it belongs in the same selection rather
//! than beside it. Where it sits there is decided in [`crate::FolderCrawlers`]; this module is
//! the source itself: which rules are asked, in which order, and what their answers mean.
//!
//! **The order inside the rules: the person's own first.** A rule someone wrote for a service
//! the shipped pack also covers must win, or a broken shipped rule could not be bridged
//! without waiting for a release. The two never share an id — `rd_siterules::Catalogue`
//! refuses that — so this is a precedence between two different rules, not an override.
//!
//! **A rule the self-test found dead is not asked.** RD-110-09 probes every rule against the
//! address it names itself and sorts the answer into four states; a rule whose service no
//! longer answers at all is skipped here rather than deleted, so it costs no request and the
//! person keeps the rule, its history and the chance that the service comes back. The ids
//! arrive through [`SiteRules::set_dead`] because this crate has no database and is not
//! getting one.
//!
//! **Only an unclaimed address keeps the search going.** `RunError::not_mine` is true for
//! exactly one refusal: the rule's own `match` does not claim the address, which says nothing
//! about the page. Every other code — dead, guarded, changed, over a limit — is a statement
//! about *this* page, and a statement is reported rather than swallowed by the next source in
//! line.

use std::{
    collections::BTreeSet,
    sync::{Arc, RwLock},
};

use async_trait::async_trait;
use rd_siterules::{Catalogue, Crawl, Executor, Rule, RunError, SystemClock};
use url::Url;

/// Runs one rule against one address.
///
/// A trait rather than the executor itself so the order and the fallback above can be read —
/// and tested — without a network, exactly as [`crate::FolderCrawlers`] keeps its plugins
/// behind one.
#[async_trait]
pub trait RuleRunner: Send + Sync {
    async fn run(&self, rule: &Rule, address: &Url) -> Result<Crawl, RunError>;
}

/// The runner a running installation uses: `rd_siterules::Executor` over the adapters in
/// `rd-plugin-host`, where the proxy profiles, the TLS roots and the captcha broker are.
///
/// A fresh fetcher and a fresh clock per run: the fetcher's cookies and pinned addresses
/// belong to one run, and the clock measures that run's budget from its own start.
pub struct HostRuleRunner {
    network: rd_plugin_host::RuleNetwork,
    captcha: Option<Arc<dyn rd_plugin_api::CaptchaSolver>>,
}

impl HostRuleRunner {
    #[must_use]
    pub fn new(network: rd_plugin_host::RuleNetwork) -> Self {
        Self {
            network,
            captcha: None,
        }
    }

    /// Gives rule runs the captcha broker. Without it a `captcha` step refuses with
    /// `site_rules.captcha_failed` rather than being skipped.
    #[must_use]
    pub fn with_captcha(mut self, captcha: Arc<dyn rd_plugin_api::CaptchaSolver>) -> Self {
        self.captcha = Some(captcha);
        self
    }
}

#[async_trait]
impl RuleRunner for HostRuleRunner {
    async fn run(&self, rule: &Rule, address: &Url) -> Result<Crawl, RunError> {
        let fetcher = self.network.fetcher();
        let resolver = rd_plugin_host::RuleResolver;
        let clock = SystemClock::new();
        let captcha = self
            .captcha
            .as_ref()
            .map(|solver| rd_plugin_host::RuleCaptcha::new(Arc::clone(solver)));
        let executor = Executor::new(&fetcher, &resolver, &clock);
        let executor = match &captcha {
            Some(captcha) => executor.with_captcha(captcha),
            None => executor,
        };
        executor.run(rule, address).await
    }
}

/// What the rules had to say about one address.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RuleOutcome {
    /// The links behind the address, and the package name the rule read.
    Crawled { rule: String, crawl: Crawl },
    /// A rule claimed the address and then said what is wrong with the page.
    Refused { rule: String, error: RunError },
}

/// The rules this installation consults, and the order it consults them in.
pub struct SiteRules {
    /// Replaceable so the interface (RD-110-08) can take effect without a restart; read under
    /// the lock and cloned out of it, because a run may not hold a lock across an await.
    catalogue: RwLock<Arc<Catalogue>>,
    /// The ids the last self-test found dead (RD-110-09). Replaceable for the same reason
    /// the catalogue is: a run finishes and the next paste already profits.
    dead: RwLock<Arc<BTreeSet<String>>>,
    runner: Arc<dyn RuleRunner>,
}

impl SiteRules {
    #[must_use]
    pub fn new(catalogue: Catalogue, runner: Arc<dyn RuleRunner>) -> Self {
        Self {
            catalogue: RwLock::new(Arc::new(catalogue)),
            dead: RwLock::new(Arc::new(BTreeSet::new())),
            runner,
        }
    }

    /// The rules in force. A poisoned lock yields the catalogue anyway: a panic somewhere
    /// else is no reason to stop recognising pages.
    #[must_use]
    pub fn catalogue(&self) -> Arc<Catalogue> {
        Arc::clone(
            &self
                .catalogue
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        )
    }

    /// Replaces the rules in force, for the interface that edits them (RD-110-08).
    pub fn replace(&self, catalogue: Catalogue) {
        let mut held = self
            .catalogue
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *held = Arc::new(catalogue);
    }

    /// The rules the self-test found dead, which are skipped until a later run says
    /// otherwise.
    #[must_use]
    pub fn dead(&self) -> Arc<BTreeSet<String>> {
        Arc::clone(
            &self
                .dead
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        )
    }

    /// Names the rules the self-test found dead. `serve` fills this at start from what the
    /// last run stored; the self-test writes it forward when it has run again.
    pub fn set_dead(&self, ids: BTreeSet<String>) {
        let mut held = self
            .dead
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *held = Arc::new(ids);
    }

    /// Whether there is any rule at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.catalogue().rules().next().is_none()
    }

    /// Whether some rule that is switched on claims this address, without fetching anything.
    ///
    /// The cheap half of [`Self::consult`], and the only thing a watched release page needs
    /// of the rules (RD-110-21): a listing links to its navigation, to the group's own site
    /// and to a dozen unrelated pages, and this is what separates the release pages among
    /// them from the rest. The same order and the same dead-rule skip as a real run, so the
    /// answer never promises a rule the run would then refuse to use.
    #[must_use]
    pub fn claims(&self, address: &Url) -> bool {
        let catalogue = self.catalogue();
        let dead = self.dead();
        catalogue
            .user()
            .iter()
            .chain(catalogue.shipped().iter())
            .any(|rule| !dead.contains(&rule.id) && rule.claims(address))
    }

    /// Asks the rules about one address and answers for the first one that says anything
    /// about it. `None` when no rule claims it, which leaves the search to the next source.
    pub async fn consult(&self, address: &Url) -> Option<RuleOutcome> {
        let catalogue = self.catalogue();
        let dead = self.dead();
        // The person's own rules first; see the module documentation.
        for rule in catalogue.user().iter().chain(catalogue.shipped().iter()) {
            // A service the self-test found gone is not asked again. The rule stays, and a
            // later run can revive it; what it does not do is cost a request per paste.
            if dead.contains(&rule.id) {
                continue;
            }
            match self.runner.run(rule, address).await {
                Ok(crawl) => {
                    return Some(RuleOutcome::Crawled {
                        rule: rule.name.clone(),
                        crawl,
                    });
                }
                Err(error) if error.not_mine() => {
                    continue;
                }
                Err(error) => {
                    return Some(RuleOutcome::Refused {
                        rule: rule.name.clone(),
                        error,
                    });
                }
            }
        }
        None
    }
}

#[cfg(test)]
#[path = "siterules_tests.rs"]
pub(crate) mod tests;
