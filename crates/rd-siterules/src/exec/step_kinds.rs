//! The body of each of the seven step kinds, one method each; [`super`] keeps the dispatcher
//! that picks one. Split from `steps.rs` (RD-1280-08).

use std::collections::BTreeMap;

use super::super::{
    decode,
    error::RunError,
    guard::host_allowed,
    ports::{CaptchaRequest, Method},
    run::Run,
    value::{CAPTCHA_VARIABLE, Expansion, Value},
};
use super::{first_capture, json_at};
use crate::step::{Decoding, PAGE_VARIABLE, compile_pattern};

impl Run<'_> {
    /// `fetch`: one page, or one per entry of a list placeholder.
    pub(super) async fn fetch_step(
        &mut self,
        index: usize,
        url: &Option<String>,
        into: &Option<String>,
    ) -> Result<(), RunError> {
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
        Ok(())
    }

    /// `fetch-json`: the value at a JSON pointer, or the values of one fetch per entry.
    pub(super) async fn fetch_json_step(
        &mut self,
        index: usize,
        url: &str,
        path: &str,
        into: &str,
    ) -> Result<(), RunError> {
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
        Ok(())
    }

    /// `regex`: the first capture group of the first match, or of every match with `all`.
    pub(super) fn regex_step(
        &mut self,
        index: usize,
        pattern: &str,
        from: &Option<String>,
        into: &str,
        all: bool,
    ) -> Result<(), RunError> {
        let source = from.as_deref().unwrap_or(PAGE_VARIABLE);
        let value = self.read(index, "regex", source)?;
        let regex = compile_pattern(pattern).map_err(|error| RunError::Structure {
            step: index,
            kind: "regex",
            detail: format!("the pattern does not compile: {error}"),
        })?;
        let mut found = Vec::new();
        for text in value.iter() {
            if all {
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
        Ok(())
    }

    /// `decode`: every entry of a variable, or a refusal for the first that does not decode.
    pub(super) fn decode_step(
        &mut self,
        index: usize,
        encoding: Decoding,
        from: &str,
        into: &str,
    ) -> Result<(), RunError> {
        let value = self.read(index, "decode", from)?;
        let mut decoded = Vec::new();
        for text in value.iter() {
            decoded.push(
                decode::decode(encoding, text).ok_or_else(|| RunError::DecodeFailed {
                    step: index,
                    encoding: decode::name(encoding).to_owned(),
                })?,
            );
        }
        self.variables.set(into, Value::list(decoded));
        Ok(())
    }

    /// `form`: submits the expanded fields and keeps the response.
    pub(super) async fn form_step(
        &mut self,
        index: usize,
        url: &str,
        fields: &BTreeMap<String, String>,
        into: &Option<String>,
        json: bool,
    ) -> Result<(), RunError> {
        let expanded = self.expand(index, "form", url)?;
        let target = self.target(index, "form", &expanded)?;
        let mut body = BTreeMap::new();
        for (name, template) in fields {
            body.insert(name.clone(), self.expand(index, "form", template)?);
        }
        let (_, response) = self.fetch_page(target, Method::Post, body, json).await?;
        self.variables
            .set(into.as_deref().unwrap_or(PAGE_VARIABLE), response.body);
        Ok(())
    }

    /// `redirect`: the target each address answers with.
    pub(super) async fn redirect_step(
        &mut self,
        index: usize,
        from: &str,
        into: &str,
    ) -> Result<(), RunError> {
        let value = self.read(index, "redirect", from)?;
        let mut targets = Vec::new();
        for text in value.iter() {
            let url = self.target(index, "redirect", text)?;
            targets.push(self.redirect_target_of(index, &url).await?);
        }
        self.variables.set(into, Value::list(targets));
        Ok(())
    }

    /// `captcha`: hands the challenge to the broker, held to the rule's own hosts.
    pub(super) async fn captcha_step(
        &mut self,
        index: usize,
        challenge: &str,
        sitekey: &Option<String>,
        into: &Option<String>,
        page: &Option<String>,
        invisible: bool,
    ) -> Result<(), RunError> {
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
                challenge: challenge.to_owned(),
                sitekey,
                page_url,
                invisible,
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
        Ok(())
    }
}
