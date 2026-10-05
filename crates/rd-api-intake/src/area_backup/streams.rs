//! Export and import of the stream channels and their schedules.

use super::*;

#[utoipa::path(get, path = "/api/v1/streams/export", tag = "streams", responses((status = 200, body = AreaBundle)))]
pub async fn export_streams(State(state): State<AppState>) -> Result<Json<AreaBundle>, ApiError> {
    let categories = state.database.list_categories().await?;
    let channels = state.database.list_stream_channels().await?;
    let schedules = state.database.list_stream_schedules().await?;
    let category_name = |id: rd_core::CategoryId| {
        categories
            .iter()
            .find(|category| category.id == id)
            .map(|category| category.name.clone())
    };
    let channel_name = |id: rd_core::StreamChannelId| {
        channels
            .iter()
            .find(|channel| channel.id == id)
            .map(|channel| channel.name.clone())
    };
    let bundled_schedules = schedules
        .iter()
        .filter_map(|schedule| {
            Some(BundleAreaStreamSchedule {
                channel_name: channel_name(schedule.channel_id)?,
                name: schedule.name.clone(),
                enabled: schedule.enabled,
                kind: schedule.kind.clone(),
                timezone: schedule.timezone.clone(),
                window_minutes: schedule.window_minutes,
                lead_minutes: schedule.lead_minutes,
                trail_minutes: schedule.trail_minutes,
                replay_from_start: schedule.replay_from_start,
            })
        })
        .collect();
    let bundled_channels = channels
        .into_iter()
        .map(|channel| BundleAreaStreamChannel {
            url: channel.url,
            name: channel.name,
            quality: channel.quality,
            category_name: channel.category_id.and_then(category_name),
            enabled: channel.enabled,
            recording: channel.recording,
        })
        .collect();
    Ok(Json(AreaBundle {
        stream_channels: Some(bundled_channels),
        stream_schedules: Some(bundled_schedules),
        ..AreaBundle::empty()
    }))
}

#[utoipa::path(post, path = "/api/v1/streams/import", tag = "streams", request_body = AreaBundle, responses((status = 200, body = ImportAreaSummary), (status = 400, body = crate::error::ErrorBody)))]
pub async fn import_streams(
    State(state): State<AppState>,
    Json(bundle): Json<AreaBundle>,
) -> Result<Json<ImportAreaSummary>, ApiError> {
    validate_header(&bundle)?;
    if bundle.stream_channels.is_none() && bundle.stream_schedules.is_none() {
        return Err(section_missing("streams"));
    }
    let channel_entries = bundle.stream_channels.unwrap_or_default();
    let schedule_entries = bundle.stream_schedules.unwrap_or_default();
    crate::error_codes::validate_bundle_section(channel_entries.len())?;
    crate::error_codes::validate_bundle_section(schedule_entries.len())?;
    let categories = state.database.list_categories().await?;
    // Keyed by name because both uses are lookups by name: the duplicate check here, and the
    // channel a schedule attaches to below. As a list both were linear scans inside a loop.
    let mut channels: HashMap<String, rd_core::StreamChannel> = state
        .database
        .list_stream_channels()
        .await?
        .into_iter()
        .map(|channel| (channel.name.clone(), channel))
        .collect();
    let mut summary = ImportAreaSummary::default();

    for entry in channel_entries {
        if channels.contains_key(&entry.name) {
            summary.skipped += 1;
            continue;
        }
        let category_id = entry.category_name.as_ref().and_then(|name| {
            categories
                .iter()
                .find(|category| &category.name == name)
                .map(|category| category.id)
        });
        let request = crate::dto::StreamChannelRequest {
            url: entry.url,
            name: Some(entry.name),
            quality: entry.quality,
            category_id,
            enabled: entry.enabled,
            recording: entry.recording,
        };
        let Ok(input) = crate::stream_handlers::channel_input(request) else {
            summary.skipped += 1;
            continue;
        };
        let created = state.database.create_stream_channel(input).await?;
        channels.insert(created.name.clone(), created);
        summary.created += 1;
    }

    // After the channels, so a schedule can attach to one this same import just created.
    let existing_schedules: HashSet<(String, rd_core::StreamChannelId)> = state
        .database
        .list_stream_schedules()
        .await?
        .into_iter()
        .map(|schedule| (schedule.name, schedule.channel_id))
        .collect();
    for entry in schedule_entries {
        let Some(channel) = channels.get(&entry.channel_name) else {
            summary.skipped += 1;
            continue;
        };
        if existing_schedules.contains(&(entry.name.clone(), channel.id)) {
            summary.skipped += 1;
            continue;
        }
        let request = crate::stream_schedule_handlers::StreamScheduleRequest {
            channel_id: channel.id,
            name: entry.name,
            enabled: entry.enabled,
            kind: entry.kind,
            timezone: entry.timezone,
            window_minutes: entry.window_minutes,
            lead_minutes: entry.lead_minutes,
            trail_minutes: entry.trail_minutes,
            replay_from_start: entry.replay_from_start,
        };
        let Ok(input) = crate::stream_schedule_handlers::schedule_input(request) else {
            summary.skipped += 1;
            continue;
        };
        state.database.create_stream_schedule(input).await?;
        summary.created += 1;
    }
    Ok(Json(summary))
}
