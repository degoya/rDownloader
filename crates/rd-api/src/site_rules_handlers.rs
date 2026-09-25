//! The site-rule settings page over REST (RD-110-08).
//!
//! Four things happen here that are worth stating before the code says them.
//!
//! **Every rule is the person's own** (RD-130-07). Nothing arrives with the binary any more;
//! the project's rules are a signed release file, and importing it stores them here like any
//! other rule, so each can be edited, duplicated, switched and removed.
//!
//! **Every body is parsed and validated before it is stored**, through `rd_siterules::Rule`
//! and `Rule::validate`, which is the same gate a rule from the pack passes. A rule from a
//! file somebody was sent is untrusted input in the strict sense: it names hosts to fetch and
//! patterns to run. It gets no shortcut.
//!
//! **An imported rule is stored switched off, always** -- the signed release file's included.
//! The import endpoint does not read an `enabled` field and does not offer one; switching a
//! rule on is a separate request per rule, which is the confirmation the job asks for -- and it
//! is enforced here rather than in the interface, so a client that never drew the dialog still
//! activates nothing. A signature proves who wrote a rule, not that this person wants it.
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
use rd_siterules::Rule;
use url::Url;

use crate::{
    AppState,
    dto::MessageResponse,
    error::ApiError,
    site_rules_dto::{
        ImportSiteRulesResponse, ImportedSiteRuleResponse, SaveSiteRuleRequest,
        SiteRuleCheckResponse, SiteRuleDocument, SiteRuleGroupResponse, SiteRuleImportRequest,
        SiteRuleResponse, SiteRuleSwitchRequest, SiteRulesResponse, TestSiteRuleRequest,
        TestSiteRuleResponse, TestedLinkResponse,
    },
    site_rules_service,
};

/// The document version the export writes and the import reads.
const DOCUMENT_VERSION: u32 = rd_siterules::FORMAT_VERSION;

/// Reads a rule body and refuses one that could not work, with the code the interface
/// translates. The same two steps the pack's own loader performs, in the same order.
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

/// One row built from a parsed rule.
fn row(
    rule: &Rule,
    enabled: bool,
    active: bool,
    checks: &BTreeMap<String, rd_db::SiteRuleCheck>,
) -> SiteRuleResponse {
    SiteRuleResponse {
        id: rule.id.clone(),
        name: rule.name.clone(),
        group: rule.group.clone(),
        hosts: rule.matches.hosts.clone(),
        version: rule.version,
        probe: rule.probe.clone(),
        mirrors: rule.mirrors,
        steps: rule.steps.len(),
        enabled,
        active,
        rule: serde_json::to_value(rule).unwrap_or(serde_json::Value::Null),
        check: checks.get(&rule.id).map(check_response),
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
            Ok(rule) => rules.push(row(&rule, stored.enabled, active, &checks)),
            Err(_) => rules.push(SiteRuleResponse {
                id: stored.id.clone(),
                name: stored.name.clone(),
                group: stored.group.clone(),
                hosts: Vec::new(),
                version: 0,
                probe: String::new(),
                mirrors: false,
                steps: 0,
                enabled: stored.enabled,
                active: false,
                rule: stored.rule,
                check: checks.get(&stored.id).map(check_response),
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
    post,
    path = "/api/v1/site-rules",
    tag = "configuration",
    request_body = SaveSiteRuleRequest,
    responses((status = 200, body = MessageResponse))
)]
pub async fn create_site_rule(
    State(state): State<AppState>,
    Json(request): Json<SaveSiteRuleRequest>,
) -> Result<Json<MessageResponse>, ApiError> {
    let rule = parse_rule(&request.rule)?;
    if state
        .database
        .list_site_rules()
        .await?
        .iter()
        .any(|stored| stored.id == rule.id)
    {
        return Err(ApiError::conflict(
            "site_rules.duplicate_id",
            "A rule with this id already exists",
        )
        .with_param("rule", rule.id));
    }
    store(&state, &rule, request.enabled).await?;
    Ok(Json(MessageResponse::new(
        "site_rules.saved",
        "The rule was saved",
    )))
}

#[utoipa::path(
    put,
    path = "/api/v1/site-rules/{id}",
    tag = "configuration",
    request_body = SaveSiteRuleRequest,
    params(("id" = String, Path, description = "The rule's own identifier")),
    responses((status = 200, body = MessageResponse))
)]
pub async fn update_site_rule(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<SaveSiteRuleRequest>,
) -> Result<Json<MessageResponse>, ApiError> {
    let rule = parse_rule(&request.rule)?;
    if rule.id != id {
        // Renaming a rule is a delete and a create, because the id is what the switch, the
        // self-test row and every reference to it are keyed by.
        return Err(ApiError::bad_request(
            "site_rules.id_mismatch",
            "The rule's id does not match the address it was sent to",
        )
        .with_param("rule", rule.id));
    }
    user_rule(&state, &id).await?;
    store(&state, &rule, request.enabled).await?;
    Ok(Json(MessageResponse::new(
        "site_rules.saved",
        "The rule was saved",
    )))
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
    // The broker is handed over on purpose, unlike in `rdownloader doctor site-rules`: this
    // run is one somebody asked for, in front of the interface that can show them the
    // challenge, so a captcha step is answered rather than reported as blocked.
    let runner = rd_plugin_ext::HostRuleRunner::new(rd_plugin_host::RuleNetwork::new(
        state.database.clone(),
        state.secrets.clone(),
        state.scheduler.network_defaults(),
    ))
    .with_captcha(std::sync::Arc::new(state.scheduler.captcha()));
    let crawl = match rd_plugin_ext::RuleRunner::run(&runner, &rule, &address).await {
        Ok(crawl) => crawl,
        Err(error) => {
            return Ok(Json(TestSiteRuleResponse {
                address: address.to_string(),
                package_name: None,
                pages_fetched: 0,
                mirrors: false,
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
        let verdict =
            crate::collector_crawl_verdict::verdict(&state, &url, &media, &gallery, deadline).await;
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
        kept: links.len() - refused,
        refused,
        links,
        error: None,
    }))
}

#[utoipa::path(
    get,
    path = "/api/v1/site-rules/export",
    tag = "configuration",
    responses((status = 200, body = SiteRuleDocument))
)]
pub async fn export_site_rules(
    State(state): State<AppState>,
) -> Result<Json<SiteRuleDocument>, ApiError> {
    Ok(Json(SiteRuleDocument {
        format_version: DOCUMENT_VERSION,
        rules: state
            .database
            .list_site_rules()
            .await?
            .into_iter()
            .map(|stored| stored.rule)
            .collect(),
    }))
}

#[utoipa::path(
    post,
    path = "/api/v1/site-rules/import",
    tag = "configuration",
    request_body(content = SiteRuleImportRequest, content_type = "application/json"),
    responses((status = 200, body = ImportSiteRulesResponse))
)]
pub async fn import_site_rules(
    State(state): State<AppState>,
    body: Bytes,
) -> Result<Json<ImportSiteRulesResponse>, ApiError> {
    let (bodies, signed) = import_bodies(&body)?;
    let mut existing: Vec<String> = state
        .database
        .list_site_rules()
        .await?
        .into_iter()
        .map(|stored| stored.id)
        .collect();
    let mut results = Vec::with_capacity(bodies.len());
    let mut stored_count = 0usize;
    for body in &bodies {
        let refused = |id: String, name: String, code: &str| ImportedSiteRuleResponse {
            id,
            name,
            status: "refused".to_owned(),
            code: Some(code.to_owned()),
        };
        let rule = match parse_rule(body) {
            Ok(rule) => rule,
            Err(error) => {
                let id = body
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                let name = body
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                results.push(refused(id, name, error.code()));
                continue;
            }
        };
        // A second import of the release file meets the rules the first one stored, perhaps
        // edited since: the stored rule stays and the file's is reported, never written over it.
        if existing.iter().any(|id| id == &rule.id) {
            results.push(refused(
                rule.id.clone(),
                rule.name.clone(),
                "site_rules.duplicate_id",
            ));
            continue;
        }
        // Switched off, without asking and without an option to ask otherwise: nothing a file
        // brings starts active here, signed or not.
        store(&state, &rule, false).await?;
        existing.push(rule.id.clone());
        stored_count += 1;
        results.push(ImportedSiteRuleResponse {
            id: rule.id.clone(),
            name: rule.name.clone(),
            status: "stored".to_owned(),
            code: None,
        });
    }
    Ok(Json(ImportSiteRulesResponse {
        rules: results,
        stored: stored_count,
        signed,
    }))
}

/// The rule bodies an imported file carries, and whether they came under a signature that
/// held (RD-130-07).
///
/// A file with `signatures` is the signed release file and is verified as one, against the
/// compiled-in site-rules root and from the bytes exactly as they arrived -- the signature
/// covers those bytes, not a parsed copy of them. One that does not verify is refused whole,
/// with the pack's own code, and is never read a second time as an unsigned export: a release
/// file that fails its signature is a damaged or altered one, not somebody's own rules.
fn import_bodies(bytes: &[u8]) -> Result<(Vec<serde_json::Value>, bool), ApiError> {
    let value: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|error| ApiError::bad_request("site_rules.malformed", error.to_string()))?;
    if value.get("signatures").is_some() {
        let pack = rd_siterules::verify(bytes, None, chrono::Utc::now())
            .map_err(|error| ApiError::bad_request(error.code(), error.to_string()))?;
        let bodies = pack
            .rules
            .iter()
            .map(serde_json::to_value)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| ApiError::bad_request("site_rules.invalid_rule", error.to_string()))?;
        return Ok((bodies, true));
    }
    let document: SiteRuleDocument = serde_json::from_value(value)
        .map_err(|error| ApiError::bad_request("site_rules.malformed", error.to_string()))?;
    if document.format_version != DOCUMENT_VERSION {
        return Err(ApiError::bad_request(
            "site_rules.format_version_unsupported",
            "This build does not read that rule format",
        )
        .with_param("format_version", document.format_version));
    }
    Ok((document.rules, false))
}

/// Writes one of the person's own rules and puts the new catalogue into force.
async fn store(state: &AppState, rule: &Rule, enabled: bool) -> Result<(), ApiError> {
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
        })
        .await?;
    reload(state).await;
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
