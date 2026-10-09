//! Pausing the whole queue for a while (RD-190-20): "pause for 30 minutes", "until 18:00".
//!
//! The pause itself is the scheduler's (`rd_scheduler::QueuePause`): the files it stops, the
//! hold that keeps new ones back, the end that survives a restart. These routes set it, read it
//! and end it early. Reading it also names the accounts whose traffic is used up (RD-1190-14),
//! the other thing that holds downloads back without anybody pressing pause.

use axum::{Json, extract::State};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{AppState, error::ApiError};

/// The longest pause one request may set; anything open-ended is the plain "pause all".
const MAX_MINUTES: u32 = 30 * 24 * 60;

#[derive(Deserialize, ToSchema)]
pub struct QueuePauseRequest {
    /// How long, in minutes from now; give this or `until`.
    #[serde(default)]
    pub minutes: Option<u32>,
    /// When the pause ends; give this or `minutes`.
    #[serde(default)]
    pub until: Option<DateTime<Utc>>,
}

#[derive(Serialize, ToSchema)]
pub struct QueuePauseResponse {
    /// Whether a pause is in force.
    pub paused: bool,
    /// When it ends and the queue runs again; `null` while paused for a pause that lasts until
    /// it is resumed — the one the stop mark sets (RD-1210-02).
    pub until: Option<DateTime<Utc>>,
    /// The files it stopped; its end resumes those still paused.
    pub files: u32,
    /// The accounts whose traffic their hoster reports used up, the soonest to end first
    /// (RD-1190-14); empty while none is.
    pub account_traffic: Vec<AccountTrafficHoldResponse>,
    /// Where the queue will pause next: the stop mark in force (RD-1210-02), `null` while none
    /// is set.
    pub stop_mark: Option<crate::stop_mark_handlers::QueueStopMarkResponse>,
}

/// An account whose traffic is used up, and what that holds back.
#[derive(Serialize, ToSchema)]
pub struct AccountTrafficHoldResponse {
    pub account_id: rd_core::AccountId,
    /// The account's own name, as the account list shows it.
    pub account_label: String,
    /// The provider slug, such as `ddownload`.
    pub provider: String,
    /// What the setting makes of it: `nothing`, `pause_account` or `pause_queue`.
    pub action: rd_core::AccountTrafficAction,
    /// When the hoster's wait ends and the waiting files try again.
    pub until: DateTime<Utc>,
    /// When the account is next checked for traffic; traffic above zero continues at once.
    pub next_check_at: DateTime<Utc>,
}

#[derive(Serialize, ToSchema)]
pub struct QueueResumeResponse {
    /// Files the ended pause queued again.
    pub resumed: u32,
}

/// The pause in force, if any, and the stop mark that will set one.
#[utoipa::path(get, path = "/api/v1/queue/pause", tag = "downloads", responses((status = 200, body = QueuePauseResponse)))]
pub async fn get_queue_pause(State(state): State<AppState>) -> Json<QueuePauseResponse> {
    Json(response(&state, state.scheduler.queue_pause().await).await)
}

/// Pauses every waiting and running file until the end, and holds back new ones until then.
/// A pause already in force moves to the new end and keeps the files it holds.
#[utoipa::path(put, path = "/api/v1/queue/pause", tag = "downloads", request_body = QueuePauseRequest, responses((status = 200, body = QueuePauseResponse), (status = 400, body = crate::error::ErrorBody)))]
pub async fn pause_queue(
    State(state): State<AppState>,
    Json(request): Json<QueuePauseRequest>,
) -> Result<Json<QueuePauseResponse>, ApiError> {
    let until = pause_end(&request, Utc::now())?;
    let pause = state.scheduler.pause_queue_until(until).await?;
    Ok(Json(response(&state, Some(pause)).await))
}

/// Ends the timed pause now: the files it stopped are queued again, and the hold goes. The
/// accounts held for their used-up traffic are let go too (RD-1190-14): a start by hand
/// outranks the automatic hold, and their waiting files keep their own due times.
#[utoipa::path(delete, path = "/api/v1/queue/pause", tag = "downloads", responses((status = 200, body = QueueResumeResponse)))]
pub async fn resume_queue(
    State(state): State<AppState>,
) -> Result<Json<QueueResumeResponse>, ApiError> {
    state.scheduler.release_account_traffic().await;
    let resumed = state.scheduler.resume_queue().await?;
    Ok(Json(QueueResumeResponse {
        resumed: u32::try_from(resumed).unwrap_or(u32::MAX),
    }))
}

async fn response(state: &AppState, pause: Option<rd_scheduler::QueuePause>) -> QueuePauseResponse {
    QueuePauseResponse {
        paused: pause.is_some(),
        until: pause.as_ref().and_then(|pause| pause.until),
        files: pause.map_or(0, |pause| {
            u32::try_from(pause.files.len()).unwrap_or(u32::MAX)
        }),
        account_traffic: account_traffic(state).await,
        stop_mark: stop_mark(state).await,
    }
}

/// The stop mark, named; a read that fails names none and says so, like the accounts below.
async fn stop_mark(state: &AppState) -> Option<crate::stop_mark_handlers::QueueStopMarkResponse> {
    crate::stop_mark_handlers::current(state)
        .await
        .unwrap_or_else(|error| {
            tracing::warn!(?error, "the stop mark could not be read");
            None
        })
}

/// The accounts held for their traffic, named. One the database cannot name — deleted a moment
/// ago — is left out rather than shown nameless; a read that fails names none and says so.
async fn account_traffic(state: &AppState) -> Vec<AccountTrafficHoldResponse> {
    let holds = state.scheduler.account_traffic();
    if holds.is_empty() {
        return Vec::new();
    }
    let accounts = match state.database.list_accounts().await {
        Ok(accounts) => accounts,
        Err(error) => {
            tracing::warn!(%error, "the accounts waiting for traffic could not be named");
            return Vec::new();
        }
    };
    holds
        .into_iter()
        .filter_map(|hold| {
            let account = accounts
                .iter()
                .find(|account| account.id == hold.account_id)?;
            Some(AccountTrafficHoldResponse {
                account_id: hold.account_id,
                account_label: account.label.clone(),
                provider: account.provider.clone(),
                action: hold.action,
                until: hold.until,
                next_check_at: hold.next_check_at,
            })
        })
        .collect()
}

/// Exactly one of `minutes` and `until`, ending at least a minute and at most thirty days ahead.
pub(crate) fn pause_end(
    request: &QueuePauseRequest,
    now: DateTime<Utc>,
) -> Result<DateTime<Utc>, ApiError> {
    let until = match (request.minutes, request.until) {
        (Some(minutes), None) if (1..=MAX_MINUTES).contains(&minutes) => {
            Some(now + Duration::minutes(i64::from(minutes)))
        }
        (None, Some(until))
            if until > now && until <= now + Duration::minutes(i64::from(MAX_MINUTES)) =>
        {
            Some(until)
        }
        _ => None,
    };
    until.ok_or_else(|| {
        ApiError::bad_request(
            "queue.pause_end_invalid",
            "Give the pause either minutes or an end time, within 30 days",
        )
        .with_param("max_minutes", MAX_MINUTES)
    })
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, Utc};

    use super::{MAX_MINUTES, QueuePauseRequest, pause_end};

    fn request(minutes: Option<u32>, until: Option<chrono::DateTime<Utc>>) -> QueuePauseRequest {
        QueuePauseRequest { minutes, until }
    }

    #[test]
    fn minutes_count_from_now() {
        let now = Utc::now();
        let end = pause_end(&request(Some(30), None), now).expect("end");
        assert_eq!(end, now + Duration::minutes(30));
    }

    #[test]
    fn an_end_time_is_taken_as_given() {
        let now = Utc::now();
        let until = now + Duration::hours(3);
        assert_eq!(
            pause_end(&request(None, Some(until)), now).expect("end"),
            until
        );
    }

    #[test]
    fn exactly_one_end_within_thirty_days_is_accepted() {
        let now = Utc::now();
        let soon = now + Duration::hours(1);
        for refused in [
            request(None, None),
            request(Some(30), Some(soon)),
            request(Some(0), None),
            request(Some(MAX_MINUTES + 1), None),
            request(None, Some(now - Duration::minutes(1))),
            request(None, Some(now + Duration::days(31))),
        ] {
            let error = pause_end(&refused, now).expect_err("refused");
            assert_eq!(error.code(), "queue.pause_end_invalid");
        }
    }
}
