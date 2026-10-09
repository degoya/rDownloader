//! Creating and replacing one of the person's own rules, from the editor or an MCP tool, each
//! recorded as the rule's origin (RD-1200-05).

use super::*;

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
    create_site_rule_from(&state, &request, SiteRuleOriginKind::Editor).await
}

/// [`create_site_rule`] for a caller that is not the editor; `origin` is what the rule records.
pub async fn create_site_rule_from(
    state: &AppState,
    request: &SaveSiteRuleRequest,
    origin: SiteRuleOriginKind,
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
    store(state, &rule, request.enabled, origin).await?;
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
    update_site_rule_from(&state, &id, &request, SiteRuleOriginKind::Editor).await
}

/// [`update_site_rule`] for a caller that is not the editor. A changed body takes the new
/// origin: whoever wrote the change is where the body now comes from.
pub async fn update_site_rule_from(
    state: &AppState,
    id: &str,
    request: &SaveSiteRuleRequest,
    origin: SiteRuleOriginKind,
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
    let stored = user_rule(state, id).await?;
    // Saving a body unchanged (the editor's switch, say) keeps where it came from.
    let origin = if serde_json::to_value(&rule).ok().as_ref() == Some(&stored.rule) {
        stored.origin
    } else {
        origin
    };
    store(state, &rule, request.enabled, origin).await?;
    Ok(Json(MessageResponse::new(
        "site_rules.saved",
        "The rule was saved",
    )))
}
