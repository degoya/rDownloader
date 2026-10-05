//! Export and import of the automations.

use super::*;

#[utoipa::path(get, path = "/api/v1/automations/export", tag = "automations", responses((status = 200, body = AreaBundle)))]
pub async fn export_automations(
    State(state): State<AppState>,
) -> Result<Json<AreaBundle>, ApiError> {
    let categories = state.database.list_categories().await?;
    let targets = state.database.list_notification_targets().await?;
    let automations = state.database.list_automations().await?;
    let mut definitions =
        rd_api_core::automation_service::current_definitions(&state.database, &automations).await?;
    let mut entries = Vec::new();
    for automation in automations {
        let Some(definition) = definitions.remove(&automation.id) else {
            continue;
        };
        let actions = definition
            .actions
            .iter()
            .filter_map(|action| match action {
                rd_automation::Action::Webhook { target_id } => targets
                    .iter()
                    .find(|target| target.id == *target_id)
                    .map(|target| BundleAreaAction::Webhook {
                        target_name: target.name.clone(),
                    }),
                rd_automation::Action::Script { name } => {
                    Some(BundleAreaAction::Script { name: name.clone() })
                }
                rd_automation::Action::SetCategory { category_id } => categories
                    .iter()
                    .find(|category| category.id == *category_id)
                    .map(|category| BundleAreaAction::SetCategory {
                        category_name: category.name.clone(),
                    }),
                rd_automation::Action::PausePackage => Some(BundleAreaAction::PausePackage),
                rd_automation::Action::ResumePackage => Some(BundleAreaAction::ResumePackage),
            })
            .collect();
        entries.push(BundleAreaAutomation {
            name: automation.name,
            enabled: automation.enabled,
            trigger: definition.trigger,
            condition: definition.condition,
            actions,
        });
    }
    Ok(Json(AreaBundle {
        automations: Some(entries),
        ..AreaBundle::empty()
    }))
}

#[utoipa::path(post, path = "/api/v1/automations/import", tag = "automations", request_body = AreaBundle, responses((status = 200, body = ImportAreaSummary), (status = 400, body = crate::error::ErrorBody)))]
pub async fn import_automations(
    State(state): State<AppState>,
    Json(bundle): Json<AreaBundle>,
) -> Result<Json<ImportAreaSummary>, ApiError> {
    validate_header(&bundle)?;
    let entries = bundle
        .automations
        .ok_or_else(|| section_missing("automations"))?;
    crate::error_codes::validate_bundle_section(entries.len())?;
    let categories = state.database.list_categories().await?;
    let targets = state.database.list_notification_targets().await?;
    // Same reason as the subscriptions import: one scan per entry against every stored name.
    let mut existing: HashSet<String> = state
        .database
        .list_automations()
        .await?
        .into_iter()
        .map(|automation| automation.name)
        .collect();
    let mut summary = ImportAreaSummary::default();
    for entry in entries {
        if existing.contains(&entry.name) {
            summary.skipped += 1;
            continue;
        }
        // An action pointing at something this instance does not have cannot be carried over,
        // and an automation missing one of its actions is not the automation somebody exported.
        // Counted as skipped rather than imported half-done.
        let mut actions = Vec::with_capacity(entry.actions.len());
        let mut unresolved = false;
        for action in &entry.actions {
            let resolved = match action {
                BundleAreaAction::Webhook { target_name } => targets
                    .iter()
                    .find(|target| &target.name == target_name)
                    .map(|target| rd_automation::Action::Webhook {
                        target_id: target.id,
                    }),
                BundleAreaAction::Script { name } => {
                    Some(rd_automation::Action::Script { name: name.clone() })
                }
                BundleAreaAction::SetCategory { category_name } => categories
                    .iter()
                    .find(|category| &category.name == category_name)
                    .map(|category| rd_automation::Action::SetCategory {
                        category_id: category.id,
                    }),
                BundleAreaAction::PausePackage => Some(rd_automation::Action::PausePackage),
                BundleAreaAction::ResumePackage => Some(rd_automation::Action::ResumePackage),
            };
            match resolved {
                Some(action) => actions.push(action),
                None => {
                    unresolved = true;
                    break;
                }
            }
        }
        if unresolved {
            summary.skipped += 1;
            continue;
        }
        let request = crate::automation_input::AutomationRequest {
            name: entry.name.clone(),
            enabled: entry.enabled,
            trigger: entry.trigger,
            condition: entry.condition,
            actions,
        };
        let Ok(input) = crate::automation_input::validated(request) else {
            summary.skipped += 1;
            continue;
        };
        state.database.upsert_automation(None, input).await?;
        existing.insert(entry.name);
        summary.created += 1;
    }
    Ok(Json(summary))
}
