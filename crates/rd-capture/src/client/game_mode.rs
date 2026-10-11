//! The game mode's hold and release (RD-1240-19, `crate::game_mode`) and the tray's switch of it
//! (RD-1240-23).

use anyhow::Result;
use chrono::{DateTime, Utc};

use super::{CaptureClient, ensure_success};
use crate::game_mode::Held;

#[derive(serde::Deserialize)]
struct HoldAnswer {
    held: bool,
    until: Option<DateTime<Utc>>,
}

#[derive(serde::Deserialize)]
struct ReleaseAnswer {
    released: bool,
}

impl CaptureClient {
    /// Sets the game mode's hold, or renews the agent's own one ending at `renews`. What it holds
    /// -- the queue or a profile -- the service reads from the agent's settings.
    ///
    /// Refused with `auth.scope_insufficient` unless the agent was paired with queue control.
    pub(crate) async fn hold_game_mode(&self, renews: Option<DateTime<Utc>>) -> Result<Held> {
        let endpoint = self.service.join("api/v1/capture/game-mode/hold")?;
        let body = serde_json::json!({ "renews": renews });
        let response = self.send(self.http.post(endpoint).json(&body)).await?;
        let response = ensure_success(response, "game mode hold").await?;
        let answer: HoldAnswer = response.json().await?;
        Ok(match (answer.held, answer.until) {
            (true, Some(until)) => Held::Until(until),
            _ => Held::Refused,
        })
    }

    /// Lifts the agent's own hold ending at `until`; `false` when nothing of its own held.
    pub(crate) async fn release_game_mode(&self, until: DateTime<Utc>) -> Result<bool> {
        let endpoint = self.service.join("api/v1/capture/game-mode/release")?;
        let body = serde_json::json!({ "until": until });
        let response = self.send(self.http.post(endpoint).json(&body)).await?;
        let response = ensure_success(response, "game mode release").await?;
        let answer: ReleaseAnswer = response.json().await?;
        Ok(answer.released)
    }

    /// Switches game mode on or off at the service, which keeps its triggers and keeps the switch
    /// for every start. Refused with `auth.scope_insufficient` like the hold.
    pub(crate) async fn set_game_mode_enabled(
        &self,
        enabled: bool,
    ) -> Result<rd_core::CaptureAgentSettings> {
        let endpoint = self.service.join("api/v1/capture/game-mode")?;
        let body = serde_json::json!({ "enabled": enabled });
        let response = self.send(self.http.post(endpoint).json(&body)).await?;
        let response = ensure_success(response, "game mode switch").await?;
        Ok(response.json().await?)
    }
}
