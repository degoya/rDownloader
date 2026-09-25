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
    guard::{is_public, literal_address},
    ports::{CaptchaRequest, Method},
    run::Run,
    value::{CAPTCHA_VARIABLE, PAGE_URL_VARIABLE, Value},
};
use crate::{
    format::PackageSource,
    step::{LINKS_VARIABLE, PAGE_VARIABLE, Step},
};

impl Run<'_> {
    /// Runs one step of the rule.
    pub(crate) async fn step(&mut self, index: usize, step: &Step) -> Result<(), RunError> {
        self.check_time()?;
        match step {
            Step::Fetch { url, into } => {
                let target = match url {
                    Some(template) => {
                        self.target(index, "fetch", &self.expand(index, "fetch", template)?)?
                    }
                    None => self.address.clone(),
                };
                let (_, response) = self
                    .fetch_page(target, Method::Get, BTreeMap::new())
                    .await?;
                self.variables
                    .set(into.as_deref().unwrap_or(PAGE_VARIABLE), response.body);
            }
            Step::FetchJson { url, path, into } => {
                let expanded = self.expand(index, "fetch-json", url)?;
                let target = self.target(index, "fetch-json", &expanded)?;
                let (_, response) = self
                    .fetch_page(target, Method::Get, BTreeMap::new())
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
                let regex = Regex::new(pattern).map_err(|error| RunError::Structure {
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
            Step::Form { url, fields, into } => {
                let expanded = self.expand(index, "form", url)?;
                let target = self.target(index, "form", &expanded)?;
                let mut body = BTreeMap::new();
                for (name, template) in fields {
                    body.insert(name.clone(), self.expand(index, "form", template)?);
                }
                let (_, response) = self.fetch_page(target, Method::Post, body).await?;
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
            } => {
                let solver = self.ports.captcha.ok_or_else(|| RunError::CaptchaFailed {
                    step: index,
                    reason: "no captcha broker is available to this run".to_owned(),
                })?;
                let sitekey = match sitekey {
                    Some(template) => Some(self.expand(index, "captcha", template)?),
                    None => None,
                };
                let page_url = self.current_page(index)?;
                let token = solver
                    .solve(CaptchaRequest {
                        challenge: challenge.clone(),
                        sitekey,
                        page_url,
                    })
                    .await
                    .map_err(|reason| RunError::CaptchaFailed {
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
        if links.is_empty() {
            return Err(RunError::NoLinks);
        }
        Ok(links)
    }

    /// The package name, or `None` when the source found nothing.
    pub(crate) fn package_name(&self) -> Option<String> {
        let found = match &self.rule.package {
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
                let regex = Regex::new(pattern).ok()?;
                value
                    .iter()
                    .find_map(|text| regex.captures(text).and_then(first_capture))?
            }
            PackageSource::Variable { name } => self.variables.get(name)?.first()?.to_owned(),
        };
        let cleaned = found.split_whitespace().collect::<Vec<_>>().join(" ");
        (!cleaned.is_empty()).then_some(cleaned)
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

    fn read(&self, index: usize, kind: &'static str, name: &str) -> Result<Value, RunError> {
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
fn first_capture(captures: regex::Captures<'_>) -> Option<String> {
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
