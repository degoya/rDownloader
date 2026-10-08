//! What a site rule's run proposes to the collector: one link per address it found, with the
//! package it belongs in and the mirror group it is part of.
//!
//! Split out of `crawler.rs` with RD-1170-02, when a rule could first yield several packages.
//! Nothing here decides anything a rule did not state: the package name travels as the
//! `package_hint` `rd_collector::grouping` builds packages from, and a mirror set travels as
//! the declared mirror key `rd_collector::mirrors` groups by (RD-110-18).

use rd_plugin_host::extension::CrawledLink;
use rd_siterules::Crawl;

/// The proposals behind one rule run. `rule` is the rule's name.
///
/// A mirror key names the rule *and* the address it read, so two pages crawled into one
/// package stay two groups; for a rule with `groups` it also names the group and the set,
/// so the first episode at two hosters is one group and the second episode another. It names
/// no quality and no language: the rule format carries neither per link, so those are left
/// to the release name.
pub(super) fn proposals(rule: &str, crawl: Crawl) -> Vec<CrawledLink> {
    if crawl.groups.is_empty() {
        let mirror = crawl
            .mirrors
            .then(|| hint(format!("{rule}|{}", crawl.address)));
        return crawl
            .links
            .into_iter()
            .map(|found| proposal(found, crawl.package_name.clone(), mirror.clone()))
            .collect();
    }
    let page = crawl.address;
    crawl
        .groups
        .into_iter()
        .enumerate()
        .flat_map(|(index, group)| {
            let page = &page;
            let package = group.name.or_else(|| crawl.package_name.clone());
            group.links.into_iter().map(move |link| {
                let mirror = link
                    .mirror
                    .map(|set| hint(format!("{rule}|{page}|{index}|{set}")));
                proposal(link.url, package.clone(), mirror)
            })
        })
        .collect()
}

fn hint(group: String) -> rd_core::MirrorHint {
    rd_core::MirrorHint {
        group,
        quality: None,
        language: None,
    }
}

fn proposal(
    url: String,
    package_hint: Option<String>,
    mirror_hint: Option<rd_core::MirrorHint>,
) -> CrawledLink {
    CrawledLink {
        url,
        file_name: None,
        size: None,
        package_hint,
        mirror_hint,
    }
}

#[cfg(test)]
#[path = "rule_links_tests.rs"]
mod tests;
