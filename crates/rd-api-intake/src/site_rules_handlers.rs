//! The site-rule settings page over REST (RD-110-08).
//!
//! Five things happen here that are worth stating before the code says them.
//!
//! **Every rule is the person's own** (RD-130-07). Nothing but the switched-off examples for
//! free sites arrives with the binary (RD-1230-03); rules for other sites come from an exchange
//! file somebody exported, and each can be edited, duplicated, switched and removed.
//!
//! **Every body is parsed and validated before it is stored**, through `rd_siterules::Rule`
//! and `Rule::validate`. A rule from a file somebody was sent is untrusted input in the strict
//! sense: it names hosts to fetch and patterns to run. It gets no shortcut.
//!
//! **An import arrives as it was exported** (RD-1230-03): no signature, each rule with the
//! switch it had at the exporter. What a person agrees to is the preview
//! (`import/preview`): the list of rules, new or replacing or the same, before anything is
//! stored -- and a stored rule of the same id is replaced only when the request names it in
//! `replace`, which the dialog asks first.
//!
//! **Every write records where the body came from** (RD-1200-05): an import, the editor, an
//! MCP tool or the example list. A switch keeps the origin; an edit replaces it.
//!
//! **A write takes effect without a restart.** The catalogue in force is replaced after every
//! change, through `rd_plugin_ext::SiteRules::replace`, which exists for exactly this.

use std::collections::BTreeMap;

use axum::{
    Json,
    body::Bytes,
    extract::{Path, State},
};
use rd_db::NewUserSiteRule;
/// What a write records as the rule's origin; the MCP tools name theirs through it.
pub use rd_db::SiteRuleOriginKind;
use rd_siterules::Rule;
use url::Url;

use crate::{
    AppState,
    dto::MessageResponse,
    error::ApiError,
    site_rules_dto::{
        ImportSiteRulesResponse, ImportedSiteRuleResponse, SaveSiteRuleRequest,
        SiteRuleCheckResponse, SiteRuleDocument, SiteRuleDocumentEntry, SiteRuleExamplesResponse,
        SiteRuleExportQuery, SiteRuleGroupResponse, SiteRuleImportPreviewResponse,
        SiteRuleImportRequest, SiteRuleOriginResponse, SiteRuleResponse, SiteRuleSwitchRequest,
        SiteRulesClearRequest, SiteRulesClearResponse, SiteRulesResponse, TestSiteRuleRequest,
        TestSiteRuleResponse, TestedEntryResponse, TestedGroupLinkResponse, TestedGroupResponse,
        TestedLinkResponse,
    },
    site_rules_service,
};

mod manage;
mod transfer;
mod write;

pub use manage::*;
pub use transfer::*;
pub use write::*;

/// Reads a rule body and refuses one that could not work, with the code the interface
/// translates.
fn parse_rule(body: &serde_json::Value) -> Result<Rule, ApiError> {
    let rule: Rule = serde_json::from_value(body.clone()).map_err(|error| {
        ApiError::bad_request("site_rules.invalid_rule", error.to_string())
            .with_param("reason", error.to_string())
    })?;
    rule.validate().map_err(|error| {
        ApiError::bad_request("site_rules.invalid_rule", error.to_string())
            .with_param("rule", rule.id.clone())
            .with_param("reason", error.to_string())
    })?;
    Ok(rule)
}

/// Puts the rules that are now stored into the selection, so a change takes effect on the
/// next paste rather than after a restart.
async fn reload(state: &AppState) {
    let Some(rules) = state.crawlers.rules() else {
        return;
    };
    rules.replace(site_rules_service::catalogue(&state.database).await);
}

fn check_response(check: &rd_db::SiteRuleCheck) -> SiteRuleCheckResponse {
    SiteRuleCheckResponse {
        verdict: check.verdict.clone(),
        code: check.code.clone(),
        links: check.links,
        pages: check.pages,
        checked_at: check.checked_at.clone(),
    }
}

fn origin_response(origin: SiteRuleOriginKind) -> SiteRuleOriginResponse {
    SiteRuleOriginResponse {
        kind: origin.as_str().to_owned(),
    }
}

/// One row built from a parsed rule.
fn row(
    rule: &Rule,
    stored: &rd_db::UserSiteRule,
    active: bool,
    checks: &BTreeMap<String, rd_db::SiteRuleCheck>,
) -> SiteRuleResponse {
    SiteRuleResponse {
        id: rule.id.clone(),
        name: rule.name.clone(),
        description: rule.description.clone(),
        group: rule.group.clone(),
        hosts: rule.matches.hosts.clone(),
        version: rule.version,
        probe: rule.probe.clone(),
        mirrors: rule.mirrors,
        steps: rule.steps.len(),
        enabled: stored.enabled,
        active,
        rule: serde_json::to_value(rule).unwrap_or(serde_json::Value::Null),
        check: checks.get(&rule.id).map(check_response),
        origin: origin_response(stored.origin),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/site-rules",
    tag = "configuration",
    responses((status = 200, body = SiteRulesResponse))
)]
pub async fn list_site_rules(
    State(state): State<AppState>,
) -> Result<Json<SiteRulesResponse>, ApiError> {
    let switches = site_rules_service::Switches::load(&state.database).await;
    let checks = site_rules_service::checks(&state.database).await;
    let mut rules: Vec<SiteRuleResponse> = Vec::new();
    for stored in state.database.list_site_rules().await? {
        let active = stored.enabled && switches.group_on(&stored.group);
        // A body an older build wrote still gets a row: the person has to be able to see it
        // and delete it, and a list that silently drops what it cannot parse is how a rule
        // becomes a ghost. What it does not get is the fields only a parsed rule has.
        match serde_json::from_value::<Rule>(stored.rule.clone()) {
            Ok(rule) => rules.push(row(&rule, &stored, active, &checks)),
            Err(_) => rules.push(SiteRuleResponse {
                id: stored.id.clone(),
                name: stored.name.clone(),
                description: None,
                group: stored.group.clone(),
                hosts: Vec::new(),
                version: 0,
                probe: String::new(),
                mirrors: false,
                steps: 0,
                enabled: stored.enabled,
                active: false,
                check: checks.get(&stored.id).map(check_response),
                origin: origin_response(stored.origin),
                rule: stored.rule,
            }),
        }
    }
    rules.sort_by(|left, right| {
        left.group
            .cmp(&right.group)
            .then_with(|| left.name.cmp(&right.name))
    });
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for rule in &rules {
        *counts.entry(rule.group.clone()).or_default() += 1;
    }
    let groups = counts
        .into_iter()
        .map(|(group, rules)| SiteRuleGroupResponse {
            enabled: switches.group_on(&group),
            group,
            rules,
        })
        .collect();
    Ok(Json(SiteRulesResponse { rules, groups }))
}

#[utoipa::path(
    delete,
    path = "/api/v1/site-rules/{id}",
    tag = "configuration",
    params(("id" = String, Path, description = "The rule's own identifier")),
    responses((status = 200, body = MessageResponse))
)]
pub async fn delete_site_rule(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<MessageResponse>, ApiError> {
    if !state.database.delete_site_rule(&id).await? {
        return Err(not_found(&id));
    }
    reload(&state).await;
    Ok(Json(MessageResponse::new(
        "site_rules.deleted",
        "The rule was removed",
    )))
}

#[utoipa::path(
    put,
    path = "/api/v1/site-rules/{id}/enabled",
    tag = "configuration",
    request_body = SiteRuleSwitchRequest,
    params(("id" = String, Path, description = "The rule's own identifier")),
    responses((status = 200, body = MessageResponse))
)]
pub async fn set_site_rule_enabled(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<SiteRuleSwitchRequest>,
) -> Result<Json<MessageResponse>, ApiError> {
    // A rule keeps its switch where RD-110-04 put it, in its own row.
    let stored = user_rule(&state, &id).await?;
    state
        .database
        .upsert_site_rule(NewUserSiteRule {
            id: stored.id,
            name: stored.name,
            group: stored.group,
            enabled: request.enabled,
            rule: stored.rule,
            origin: stored.origin,
        })
        .await?;
    reload(&state).await;
    Ok(Json(MessageResponse::new(
        "site_rules.switched",
        "The rule was switched",
    )))
}

#[utoipa::path(
    put,
    path = "/api/v1/site-rule-groups/{group}/enabled",
    tag = "configuration",
    request_body = SiteRuleSwitchRequest,
    params(("group" = String, Path, description = "The group name, as the rules carry it")),
    responses((status = 200, body = MessageResponse))
)]
pub async fn set_site_rule_group_enabled(
    State(state): State<AppState>,
    Path(group): Path<String>,
    Json(request): Json<SiteRuleSwitchRequest>,
) -> Result<Json<MessageResponse>, ApiError> {
    let known = state
        .database
        .list_site_rules()
        .await?
        .iter()
        .any(|stored| stored.group == group);
    if !known {
        return Err(ApiError::not_found(
            "site_rules.group_not_found",
            "No rule carries this group",
        )
        .with_param("group", group));
    }
    state
        .database
        .set_site_rule_switch(rd_db::SCOPE_GROUP, &group, request.enabled)
        .await?;
    reload(&state).await;
    Ok(Json(MessageResponse::new(
        "site_rules.switched",
        "The group was switched",
    )))
}

#[utoipa::path(
    post,
    path = "/api/v1/site-rules/test",
    tag = "configuration",
    request_body = TestSiteRuleRequest,
    responses((status = 200, body = TestSiteRuleResponse))
)]
pub async fn test_site_rule(
    State(state): State<AppState>,
    Json(request): Json<TestSiteRuleRequest>,
) -> Result<Json<TestSiteRuleResponse>, ApiError> {
    let rule = parse_rule(&request.rule)?;
    let address = Url::parse(request.address.trim())
        .ok()
        .filter(|url| matches!(url.scheme(), "http" | "https") && url.host_str().is_some())
        .ok_or_else(|| {
            ApiError::bad_request(
                "site_rules.invalid_address",
                "That is not an http or https address",
            )
            .with_param("address", request.address.clone())
        })?;
    let runner = trial_runner(&state);
    let crawl = match rd_plugin_ext::RuleRunner::run(runner.as_ref(), &rule, &address).await {
        Ok(crawl) => crawl,
        Err(error) => {
            return Ok(Json(TestSiteRuleResponse {
                address: address.to_string(),
                package_name: None,
                pages_fetched: 0,
                mirrors: false,
                groups: Vec::new(),
                entries: Vec::new(),
                links: Vec::new(),
                kept: 0,
                refused: 0,
                error: Some(error.code().to_owned()),
            }));
        }
    };
    // Exactly the judgement the intake applies to a crawled address (RD-110-07), asked here
    // rather than rebuilt, so the trial run cannot disagree with what a real paste would do.
    let media = state.media_settings.read().await.clone();
    let gallery = state.gallery_settings.read().await.clone();
    let deadline = std::time::Instant::now() + crate::collector_crawl_verdict::PROBE_BUDGET;
    // A rule's find keeps to the public internet, as in a real paste (RD-1190-18); the trial
    // probed it without the rule (RD-1190-22).
    let internet = state.scheduler.remote_address_policy(false);
    let mut links = Vec::with_capacity(crawl.links.len());
    for link in &crawl.links {
        let Ok(url) = Url::parse(link) else {
            links.push(TestedLinkResponse {
                url: link.clone(),
                verdict: "not-a-file".to_owned(),
                code: Some("collector.crawl_not_a_file".to_owned()),
            });
            continue;
        };
        let verdict = crate::collector_crawl_verdict::verdict(
            &state,
            &url,
            &media,
            &gallery,
            deadline,
            Some(&internet),
        )
        .await;
        links.push(TestedLinkResponse {
            url: link.clone(),
            verdict: verdict.as_str().to_owned(),
            code: verdict.code().map(str::to_owned),
        });
    }
    let refused = links.iter().filter(|link| link.code.is_some()).count();
    Ok(Json(TestSiteRuleResponse {
        address: crawl.address.to_string(),
        package_name: crawl.package_name.clone(),
        pages_fetched: crawl.pages_fetched,
        mirrors: crawl.mirrors,
        groups: crawl.groups.iter().map(group_response).collect(),
        entries: crawl
            .pick
            .iter()
            .flat_map(|list| &list.entries)
            .map(|entry| TestedEntryResponse {
                label: entry.label.clone(),
                attributes: entry.attributes.clone(),
            })
            .collect(),
        kept: links.len() - refused,
        refused,
        links,
        error: None,
    }))
}

/// The runner a trial run uses: the one the crawler selection runs every rule through, so a
/// trial and a paste cannot disagree about one page (RD-1170-02), and only without a
/// selection one of its own, built the same way `serve` builds it.
///
/// The broker is handed over on purpose, unlike in `rdownloader doctor site-rules`: this run
/// is one somebody asked for, in front of the interface that can show them the challenge, so
/// a captcha step is answered rather than reported as blocked.
fn trial_runner(state: &AppState) -> std::sync::Arc<dyn rd_plugin_ext::RuleRunner> {
    if let Some(rules) = state.crawlers.rules() {
        return rules.runner();
    }
    std::sync::Arc::new(
        rd_plugin_ext::HostRuleRunner::new(rd_plugin_host::RuleNetwork::new(
            state.database.clone(),
            state.secrets.clone(),
            state.scheduler.network_defaults(),
        ))
        .with_captcha(std::sync::Arc::new(state.scheduler.captcha())),
    )
}

/// One group of a trial run, as the editor shows it.
fn group_response(group: &rd_siterules::CrawlGroup) -> TestedGroupResponse {
    TestedGroupResponse {
        name: group.name.clone(),
        links: group
            .links
            .iter()
            .map(|link| TestedGroupLinkResponse {
                url: link.url.clone(),
                mirror: link.mirror,
            })
            .collect(),
    }
}

/// Writes one of the person's own rules with where it came from and puts the new catalogue
/// into force.
async fn store(
    state: &AppState,
    rule: &Rule,
    enabled: bool,
    origin: SiteRuleOriginKind,
) -> Result<(), ApiError> {
    persist(state, rule, enabled, origin).await?;
    reload(state).await;
    Ok(())
}

/// [`store`] without putting the catalogue into force, for a caller that writes several rules
/// and reloads once.
async fn persist(
    state: &AppState,
    rule: &Rule,
    enabled: bool,
    origin: SiteRuleOriginKind,
) -> Result<(), ApiError> {
    state
        .database
        .upsert_site_rule(NewUserSiteRule {
            id: rule.id.clone(),
            name: rule.name.clone(),
            group: rule.group.clone(),
            enabled,
            rule: serde_json::to_value(rule).map_err(|error| {
                ApiError::bad_request("site_rules.invalid_rule", error.to_string())
            })?,
            origin,
        })
        .await?;
    Ok(())
}

/// The stored rule, or a refusal naming the id.
async fn user_rule(state: &AppState, id: &str) -> Result<rd_db::UserSiteRule, ApiError> {
    state
        .database
        .list_site_rules()
        .await?
        .into_iter()
        .find(|stored| stored.id == id)
        .ok_or_else(|| not_found(id))
}

fn not_found(id: &str) -> ApiError {
    ApiError::not_found("site_rules.not_found", "No rule with that id").with_param("rule", id)
}
