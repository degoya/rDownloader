//! What the download window reads from the bandwidth state (RD-1240-30): whether the profile in
//! force pauses downloads, and the timezone every window is read in.

use chrono_tz::Tz;

use super::BandwidthService;

impl BandwidthService {
    /// Whether the profile in force pauses downloads, and the schedule's timezone.
    ///
    /// The profile in force, not the schedule's: a profile switched on by hand stands in front
    /// of it here as everywhere, which is how "download now anyway" works — switch to a profile
    /// that does not pause, or to no limits, until a time.
    pub async fn download_pause_policy(&self) -> (bool, Tz) {
        let state = self.state.read().await;
        let pauses = state
            .active_profile()
            .is_some_and(|profile| profile.pause_downloads);
        (pauses, state.schedule.timezone)
    }
}
