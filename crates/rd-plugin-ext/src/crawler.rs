//! Folder crawler plugins (RD-104-03).
//!
//! A crawler turns one address into the files behind it — a cloud folder, a directory share,
//! a release page. It proposes, exactly as an intake parser does: what it returns goes
//! through the same review, the same blocklist and the same routing rules a pasted link does,
//! so the worst a bad crawler can do is suggest links a person then declines.
//!
//! What it may *not* do is decided here rather than in the plugin: how many links are taken,
//! whether an answer is a URL at all, and whether a crawler may hand back the very address it
//! was given. That last one matters — without it a crawler could answer with its own input
//! and the caller would crawl it again, for ever.
//!
//! Since RD-110-06 a crawler is not always a plugin: a site rule answers the same question
//! from data, and [`FolderCrawlers::expand`] asks the two kinds in one fixed order. Every
//! rule an answer has to keep is applied to both, in [`FolderCrawlers::accept`].

use std::{
    collections::{BTreeSet, HashMap, HashSet},
    sync::Arc,
};

use anyhow::Result;
use async_trait::async_trait;
use rd_core::AccountId;
use rd_plugin_api::ResolverHost;
use rd_plugin_host::{
    PluginInstaller, PluginType, PluginTypeRegistry,
    extension::{CrawlRefusal, FolderCrawler},
};

use crate::siterules::{RuleOutcome, SiteRules};

/// Most links accepted from one crawler for one address.
///
/// Lower than the host's own ceiling on purpose: this is the number that lands in somebody's
/// review list, and a folder with more files than this is a job for a subscription rather
/// than for one paste.
const MAX_LINKS: usize = 500;

/// What a crawler found behind one address.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CrawledLink {
    /// The address, with any login the crawler put in front of it already taken out.
    pub url: url::Url,
    pub file_name: Option<String>,
    pub size: Option<u64>,
    pub package_hint: Option<String>,
    /// What the source said about this link being one of several copies of the same file
    /// (RD-110-18). A release page knows it; a folder listing does not.
    pub mirror: Option<rd_core::MirrorHint>,
    /// The user name the crawler asked for these files to be fetched under, when it named
    /// one (RD-108-07). Never a password: see [`split_crawled_address`].
    pub login: Option<String>,
}

/// The login a crawler's whole answer asks for, and the addresses it covers.
///
/// A protected share is deliberately **not** an account. An account is a per-provider login
/// with a life of its own -- listed, checked, reused by every link of that provider. A share
/// password authenticates one share, has no provider and no meaning anywhere else. What
/// already fits that exactly is an *auth profile*: a credential scoped to a host and a path
/// prefix, its secret a `vault://` reference, applied by the queue to every address the scope
/// covers. So this is what the caller needs to mint one, and nothing more -- the password
/// itself never passes through here.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShareLogin {
    pub username: String,
    /// Host plus the longest path all the found files share, so the credential reaches the
    /// share's files and stops there.
    pub scope: rd_core::AuthScope,
}

/// Separates a crawled address from the login a crawler put in front of it (RD-108-07).
///
/// `None` when the address cannot be used: not a URL, not `http`/`https`, or -- the one that
/// matters -- carrying a **password** in its userinfo. A crawler may name the user its files
/// are fetched as, because that is not a secret and the address is the only channel the
/// `crawled-link` record has. It may not put a credential there: an address is written to a
/// database column, returned over REST and printed in log lines, so a password in one leaks
/// everywhere at once. Such a link is dropped rather than cleaned up, because a crawler that
/// did it once will have done it to every link in the answer.
#[must_use]
pub fn split_crawled_address(address: &str) -> Option<(url::Url, Option<String>)> {
    let mut url = url::Url::parse(address).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    if url.password().is_some() {
        return None;
    }
    let login = url.username().to_owned();
    if login.is_empty() {
        return Some((url, None));
    }
    // Plain logins only. Percent-encoding here would need a decoder and would let a `:` or
    // an `@` back in through the side door; the user names this is for are already plain.
    if !login
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.'))
    {
        return None;
    }
    url.set_username("").ok()?;
    Some((url, Some(login)))
}

/// The one login the whole answer asks for, or `None` when there is not exactly one.
///
/// Every file must name the same user on the same host, and the scope is the longest path
/// they all sit under. All three are refusals rather than best guesses: a credential minted
/// from a crawler's say-so must reach the files it was given for and nothing else, so a
/// disagreement, a second host or a prefix that has shrunk to `/` ends in no profile at all.
#[must_use]
pub fn share_login(links: &[CrawledLink]) -> Option<ShareLogin> {
    let first = links.first()?;
    let username = first.login.clone()?;
    let host = first.url.host_str()?.to_ascii_lowercase();
    let mut prefix: Vec<&str> = directory_segments(&first.url);
    for link in &links[1..] {
        if link.login.as_deref() != Some(username.as_str())
            || link.url.host_str().map(str::to_ascii_lowercase).as_deref() != Some(host.as_str())
        {
            return None;
        }
        let segments = directory_segments(&link.url);
        let shared = prefix
            .iter()
            .zip(segments.iter())
            .take_while(|(left, right)| left == right)
            .count();
        prefix.truncate(shared);
    }
    if prefix.is_empty() {
        return None;
    }
    let scope = rd_core::AuthScope {
        host,
        include_subdomains: false,
        path_prefix: Some(format!("/{}", prefix.join("/"))),
    };
    Some(ShareLogin { username, scope })
}

/// The path segments of the folder an address sits in, still percent-encoded so they compare
/// the way `AuthScope` later matches them.
fn directory_segments(url: &url::Url) -> Vec<&str> {
    let mut segments: Vec<&str> = url
        .path()
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect();
    // The file's own name is not part of the folder every file shares.
    segments.pop();
    segments
}

/// What asking the installed crawlers about one address produced.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CrawlOutcome {
    /// No installed crawler claims this address; it stays exactly what it was.
    NotClaimed,
    /// The files behind the address. Never empty — an empty answer is a refusal.
    Links(Vec<CrawledLink>),
    /// The address was claimed and produced nothing usable, with a code the interface can
    /// translate. Reported rather than swallowed: an empty or unreachable folder that says
    /// nothing is the defect this job exists to fix.
    Refused { code: String, message: String },
}

/// The stable code used when a plugin refuses without naming one of its own.
const UNSPECIFIED: &str = "plugin.crawl_failed";
/// The stable code for a crawler that answered with an empty list.
const EMPTY: &str = "plugin.crawl_empty";
/// The same for a rule. It is the executor's own code for a run that produced no link, so
/// the interface has one code for the case whether the emptiness was found in the rule or in
/// the acceptance below.
const RULE_EMPTY: &str = "site_rules.no_links";

/// The sources one address is offered to: the installed folder crawlers, newest version of
/// each, and the site rules between the two groups of them.
pub struct FolderCrawlers {
    plugins: Vec<Crawler>,
    rules: Option<Arc<SiteRules>>,
}

/// What the selection needs of one crawler.
///
/// A trait rather than the concrete wrapper so the order and the fallback below can be read
/// — and tested — without a compiled WebAssembly component: what they do is decide, and a
/// decision that can only be exercised through a toolchain is a decision nobody checks.
#[async_trait]
trait CrawlerPlugin: Send + Sync {
    /// Whether this plugin claims `url`, answered from the address alone.
    async fn claims_address(&self, url: &str) -> Result<bool>;
    /// What lies behind `url`, for one account.
    async fn crawl_address(
        &self,
        url: &str,
        account: Option<AccountId>,
    ) -> Result<Result<Vec<rd_plugin_host::extension::CrawledLink>, CrawlRefusal>>;
}

#[async_trait]
impl CrawlerPlugin for FolderCrawler {
    async fn claims_address(&self, url: &str) -> Result<bool> {
        self.claims(url).await
    }

    async fn crawl_address(
        &self,
        url: &str,
        account: Option<AccountId>,
    ) -> Result<Result<Vec<rd_plugin_host::extension::CrawledLink>, CrawlRefusal>> {
        self.crawl(url, account).await
    }
}

struct Crawler {
    /// Plugin id, which is what identifies the crawler across its versions.
    id: String,
    name: String,
    /// Provider slugs this crawler runs for, lowercase. Empty means it needs no account.
    claims: Vec<String>,
    /// Whether this crawler recognises an address by the shape of its path rather than by
    /// its host. Generic crawlers are asked last; see [`FolderCrawlers::load`].
    generic: bool,
    plugin: Box<dyn CrawlerPlugin>,
}

impl FolderCrawlers {
    /// Loads every installed crawler, skipping any that fails to build.
    ///
    /// A broken plugin costs its own feature and nothing else: intake still works, and the
    /// failure is logged rather than taking the LinkGrabber down with it.
    pub async fn load(
        installer: &PluginInstaller,
        host: Option<Arc<dyn ResolverHost>>,
    ) -> Result<Self> {
        Ok(Self::from_registry(
            &PluginTypeRegistry::load(installer).await?,
            host,
        ))
    }

    /// The same, from a registry the adapters share.
    ///
    /// Loading a registry re-verifies and compiles every installed package, so the one `load`
    /// builds for itself is only worth it for a caller that loads a single adapter. Everything
    /// started together passes one registry through all of them.
    #[must_use]
    pub fn from_registry(
        registry: &PluginTypeRegistry,
        host: Option<Arc<dyn ResolverHost>>,
    ) -> Self {
        let mut plugins = registry.instantiate(&PluginType::Crawler, |package| {
            FolderCrawler::new(package.manifest.clone(), &package.component, host.clone()).map(
                |plugin| Crawler {
                    claims: package
                        .manifest
                        .extension
                        .as_ref()
                        .map(|extension| {
                            extension
                                .claims
                                .iter()
                                .map(|claim| claim.to_ascii_lowercase())
                                .collect()
                        })
                        .unwrap_or_default(),
                    generic: package
                        .manifest
                        .extension
                        .as_ref()
                        .is_some_and(|extension| extension.generic),
                    id: package.manifest.id.to_string(),
                    name: package.manifest.name.clone(),
                    plugin: Box::new(plugin),
                },
            )
        });
        keep_newest_version(&mut plugins);
        order(&mut plugins);
        Self {
            plugins,
            rules: None,
        }
    }

    /// Puts the site rules into the selection, in their fixed place: after the crawlers that
    /// name a service, before the generic ones (RD-110-06).
    #[must_use]
    pub fn with_rules(mut self, rules: Arc<SiteRules>) -> Self {
        self.rules = Some(rules);
        self
    }

    /// The rules in the selection, for the interface that edits them (RD-110-08). `None`
    /// when this installation runs without any -- a test double, or `none()` below.
    #[must_use]
    pub fn rules(&self) -> Option<&Arc<SiteRules>> {
        self.rules.as_ref()
    }

    /// An empty set, for a service running without plugins.
    #[must_use]
    pub fn none() -> Self {
        Self {
            plugins: Vec::new(),
            rules: None,
        }
    }

    /// Whether any crawler plugin is installed. Says nothing about the rules.
    #[must_use]
    pub fn has_plugins(&self) -> bool {
        !self.plugins.is_empty()
    }

    /// Whether there is nothing to ask at all: no crawler plugin and no rule. The caller
    /// skips the whole pass on this, so a service with rules but no plugins must not read as
    /// empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty() && self.rules.as_ref().is_none_or(|rules| rules.is_empty())
    }

    /// The provider slugs the installed crawlers run for.
    ///
    /// The caller looks the accounts up: which account a crawl runs as is the application's
    /// decision, not the plugin's, and a plugin naming an account id would be naming one it
    /// has no business knowing.
    #[must_use]
    pub fn providers(&self) -> BTreeSet<String> {
        self.plugins
            .iter()
            .flat_map(|crawler| crawler.claims.iter().cloned())
            .collect()
    }

    /// Asks the sources what lies behind one address.
    ///
    /// **Plugin, then rule, then generic plugin**, and that order is the whole decision. A
    /// crawler plugin that names a service was built for exactly that service and has logic
    /// of its own; a rule is data and describes a service more broadly; a generic crawler
    /// recognises the *shape* of an address rather than a service at all and is the last
    /// resort. Whoever speaks first answers.
    ///
    /// Saying "not mine after all" is not speaking: a plugin that disclaims the address it
    /// claimed, and a rule whose `match` does not claim it, both hand the address to the next
    /// source. An address nobody claims stays exactly what it was, which is what the caller
    /// reads as "this was a link, not a folder".
    ///
    /// `accounts` maps a provider slug to the account a crawl for it runs as. A crawler whose
    /// provider has no account still runs — a public share needs none, and the provider is
    /// the one that gets to say so.
    pub async fn expand(
        &self,
        url: &url::Url,
        accounts: &HashMap<String, AccountId>,
    ) -> CrawlOutcome {
        // `order` sorted the generic crawlers last, so this is where that group begins.
        let generic = self.plugins.partition_point(|crawler| !crawler.generic);
        if let Some(outcome) = self.ask(&self.plugins[..generic], url, accounts).await {
            return outcome;
        }
        if let Some(outcome) = self.ask_rules(url).await {
            return outcome;
        }
        if let Some(outcome) = self.ask(&self.plugins[generic..], url, accounts).await {
            return outcome;
        }
        CrawlOutcome::NotClaimed
    }

    /// Asks one group of crawler plugins; `None` when none of them claimed the address, or
    /// every one that did disclaimed it again.
    async fn ask(
        &self,
        plugins: &[Crawler],
        url: &url::Url,
        accounts: &HashMap<String, AccountId>,
    ) -> Option<CrawlOutcome> {
        for crawler in plugins {
            match crawler.plugin.claims_address(url.as_str()).await {
                Ok(false) => continue,
                Ok(true) => {}
                Err(error) => {
                    tracing::warn!(
                        plugin = %crawler.name,
                        %error,
                        "crawler failed while deciding whether it claims a link"
                    );
                    continue;
                }
            }
            let account = crawler
                .claims
                .iter()
                .find_map(|slug| accounts.get(slug).copied());
            return Some(
                match crawler.plugin.crawl_address(url.as_str(), account).await {
                    Ok(Ok(links)) => Self::accept(&crawler.name, url, links, EMPTY),
                    // "Not mine after all." A crawler that recognises a share by the shape of
                    // its path cannot avoid being wrong sometimes, and being wrong once used to
                    // end the link: the first claimer's answer was the answer, refusal included.
                    // The search goes on instead, and only a crawler that actually read the
                    // address gets to speak for it.
                    Ok(Err(refusal)) if refusal.not_mine => {
                        tracing::debug!(
                            plugin = %crawler.name,
                            "crawler claimed this address and then disclaimed it"
                        );
                        continue;
                    }
                    Ok(Err(refusal)) => Self::refuse(crawler, refusal),
                    Err(error) => {
                        // A crawler that trapped, ran out of fuel or timed out says nothing about
                        // the folder, so the person is told that rather than "it is empty".
                        tracing::warn!(plugin = %crawler.name, %error, "crawler failed");
                        CrawlOutcome::Refused {
                            code: UNSPECIFIED.to_owned(),
                            message: format!("{} could not read this address", crawler.name),
                        }
                    }
                },
            );
        }
        None
    }

    /// Asks the rules, which sit between the two groups of plugins.
    ///
    /// The rule that produced links hands them through the same acceptance a plugin's answer
    /// goes through — no second mechanic — and the package name it read travels as the
    /// `package_hint` of every one of them, which is what `rd_collector::grouping` builds the
    /// package from.
    async fn ask_rules(&self, url: &url::Url) -> Option<CrawlOutcome> {
        match self.rules.as_ref()?.consult(url).await? {
            RuleOutcome::Crawled { rule, crawl } => {
                let package_hint = crawl.package_name;
                // A rule that says its page is one release makes every link it found a
                // mirror of the others (RD-110-18). The key names the rule *and* the address
                // it read, so two pages crawled into one package stay two groups. It names
                // no quality and no language: the rule format carries no per-link metadata,
                // so those are left to the release name.
                let mirror = crawl.mirrors.then(|| rd_core::MirrorHint {
                    group: format!("{rule}|{}", crawl.address),
                    quality: None,
                    language: None,
                });
                let links = crawl
                    .links
                    .into_iter()
                    .map(|found| rd_plugin_host::extension::CrawledLink {
                        url: found,
                        file_name: None,
                        size: None,
                        package_hint: package_hint.clone(),
                        mirror_hint: mirror.clone(),
                    })
                    .collect();
                Some(Self::accept(&rule, url, links, RULE_EMPTY))
            }
            // Every code but "not my page" is a statement about this page, and a statement is
            // reported rather than handed to the next source, which would only produce a
            // second one.
            RuleOutcome::Refused { rule, error } => {
                tracing::info!(
                    rule = %rule,
                    code = error.code(),
                    "a site rule could not read this address"
                );
                Some(CrawlOutcome::Refused {
                    code: error.code().to_owned(),
                    message: format!("{rule}: {error}"),
                })
            }
        }
    }

    /// Turns what a source said into links the collector may act on.
    fn accept(
        source: &str,
        crawled: &url::Url,
        links: Vec<rd_plugin_host::extension::CrawledLink>,
        empty: &str,
    ) -> CrawlOutcome {
        let mut accepted = Vec::new();
        for link in links.into_iter().take(MAX_LINKS) {
            // A proposal that is not usable is dropped rather than reported: the person who
            // pasted the folder cannot fix somebody else's plugin. An address carrying a
            // password is one of these; see `split_crawled_address`.
            let Some((url, login)) = split_crawled_address(&link.url) else {
                tracing::warn!(source, "a crawler proposed an address that cannot be used");
                continue;
            };
            // A source that hands its own input back would be crawled again, and again.
            if &url == crawled {
                tracing::warn!(
                    source,
                    "a crawler proposed the address it was given; dropped"
                );
                continue;
            }
            accepted.push(CrawledLink {
                url,
                file_name: sanitize(link.file_name.as_deref()),
                size: link.size,
                package_hint: sanitize(link.package_hint.as_deref()),
                mirror: link.mirror_hint,
                login,
            });
        }
        if accepted.is_empty() {
            return CrawlOutcome::Refused {
                code: empty.to_owned(),
                message: format!("{source} found no files behind this address"),
            };
        }
        CrawlOutcome::Links(accepted)
    }

    fn refuse(crawler: &Crawler, refusal: CrawlRefusal) -> CrawlOutcome {
        CrawlOutcome::Refused {
            code: refusal.code.unwrap_or_else(|| UNSPECIFIED.to_owned()),
            message: if refusal.message.trim().is_empty() {
                format!("{} could not read this address", crawler.name)
            } else {
                refusal.message
            },
        }
    }
}

/// Drops every installed version of a crawler but the newest.
///
/// `load_verified` hands out one package per installed *version*, newest first, so a machine
/// that still has 1.2.3 next to 1.2.4 of one crawler asked both: the second was asked about an
/// address the first had already crawled, and the same folder was expanded into the review list
/// twice. The first entry for an id wins, which is the highest SemVer, and it runs before
/// [`order`] so the surviving entry keeps its place in the generic-last sequence.
fn keep_newest_version(plugins: &mut Vec<Crawler>) {
    let mut seen = HashSet::new();
    plugins.retain(|crawler| seen.insert(crawler.id.clone()));
}

/// Puts the crawlers into the order they are asked in: the ones that name a service first,
/// the generic ones last, and inside each group by name so the answer does not depend on the
/// order the installer happened to read the directory in.
///
/// The order is the other half of the fallback. A crawler that claims "any address ending in
/// a slash" is right often enough to be worth having and wrong often enough that it must not
/// be asked before a plugin that recognises the actual service.
fn order(plugins: &mut [Crawler]) {
    plugins.sort_by(|left, right| {
        left.generic
            .cmp(&right.generic)
            .then_with(|| left.name.cmp(&right.name))
    });
}

/// Trims a name a stranger chose down to something safe to show and to build a path from.
///
/// The separators go rather than being escaped: a file name and a package hint are both used
/// to build a path further down, and the only guarantee worth making here is that neither can
/// contain one.
pub(crate) fn sanitize(value: Option<&str>) -> Option<String> {
    let cleaned: String = value?
        .chars()
        .filter(|character| !character.is_control() && !matches!(character, '\\'))
        .collect();
    let cleaned = cleaned
        .split('/')
        .map(|segment| segment.trim().trim_matches('.').trim())
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>()
        .join("/");
    (!cleaned.is_empty()).then(|| cleaned.chars().take(255).collect())
}

#[cfg(test)]
#[path = "crawler_tests.rs"]
mod tests;
