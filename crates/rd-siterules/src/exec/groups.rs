//! A rule's `groups` at run time (RD-1170-02): the group's steps once per entry, each entry's
//! links and name read the way a rule without groups reads its own, and the mirror sets the
//! group states numbered.
//!
//! Every entry starts from the same variables -- what the rule's own steps left -- so the
//! second release of a page never reads what the first one's steps wrote, and a group whose
//! steps write no `links` yields nothing rather than the previous entry's. The entries are
//! siblings, as the requests of a `fetch` over a list are: each sits as deep as the rule's
//! steps left the run, every request still passes the one door in `run.rs`, and the run's
//! budgets are shared by all of them.

use std::collections::{BTreeMap, BTreeSet};

use url::Url;

use super::{
    error::RunError,
    run::Run,
    value::{CAPTCHA_VARIABLE, PAGE_URL_VARIABLE, Value},
};
use crate::{
    groups::{GroupMirrors, Groups},
    step::{LINKS_VARIABLE, PAGE_VARIABLE, Step},
};

/// One package a rule with `groups` produced.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CrawlGroup {
    /// The package name the group's source read, or the rule's own when it read nothing. A
    /// hint, as [`super::Crawl::package_name`] is.
    pub name: Option<String>,
    /// The links, absolute and deduplicated across the whole run, in the order found.
    pub links: Vec<GroupLink>,
}

/// One link of a group, with its place among the group's mirrors.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GroupLink {
    pub url: String,
    /// The mirror set, counted from 1 within the group: links of one group carrying the same
    /// number are copies of one file. `None` when the group states no mirrors, or when no
    /// other link of the group carries the number -- a set of one is no mirror.
    pub mirror: Option<u32>,
}

impl Run<'_> {
    /// Runs the group's steps once per entry of `groups.from` and collects one package per
    /// entry that produced a link. `fallback` is the rule's own package name.
    pub(crate) async fn groups(
        &mut self,
        groups: &Groups,
        fallback: Option<&str>,
    ) -> Result<Vec<CrawlGroup>, RunError> {
        // Numbered after the rule's own steps, so a refusal names exactly one step.
        let first = self.rule.steps.len();
        let entries: Vec<String> = self
            .read(first, "groups", &groups.from)?
            .iter()
            .map(str::to_owned)
            .collect();
        // Every group has to yield a link to count, so more entries than links is over the
        // same limit, and refused before a single entry runs.
        if entries.len() > self.limits.max_links {
            return Err(RunError::LimitLinks(self.limits.max_links));
        }
        let saved = self.saved(groups);
        let start = self.depth;
        let mut deepest = start;
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let mut proposed = 0_usize;
        let mut found = Vec::new();
        for entry in entries {
            self.restore(&saved);
            // Never the rule's own `links`, nor anything else a group could mistake for its own.
            self.variables.unset(LINKS_VARIABLE);
            self.variables.set(groups.entry_variable(), entry);
            self.depth = start;
            for (offset, step) in groups.steps.iter().enumerate() {
                self.step(first + offset, step).await?;
            }
            deepest = deepest.max(self.depth);
            let Some(value) = self.variables.get(LINKS_VARIABLE).cloned() else {
                continue;
            };
            // The limit counts what the rule proposed, as `links()` does for a rule without
            // groups, summed over every group.
            proposed += value.iter().count();
            if proposed > self.limits.max_links {
                return Err(RunError::LimitLinks(self.limits.max_links));
            }
            let links: Vec<String> = self
                .absolute_links(&value)
                .into_iter()
                .filter(|link| seen.insert(link.clone()))
                .collect();
            if links.is_empty() {
                continue;
            }
            let name = self
                .package_from(&groups.package)
                .or_else(|| fallback.map(str::to_owned));
            found.push(CrawlGroup {
                name,
                links: number_mirrors(links, groups.mirrors),
            });
        }
        self.restore(&saved);
        self.depth = deepest;
        if found.is_empty() {
            return Err(RunError::NoLinks);
        }
        Ok(found)
    }

    /// The variables a group's steps may change, as the rule's own steps left them.
    fn saved(&self, groups: &Groups) -> Vec<(String, Option<Value>)> {
        let written: BTreeSet<&str> = groups
            .steps
            .iter()
            .map(written_by)
            .chain([LINKS_VARIABLE, PAGE_URL_VARIABLE, groups.entry_variable()])
            .collect();
        written
            .into_iter()
            .map(|name| (name.to_owned(), self.variables.get(name).cloned()))
            .collect()
    }

    /// Puts back what [`Self::saved`] kept, removing what was not there.
    fn restore(&mut self, saved: &[(String, Option<Value>)]) {
        for (name, value) in saved {
            match value {
                Some(value) => self.variables.set(name, value.clone()),
                None => self.variables.unset(name),
            }
        }
    }
}

/// Every link of every group, in order: the flat list a caller that knows nothing of groups
/// reads.
pub(crate) fn flatten(groups: &[CrawlGroup]) -> Vec<String> {
    groups
        .iter()
        .flat_map(|group| group.links.iter().map(|link| link.url.clone()))
        .collect()
}

/// The variable a step writes, with the defaults the executor applies.
fn written_by(step: &Step) -> &str {
    match step {
        Step::Fetch { into, .. } | Step::Form { into, .. } => {
            into.as_deref().unwrap_or(PAGE_VARIABLE)
        }
        Step::Captcha { into, .. } => into.as_deref().unwrap_or(CAPTCHA_VARIABLE),
        Step::FetchJson { into, .. }
        | Step::Regex { into, .. }
        | Step::Decode { into, .. }
        | Step::Redirect { into, .. } => into.as_str(),
    }
}

/// Numbers the mirror sets of one group's links, as `mirrors` states them.
pub(super) fn number_mirrors(links: Vec<String>, mirrors: Option<GroupMirrors>) -> Vec<GroupLink> {
    let sets: Vec<Option<u32>> = match mirrors {
        None => vec![None; links.len()],
        Some(GroupMirrors::All) => vec![Some(1); links.len()],
        Some(GroupMirrors::ByHost) => {
            let mut positions: BTreeMap<String, u32> = BTreeMap::new();
            links
                .iter()
                .map(|link| {
                    let host = Url::parse(link)
                        .ok()
                        .and_then(|url| url.host_str().map(rd_core::host_key))
                        .unwrap_or_default();
                    let position = positions.entry(host).or_insert(0);
                    *position = position.saturating_add(1);
                    Some(*position)
                })
                .collect()
        }
    };
    let mut members: BTreeMap<u32, usize> = BTreeMap::new();
    for set in sets.iter().flatten() {
        *members.entry(*set).or_default() += 1;
    }
    links
        .into_iter()
        .zip(sets)
        .map(|(url, set)| GroupLink {
            url,
            mirror: set.filter(|set| members.get(set).copied().unwrap_or(0) > 1),
        })
        .collect()
}
