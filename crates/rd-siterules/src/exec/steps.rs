//! What each of the seven step kinds does when it runs, and how the run ends.
//!
//! Every step either writes a variable or refuses. There is no step that half-succeeds: a
//! `regex` that matched nothing is a page whose structure changed, and saying so is the whole
//! point of RD-110-09's `strukturell` verdict. The one exception is the package name, which
//! is a hint rather than a result — a rule that found its links but not its title yields the
//! links and no name.

use std::collections::BTreeMap;

use regex::Regex;
use url::Url;

use super::{
    decode,
    error::RunError,
    guard::{host_allowed, is_public, literal_address},
    ports::{CaptchaRequest, Method},
    run::Run,
    value::{CAPTCHA_VARIABLE, ExpandError, Expansion, PAGE_URL_VARIABLE, Value},
};
use crate::{
    format::PackageSource,
    step::{LINKS_VARIABLE, PAGE_VARIABLE, Step, compile_pattern},
};

/// The longest package name a run yields, in characters -- the length a package name may have
/// in the queue.
pub(crate) const MAX_PACKAGE_NAME_CHARS: usize = 200;

impl Run<'_> {
    /// Runs one step of the rule.
    pub(crate) async fn step(&mut self, index: usize, step: &Step) -> Result<(), RunError> {
        self.check_time()?;
        match step {
            Step::Fetch { url, into } => {
                let target = match url {
                    Some(template) => match self.expand_each(index, "fetch", template)? {
                        Expansion::One(expanded) => self.target(index, "fetch", &expanded)?,
                        Expansion::Each(entries) => {
                            let bodies = self
                                .fetch_each(index, "fetch", &entries, |body| Ok(Value::One(body)))
                                .await?;
                            self.variables
                                .set(into.as_deref().unwrap_or(PAGE_VARIABLE), bodies);
                            return Ok(());
                        }
                    },
                    None => self.address.clone(),
                };
                let (_, response) = self
                    .fetch_page(target, Method::Get, BTreeMap::new(), false)
                    .await?;
                self.variables
                    .set(into.as_deref().unwrap_or(PAGE_VARIABLE), response.body);
            }
            Step::FetchJson { url, path, into } => {
                let expanded = match self.expand_each(index, "fetch-json", url)? {
                    Expansion::One(expanded) => expanded,
                    Expansion::Each(entries) => {
                        let found = self
                            .fetch_each(index, "fetch-json", &entries, |body| {
                                json_at(index, &body, path)
                            })
                            .await?;
                        self.variables.set(into, found);
                        return Ok(());
                    }
                };
                let target = self.target(index, "fetch-json", &expanded)?;
                let (_, response) = self
                    .fetch_page(target, Method::Get, BTreeMap::new(), false)
                    .await?;
                let value = json_at(index, &response.body, path)?;
                self.variables.set(into, value);
            }
            Step::Regex {
                pattern,
                from,
                into,
                all,
            } => {
                let source = from.as_deref().unwrap_or(PAGE_VARIABLE);
                let value = self.read(index, "regex", source)?;
                let regex = compile_pattern(pattern).map_err(|error| RunError::Structure {
                    step: index,
                    kind: "regex",
                    detail: format!("the pattern does not compile: {error}"),
                })?;
                let mut found = Vec::new();
                for text in value.iter() {
                    if *all {
                        found.extend(regex.captures_iter(text).filter_map(first_capture));
                    } else {
                        found.extend(regex.captures(text).and_then(first_capture));
                    }
                }
                if found.is_empty() {
                    return Err(RunError::Structure {
                        step: index,
                        kind: "regex",
                        detail: format!("{pattern:?} matched nothing in {source:?}"),
                    });
                }
                self.variables.set(into, Value::list(found));
            }
            Step::Decode {
                encoding,
                from,
                into,
            } => {
                let value = self.read(index, "decode", from)?;
                let mut decoded = Vec::new();
                for text in value.iter() {
                    decoded.push(decode::decode(*encoding, text).ok_or_else(|| {
                        RunError::DecodeFailed {
                            step: index,
                            encoding: decode::name(*encoding).to_owned(),
                        }
                    })?);
                }
                self.variables.set(into, Value::list(decoded));
            }
            Step::Form {
                url,
                fields,
                into,
                json,
            } => {
                let expanded = self.expand(index, "form", url)?;
                let target = self.target(index, "form", &expanded)?;
                let mut body = BTreeMap::new();
                for (name, template) in fields {
                    body.insert(name.clone(), self.expand(index, "form", template)?);
                }
                let (_, response) = self.fetch_page(target, Method::Post, body, *json).await?;
                self.variables
                    .set(into.as_deref().unwrap_or(PAGE_VARIABLE), response.body);
            }
            Step::Redirect { from, into } => {
                let value = self.read(index, "redirect", from)?;
                let mut targets = Vec::new();
                for text in value.iter() {
                    let url = self.target(index, "redirect", text)?;
                    targets.push(self.redirect_target_of(index, &url).await?);
                }
                self.variables.set(into, Value::list(targets));
            }
            Step::Captcha {
                challenge,
                sitekey,
                into,
                page,
                invisible,
            } => {
                let solver = self.ports.captcha.ok_or_else(|| RunError::CaptchaFailed {
                    step: index,
                    reason: "no captcha broker is available to this run".to_owned(),
                })?;
                let sitekey = match sitekey {
                    Some(template) => Some(self.expand(index, "captcha", template)?),
                    None => None,
                };
                let page_url = match page {
                    Some(template) => {
                        let expanded = self.expand(index, "captcha", template)?;
                        self.target(index, "captcha", &expanded)?
                    }
                    None => self.current_page(index)?,
                };
                // Sent to the solver and the person's browser, so held to the rule's own hosts
                // like every page it fetches (RD-1190-22): a rule cannot spend the person's
                // solver credit on another site's captcha.
                if !host_allowed(self.rule, &self.origin_host, &page_url) {
                    return Err(RunError::TargetNotAllowed {
                        url: page_url.to_string(),
                    });
                }
                let asked = self.ports.clock.elapsed();
                let answer = solver
                    .solve(CaptchaRequest {
                        challenge: challenge.clone(),
                        sitekey,
                        page_url,
                        invisible: *invisible,
                    })
                    .await;
                self.captcha_waited(self.ports.clock.elapsed().saturating_sub(asked));
                let token = answer.map_err(|reason| RunError::CaptchaFailed {
                    step: index,
                    reason,
                })?;
                if token.is_empty() {
                    return Err(RunError::CaptchaFailed {
                        step: index,
                        reason: "the broker returned an empty answer".to_owned(),
                    });
                }
                self.variables
                    .set(into.as_deref().unwrap_or(CAPTCHA_VARIABLE), token);
            }
        }
        Ok(())
    }

    /// The links the run produced: absolute, deduplicated, in the order the rule found them.
    pub(crate) fn links(&self) -> Result<Vec<String>, RunError> {
        let value = self
            .variables
            .get(LINKS_VARIABLE)
            .ok_or(RunError::NoLinks)?;
        if value.iter().count() > self.limits.max_links {
            return Err(RunError::LimitLinks(self.limits.max_links));
        }
        let links = self.absolute_links(value);
        if links.is_empty() {
            return Err(RunError::NoLinks);
        }
        Ok(links)
    }

    /// What a link variable holds, as links: absolute against the page in hand, http(s)
    /// only, never on a literal local address, deduplicated, in order. Empty when nothing
    /// survives.
    pub(crate) fn absolute_links(&self, value: &Value) -> Vec<String> {
        let base = self
            .current_page(0)
            .unwrap_or_else(|_| self.address.clone());
        let mut links: Vec<String> = Vec::new();
        for text in value.iter() {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                continue;
            }
            let Ok(url) = base.join(trimmed) else {
                continue;
            };
            if !matches!(url.scheme(), "http" | "https") {
                continue;
            }
            // A rule's *output* is not fetched here, it is handed to the download engine —
            // so a link on a literal local address would point that engine at this machine.
            // Dropped rather than delivered; if nothing survives, the run refuses as empty.
            if literal_address(&url).is_some_and(|address| !is_public(address)) {
                continue;
            }
            let link = url.to_string();
            if !links.contains(&link) {
                links.push(link);
            }
        }
        links
    }

    /// The package name, or `None` when the source found nothing.
    pub(crate) fn package_name(&self) -> Option<String> {
        self.package_from(&self.rule.package)
    }

    /// What `source` reads from the variables in hand, whitespace collapsed; `None` when it
    /// finds nothing. A group (RD-1170-02) reads its own source the same way.
    pub(crate) fn package_from(&self, source: &PackageSource) -> Option<String> {
        let found = match source {
            PackageSource::Title => {
                let page = self.variables.get(PAGE_VARIABLE)?.first()?;
                let title = Regex::new("(?is)<title[^>]*>(.*?)</title>").ok()?;
                title
                    .captures(page)?
                    .get(1)
                    .map(|group| group.as_str().to_owned())?
            }
            PackageSource::Regex { pattern, source } => {
                let value = self
                    .variables
                    .get(source.as_deref().unwrap_or(PAGE_VARIABLE))?;
                let regex = compile_pattern(pattern).ok()?;
                value
                    .iter()
                    .find_map(|text| regex.captures(text).and_then(first_capture))?
            }
            PackageSource::Variable { name } => self.variables.get(name)?.first()?.to_owned(),
        };
        // Cut to what a package name may be in the queue (RD-1190-22): a page returned it whole,
        // up to the body limit, and the trial run handed that back.
        let cleaned = found
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(MAX_PACKAGE_NAME_CHARS)
            .collect::<String>();
        let cleaned = cleaned.trim_end();
        (!cleaned.is_empty()).then(|| cleaned.to_owned())
    }

    fn expand(&self, index: usize, kind: &'static str, template: &str) -> Result<String, RunError> {
        self.variables
            .expand(template)
            .map_err(|missing| RunError::Structure {
                step: index,
                kind,
                detail: format!("nothing has written the variable {:?}", missing.0),
            })
    }

    /// [`Self::expand`] for the two steps that run once per entry of a list (RD-180-18).
    fn expand_each(
        &self,
        index: usize,
        kind: &'static str,
        template: &str,
    ) -> Result<Expansion, RunError> {
        self.variables
            .expand_each(template)
            .map_err(|error| RunError::Structure {
                step: index,
                kind,
                detail: match error {
                    ExpandError::Missing(missing) => {
                        format!("nothing has written the variable {:?}", missing.0)
                    }
                    ExpandError::SeveralLists(names) => format!(
                        "the variables {names:?} are all lists; an address runs over one list, \
                         not several"
                    ),
                },
            })
    }

    /// One GET per entry of a list placeholder (RD-180-18), with what `parse` makes of each
    /// answer collected in order into one list.
    ///
    /// Every target is resolved before the first request, against the page the step started
    /// from, so a relative template does not drift onto the page the previous entry fetched.
    /// The entries are siblings: each sits one request below that page, and the run goes on
    /// from the deepest of them. Every request passes `fetch_page` and so every bolt and
    /// budget on its own; `max_pages` is what ends a list longer than the run may ask.
    async fn fetch_each(
        &mut self,
        index: usize,
        kind: &'static str,
        entries: &[String],
        parse: impl Fn(String) -> Result<Value, RunError>,
    ) -> Result<Value, RunError> {
        let targets = entries
            .iter()
            .map(|entry| self.target(index, kind, entry))
            .collect::<Result<Vec<_>, _>>()?;
        let start = self.depth;
        let mut deepest = start;
        let mut found = Vec::new();
        for target in targets {
            self.depth = start;
            let (_, response) = self
                .fetch_page(target, Method::Get, BTreeMap::new(), false)
                .await?;
            deepest = deepest.max(self.depth);
            found.extend(parse(response.body)?.iter().map(str::to_owned));
        }
        self.depth = deepest;
        Ok(Value::list(found))
    }

    pub(crate) fn read(
        &self,
        index: usize,
        kind: &'static str,
        name: &str,
    ) -> Result<Value, RunError> {
        let value = self
            .variables
            .get(name)
            .cloned()
            .ok_or_else(|| RunError::Structure {
                step: index,
                kind,
                detail: format!("nothing has written the variable {name:?}"),
            })?;
        if value.is_empty() {
            return Err(RunError::Structure {
                step: index,
                kind,
                detail: format!("the variable {name:?} is empty"),
            });
        }
        Ok(value)
    }

    /// An address from a rule, absolute or relative to the page in hand.
    fn target(&self, index: usize, kind: &'static str, text: &str) -> Result<Url, RunError> {
        let trimmed = text.trim();
        if let Ok(url) = Url::parse(trimmed) {
            return Ok(url);
        }
        self.current_page(index)?
            .join(trimmed)
            .map_err(|error| RunError::Structure {
                step: index,
                kind,
                detail: format!("{trimmed:?} is not an address: {error}"),
            })
    }

    fn current_page(&self, index: usize) -> Result<Url, RunError> {
        let text = self
            .variables
            .get(PAGE_URL_VARIABLE)
            .and_then(Value::first)
            .unwrap_or_default();
        Url::parse(text).map_err(|error| RunError::Structure {
            step: index,
            kind: "page",
            detail: format!("the current page address is unusable: {error}"),
        })
    }
}

/// The first capture group of a match, or the whole match when the pattern has no group.
pub(crate) fn first_capture(captures: regex::Captures<'_>) -> Option<String> {
    captures
        .get(1)
        .or_else(|| captures.get(0))
        .map(|group| group.as_str().to_owned())
}

/// The value a JSON pointer names, as a string or a list of strings.
fn json_at(index: usize, body: &str, pointer: &str) -> Result<Value, RunError> {
    let structure = |detail: String| RunError::Structure {
        step: index,
        kind: "fetch-json",
        detail,
    };
    let document: serde_json::Value =
        serde_json::from_str(body).map_err(|error| structure(format!("not JSON: {error}")))?;
    let value = if pointer.is_empty() {
        &document
    } else {
        document
            .pointer(pointer)
            .ok_or_else(|| structure(format!("{pointer:?} names nothing in the answer")))?
    };
    match value {
        serde_json::Value::Array(items) => {
            let mut strings = Vec::with_capacity(items.len());
            for item in items {
                strings.push(scalar(item).ok_or_else(|| {
                    structure(format!(
                        "{pointer:?} holds a list with a value that is not text"
                    ))
                })?);
            }
            Ok(Value::list(strings))
        }
        other => scalar(other)
            .map(Value::One)
            .ok_or_else(|| structure(format!("{pointer:?} is not text"))),
    }
}

/// A JSON scalar as the text a later step can work on; a list or an object is not one.
fn scalar(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(text) => Some(text.clone()),
        serde_json::Value::Number(number) => Some(number.to_string()),
        serde_json::Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}
