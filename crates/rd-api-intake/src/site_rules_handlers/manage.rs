//! Deleting every site rule, and restoring the example list (RD-1230-03).

use super::*;

/// Deletes every stored rule with its self-test result; the group switches stay.
///
/// The confirmation is a value as well as the dialog's question, as with every clear
/// (`data_reset_handlers`): a client that never drew the question still has to say it. The act
/// goes into the audit log with the number of rules it removed.
#[utoipa::path(
    post,
    path = "/api/v1/site-rules/clear",
    tag = "configuration",
    request_body = SiteRulesClearRequest,
    responses(
        (status = 200, body = SiteRulesClearResponse),
        (status = 400, description = "site_rules.not_confirmed"),
    )
)]
pub async fn clear_site_rules(
    State(state): State<AppState>,
    audit: crate::audit::AuditContext,
    Json(request): Json<SiteRulesClearRequest>,
) -> Result<Json<SiteRulesClearResponse>, ApiError> {
    if !request.confirmed {
        return Err(ApiError::bad_request(
            "site_rules.not_confirmed",
            "Deleting every site rule must be confirmed",
        ));
    }
    let removed = state.database.delete_all_site_rules().await?;
    crate::audit::record(
        &state,
        crate::audit::AuditEvent::success(rd_core::AuditAction::SiteRulesCleared)
            .by(&audit)
            .target("site_rules", "site_rules")
            .detail(rd_db::CLEARED_DETAIL_KEY, removed),
    )
    .await;
    reload(&state).await;
    Ok(Json(SiteRulesClearResponse { removed }))
}

/// Writes the examples the app brings again, switched off, for every one whose id no stored
/// rule carries; an example somebody kept and changed stays as it is.
#[utoipa::path(
    post,
    path = "/api/v1/site-rules/examples",
    tag = "configuration",
    responses((status = 200, body = SiteRuleExamplesResponse))
)]
pub async fn restore_site_rule_examples(
    State(state): State<AppState>,
) -> Result<Json<SiteRuleExamplesResponse>, ApiError> {
    let restored = site_rules_service::add_missing_examples(&state.database).await?;
    if restored > 0 {
        reload(&state).await;
    }
    Ok(Json(SiteRuleExamplesResponse { restored }))
}
