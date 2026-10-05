//! Export and import of the subscriptions.

use super::*;

#[utoipa::path(get, path = "/api/v1/subscriptions/export", tag = "subscriptions", responses((status = 200, body = AreaBundle)))]
pub async fn export_subscriptions(
    State(state): State<AppState>,
) -> Result<Json<AreaBundle>, ApiError> {
    let categories = state.database.list_categories().await?;
    let name_of = |id: rd_core::CategoryId| {
        categories
            .iter()
            .find(|category| category.id == id)
            .map(|category| category.name.clone())
    };
    let entries = state
        .database
        .list_subscriptions()
        .await?
        .into_iter()
        // A script subscription stays behind (RD-130-19): a bundle is a file somebody sends,
        // and what it would carry is an instruction to run code on the machine that opens it.
        .filter(|subscription| subscription.kind != rd_core::SubscriptionKind::Script)
        .map(|subscription| BundleAreaSubscription {
            name: subscription.name,
            url: subscription.url.to_string(),
            kind: subscription.kind,
            enabled: subscription.enabled,
            mode: subscription.mode,
            category_name: subscription.category_id.and_then(name_of),
            priority: subscription.priority,
            interval_seconds: subscription.interval_seconds,
            filters: subscription.filters,
            backlog: subscription.backlog,
            category_map: subscription.category_map,
            source_categories: subscription.source_categories,
            every_release: subscription.every_release,
            view: subscription.view,
            autoplay: subscription.autoplay,
            card_ratio: subscription.card_ratio,
            indexer_search: subscription.indexer_search,
            git_release: subscription.git_release,
            api_key_required: subscription.secret_ref.is_some(),
        })
        .collect();
    Ok(Json(AreaBundle {
        subscriptions: Some(entries),
        ..AreaBundle::empty()
    }))
}

#[utoipa::path(post, path = "/api/v1/subscriptions/import", tag = "subscriptions", request_body = AreaBundle, responses((status = 200, body = ImportAreaSummary), (status = 400, body = crate::error::ErrorBody)))]
pub async fn import_subscriptions(
    State(state): State<AppState>,
    Json(bundle): Json<AreaBundle>,
) -> Result<Json<ImportAreaSummary>, ApiError> {
    validate_header(&bundle)?;
    let entries = bundle
        .subscriptions
        .ok_or_else(|| section_missing("subscriptions"))?;
    crate::error_codes::validate_bundle_section(entries.len())?;
    let categories = state.database.list_categories().await?;
    // A set, not a list: the scan runs once per entry, so a linear one made the whole import
    // quadratic in the number of names already stored.
    let mut existing: HashSet<String> = state
        .database
        .list_subscriptions()
        .await?
        .into_iter()
        .map(|subscription| subscription.name)
        .collect();
    let mut summary = ImportAreaSummary::default();
    for entry in entries {
        // Never created from a file, whatever it says (RD-130-19): only the administrator
        // sets up a script subscription, by hand, knowing which script it runs.
        if existing.contains(&entry.name) || entry.kind == rd_core::SubscriptionKind::Script {
            summary.skipped += 1;
            continue;
        }
        let category_id = match &entry.category_name {
            // A category that does not exist here is not a reason to drop the subscription:
            // without one it falls back to the default, which is what a fresh one does anyway.
            Some(name) => categories
                .iter()
                .find(|category| &category.name == name)
                .map(|category| category.id),
            None => None,
        };
        let request = crate::subscription_handlers::SubscriptionRequest {
            name: entry.name.clone(),
            url: entry.url,
            kind: entry.kind,
            // Switched off when the original needed a key, because this bundle has none: a
            // subscription that polls without its credential only produces failures.
            enabled: entry.enabled && !entry.api_key_required,
            mode: entry.mode,
            category_id,
            priority: entry.priority,
            interval_seconds: entry.interval_seconds,
            filters: entry.filters,
            backlog: entry.backlog,
            category_map: entry.category_map,
            source_categories: entry.source_categories,
            every_release: entry.every_release,
            view: entry.view,
            autoplay: entry.autoplay,
            card_ratio: entry.card_ratio.as_str().to_owned(),
            schedule: None,
            script_arguments: Vec::new(),
            indexer_search: entry.indexer_search,
            git_release: entry.git_release,
            indexer_id: None,
            api_key: None,
        };
        let Ok(input) = crate::subscription_handlers::subscription_input(&request, None) else {
            summary.skipped += 1;
            continue;
        };
        state.database.create_subscription(input).await?;
        existing.insert(entry.name);
        summary.created += 1;
    }
    Ok(Json(summary))
}
