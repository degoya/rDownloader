//! MCP tools for writing a site rule (RD-120-32).
//!
//! RD-120-29 covered reading the rules and switching them; writing one was out because a rule
//! is tried against a live page until its selectors hold. `test_site_rule` is that trying, on
//! the same route the editor calls, so a caller can do what the editor does in the same order.
//! Since RD-130-07 every rule is the person's own, the ones imported from the signed release
//! file included, so all of them can be written here.

use axum::{
    Json,
    extract::{Path, State},
};
use rmcp::{handler::server::wrapper::Parameters, tool, tool_router};

use super::{
    RdMcpServer,
    error::{McpToolResult, respond},
    params_config::IdParams,
    params_handling::{SiteRuleParams, TestSiteRuleParams, UpdateSiteRuleParams, body},
};
use crate::site_rules_handlers as rules;

#[tool_router(router = site_rules_router, vis = "pub(crate)")]
impl RdMcpServer {
    #[tool(
        description = "Create a site rule: a step program that finds the links on a release page. `rule` is the rule document: {id, name, group, version, match: {hosts, paths}, steps: [{kind: fetch}, {kind: regex, pattern, into, all}, ...], package, probe, checked} — list_site_rules shows the document of every rule under `rule`. Step kinds: fetch, fetch-json, regex, decode, form, redirect, captcha; the steps end with the links in the variable `links`, and `package` ({from: title | regex | variable}) names the package. Optional `mirrors: true` says every link of the page is a copy of one file. A page that lists several releases uses `groups` instead (never both): {from: the variable holding one entry per package, into: the entry's variable (default entry), steps: the same step kinds run once per entry and ending with that entry's `links`, package: that entry's name, mirrors: by-host | all}. by-host: the n-th link at one hoster is a copy of the n-th link at every other hoster of the same entry; all: every link of the entry is one file. Example, one package per release with its hosters as mirrors: steps [{kind: fetch}, {kind: regex, pattern: '(?s)<div class=release>(.*?)</div>', into: releases, all: true}], groups {from: releases, steps: [{kind: regex, from: entry, pattern: 'href=(https?://[^ >]+)', into: links, all: true}], package: {from: regex, pattern: '<h2>(.*?)</h2>', source: entry}, mirrors: by-host}. A page whose releases each cost a captcha (a series page) adds `pick` to `groups` and becomes two-stage: the rule's steps only list the entries with the attributes pick reads, and the group's steps run later for the entries someone chose (list_page_entries, then resolve_page_entries). pick: {attributes: {name: pattern}} -- season, episode, resolution, language, hoster are the names the LinkGrabber groups and filters by; the first capture applied to the entry is the value. For such pages `form` may send `json: true` (the fields as one JSON object), `captcha` may name `page` (the page its widget sits on, e.g. '${url}') and say `invisible: true`, and the variable device_id holds this installation's stable 32-hex value where a page's script sends a fingerprint. Example, serienjunkies.org: steps [{kind: fetch}, {kind: regex, pattern: 'data-mediaid=\"([0-9a-f]+)\"', into: media}, {kind: regex, pattern: 'data-captchasitekey=\"([^\"]+)\"', into: sitekey}, {kind: fetch, url: 'https://serienjunkies.org/api/media/${media}/releases', into: api}, {kind: regex, from: api, pattern: '(\\{\"_id\":\"[0-9a-f]+\"[^{}]*\\})', into: releases, all: true}], groups {from: releases, pick: {attributes: {season: '\"season\":(\\d+)', episode: '\"episode\":(\\d+)', resolution: '\"resolution\":\"([^\"]+)\"'}}, steps: [{kind: regex, from: entry, pattern: '\"_id\":\"([0-9a-f]+)\"', into: release}, {kind: captcha, challenge: recaptcha-v2, sitekey: '${sitekey}', page: '${url}', invisible: true, into: token}, {kind: form, url: 'https://serienjunkies.org/api/releases/${release}/downloads/ddownload', fields: {recaptchaToken: '${token}', fphash: '${device_id}'}, json: true, into: answer}, {kind: regex, from: answer, pattern: '\"url\":\"([^\"]+)\"', into: links, all: true}], package: {from: regex, pattern: '\"name\":\"([^\"]+)\"', source: entry}}. Its id must be new. enabled defaults to false, as in the editor. Try it with test_site_rule first."
    )]
    pub async fn create_site_rule(
        &self,
        Parameters(params): Parameters<SiteRuleParams>,
    ) -> McpToolResult {
        let result = async {
            let request = body(serde_json::json!({
                "rule": params.rule,
                "enabled": params.enabled.unwrap_or(false),
            }))?;
            let Json(answer) =
                rules::create_site_rule(State(self.state.clone()), Json(request)).await?;
            Ok(answer)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Replace a site rule (id as list_site_rules gives it). `rule` is the whole rule document, in the format create_site_rule describes (groups and mirrors included), and its id must equal `id`; renaming a rule is a delete and a create."
    )]
    pub async fn update_site_rule(
        &self,
        Parameters(params): Parameters<UpdateSiteRuleParams>,
    ) -> McpToolResult {
        let result = async {
            let request = body(serde_json::json!({
                "rule": params.rule,
                "enabled": params.enabled.unwrap_or(false),
            }))?;
            let Json(answer) =
                rules::update_site_rule(State(self.state.clone()), Path(params.id), Json(request))
                    .await?;
            Ok(answer)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Delete a site rule. To keep it but stop it being consulted, switch it off with set_site_rule_enabled instead."
    )]
    pub async fn delete_site_rule(
        &self,
        Parameters(params): Parameters<IdParams>,
    ) -> McpToolResult {
        respond(
            rules::delete_site_rule(State(self.state.clone()), Path(params.id))
                .await
                .map(|Json(answer)| answer),
        )
    }

    #[tool(
        description = "Try a site rule against a live page without storing it: fetches `address` and runs the rule's steps, answering with the links it found and where each step stopped. A rule with `groups` also answers `groups`: one entry per package, with its `name` and its `links`, each link with its `mirror` set (links of one group with the same number are copies of one file). A two-stage rule (groups with `pick`) answers `entries` instead -- each with its `label` and `attributes` -- and runs the first stage only: no link is fetched and no captcha asked. `error` is the stable code of a run that produced nothing."
    )]
    pub async fn test_site_rule(
        &self,
        Parameters(params): Parameters<TestSiteRuleParams>,
    ) -> McpToolResult {
        let result = async {
            let request = body(serde_json::json!({
                "rule": params.rule,
                "address": params.address,
            }))?;
            let Json(answer) =
                rules::test_site_rule(State(self.state.clone()), Json(request)).await?;
            Ok(answer)
        }
        .await;
        respond(result)
    }
}
