//! A two-stage rule at run time (RD-1170-03): the first stage lists the entries a rule with
//! `groups.pick` finds and resolves none of them, the second resolves one chosen entry.
//!
//! **Why two stages.** A series page lists thirty releases, and the links of each sit behind a
//! captcha. Running the group's steps for every entry, as RD-1170-02 does, would put thirty
//! challenges in front of a person who wanted one season in one resolution. So the run stops
//! after the rule's own steps, reads what `pick` names from each entry, and hands the list
//! back together with the variables it wrote. Nothing past that point is fetched until
//! somebody chooses.
//!
//! **The second stage is a run of its own.** Its own budgets, its own cycle set, its depth
//! counted from the page the first stage ended on, and the variables exactly as the first stage
//! left them -- so the tenth release of a page is never refused because nine were fetched
//! before it, and two releases POSTing to the same address are not a cycle. The time a captcha
//! waits for its answer is not counted against the budget: a person is solving it, and the
//! broker keeps its own clock.

use std::collections::BTreeMap;

use regex::Regex;
use url::Url;

use super::{
    Executor,
    error::RunError,
    groups::{CrawlGroup, number_mirrors},
    run::Run,
    steps::first_capture,
    value::{DEVICE_VARIABLE, Variables},
};
use crate::{
    format::Rule,
    groups::{Groups, Pick},
    step::LINKS_VARIABLE,
};

/// Longest attribute value kept; a pattern that caught half a page is not an attribute.
const MAX_VALUE_LENGTH: usize = 120;

/// What the first stage of a two-stage rule found: the entries, and what the second stage
/// starts from.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PickList {
    pub entries: Vec<PickEntry>,
    /// Every variable the rule's own steps wrote, as they left them.
    pub variables: Variables,
}

/// One entry to choose.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PickEntry {
    /// The entry as the rule's steps left it, which the group's steps are handed.
    pub text: String,
    /// What the group's `package` reads from the entry alone, or the rule's own package name.
    pub label: Option<String>,
    /// What each of `pick.attributes` read from the entry; a name it did not match is absent.
    pub attributes: BTreeMap<String, String>,
}

impl Run<'_> {
    /// Lists the entries of `groups.from` with their labels and attributes. `fallback` is the
    /// rule's own package name.
    pub(crate) fn pick_list(
        &mut self,
        groups: &Groups,
        pick: &Pick,
        fallback: Option<&str>,
    ) -> Result<PickList, RunError> {
        // Numbered after the rule's own steps, as a group's refusal is.
        let first = self.rule.steps.len();
        let texts: Vec<String> = self
            .read(first, "groups", &groups.from)?
            .iter()
            .map(str::to_owned)
            .collect();
        // One entry is at least one link once resolved, so more entries than links is over the
        // same limit a rule without `pick` would run into.
        if texts.len() > self.limits.max_links {
            return Err(RunError::LimitLinks(self.limits.max_links));
        }
        let patterns = pick
            .attributes
            .iter()
            .map(|(name, pattern)| {
                Regex::new(pattern)
                    .map(|regex| (name.clone(), regex))
                    .map_err(|error| RunError::Structure {
                        step: first,
                        kind: "pick",
                        detail: format!("the pattern of {name:?} does not compile: {error}"),
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let entry_name = groups.entry_variable();
        let saved = self.variables.get(entry_name).cloned();
        let mut entries = Vec::with_capacity(texts.len());
        for text in texts {
            self.variables.set(entry_name, text.clone());
            let label = self
                .package_from(&groups.package)
                .or_else(|| fallback.map(str::to_owned));
            let attributes = patterns
                .iter()
                .filter_map(|(name, regex)| {
                    let found = regex.captures(&text).and_then(first_capture)?;
                    let value: String = found
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" ")
                        .chars()
                        .take(MAX_VALUE_LENGTH)
                        .collect();
                    (!value.is_empty()).then(|| (name.clone(), value))
                })
                .collect();
            entries.push(PickEntry {
                text,
                label,
                attributes,
            });
        }
        match saved {
            Some(value) => self.variables.set(entry_name, value),
            None => self.variables.unset(entry_name),
        }
        Ok(PickList {
            entries,
            variables: self.variables.clone(),
        })
    }
}

impl Executor<'_> {
    /// The second stage of a two-stage rule: the group's steps for entry `index` of `list`, as
    /// one package. `address` is the address the first stage crawled (`Crawl::address`).
    ///
    /// A refused captcha comes back as `site_rules.captcha_failed`, which a caller tells apart
    /// from every other refusal: the entry is still there to be resolved again.
    pub async fn resolve(
        &self,
        rule: &Rule,
        address: &Url,
        list: &PickList,
        index: usize,
    ) -> Result<CrawlGroup, RunError> {
        let groups = rule.groups.as_ref().ok_or(RunError::NoEntry(index))?;
        let entry = list.entries.get(index).ok_or(RunError::NoEntry(index))?;
        let origin_host = address.host_str().unwrap_or_default().to_owned();
        let mut run = Run::new(self.ports, self.limits, rule, address.clone(), origin_host);
        run.variables = list.variables.clone();
        // This run's own value, should the list have been made by a run without one.
        if let Some(device) = self.ports.device_id {
            run.variables.set(DEVICE_VARIABLE, device.to_owned());
        }
        run.captcha_paused = true;
        run.variables.unset(LINKS_VARIABLE);
        run.variables
            .set(groups.entry_variable(), entry.text.clone());
        let first = rule.steps.len();
        for (offset, step) in groups.steps.iter().enumerate() {
            run.step(first + offset, step).await?;
        }
        run.check_time()?;
        let links = run.links()?;
        let name = run
            .package_from(&groups.package)
            .or_else(|| entry.label.clone());
        Ok(CrawlGroup {
            name,
            links: number_mirrors(links, groups.mirrors),
        })
    }
}
